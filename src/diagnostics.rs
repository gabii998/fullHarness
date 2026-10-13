//! Local, bounded diagnostics. Never sent to the model or appended to chat context.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct Journal {
    directory: PathBuf,
    file: fs::File,
    bytes: usize,
    started: Instant,
    last_sync: Instant,
    unsynced_bytes: usize,
    pub error: Option<String>,
}
const CAP: usize = 16 * 1024 * 1024;
impl Journal {
    pub fn new(data: &Path, chat: u64) -> Result<Self, String> {
        let directory = data
            .join("logs")
            .join(chat.to_string())
            .join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&directory)
            .map_err(|_| "No se pudieron preparar los logs de la tarea.")?;
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("events.jsonl"))
            .map_err(|_| "No se pudo crear el log de la tarea.")?;
        Ok(Self {
            directory,
            file,
            bytes: 0,
            started: Instant::now(),
            last_sync: Instant::now(),
            unsynced_bytes: 0,
            error: None,
        })
    }
    pub fn record(&mut self, kind: &str, details: Value) {
        if self.error.is_some() {
            return;
        }
        let line = json!({"version":1,"timestamp_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "elapsed_ms":self.started.elapsed().as_millis(),"event":kind,"details":details}).to_string()+"\n";
        if self.bytes + line.len() > CAP {
            self.error = Some("Se alcanzó el límite de 16 MiB del log.".into());
            return;
        }
        if self.file.write_all(line.as_bytes()).is_err() {
            self.error = Some("No se pudo guardar el log completo.".into());
        } else {
            self.bytes += line.len();
            self.unsynced_bytes += line.len();
            // Bound unsynced data, while keeping approvals and task boundaries durable.
            if self.unsynced_bytes >= 64 * 1024
                || self.last_sync.elapsed() >= std::time::Duration::from_secs(1)
                || kind.contains("approval")
                || kind == "run_finished"
                || kind == "task_finished"
                || kind.contains("failed")
                || kind.contains("cancelled")
            {
                if self.file.sync_data().is_err() {
                    self.error = Some("No se pudo sincronizar el log completo.".into());
                } else {
                    self.unsynced_bytes = 0;
                    self.last_sync = Instant::now();
                }
            }
        }
    }
    pub fn tool_reply(&mut self, index: usize, reply: &str) -> Option<String> {
        if self.error.is_some() {
            return None;
        }
        // Reserve one MiB for events; omitting a detail must not stop metrics.
        if reply.len() > 128 * 1024 || self.bytes + reply.len() > CAP - 1024 * 1024 {
            self.record(
                "tool_detail_omitted",
                json!({
                    "index": index, "reply_bytes": reply.len(), "reason": "detail_size_limit"
                }),
            );
            return None;
        }
        let name = format!("tool-{index}.json");
        if fs::write(self.directory.join(&name), reply).is_err() {
            self.error = Some("No se pudo guardar el detalle de una herramienta.".into());
            return None;
        }
        self.bytes += reply.len();
        Some(name)
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        // Best effort at shutdown; ordinary sync failures are reported by record().
        let _ = self.file.sync_data();
    }
}

fn short(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}
/// Payload sizes only; never retain argument/output content in the metrics.
/// This is an estimate of one exposure, not provider billing or tokenization.
pub fn payload_metrics(arguments: &Value, reply: &str) -> Value {
    let arguments = arguments.to_string();
    let argument_chars = arguments.chars().count();
    let reply_chars = reply.chars().count();
    json!({"version":1,"argument_bytes":arguments.len(),"reply_bytes":reply.len(),
        "argument_chars":argument_chars,"reply_chars":reply_chars,
        "estimated_argument_tokens":argument_chars.div_ceil(4),
        "estimated_reply_tokens":reply_chars.div_ceil(4),"method":"characters_divided_by_4",
        "result_reference":serde_json::from_str::<Value>(reply).ok().and_then(|v|v["ref"].as_str().map(short))})
}
/// Count serialized payload bytes without allocating a second base64 image payload.
pub fn json_size(value: &Value) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Counter(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}
pub fn argument_metrics(name: &str, arguments: &Value) -> Value {
    match name {
        "http_request" | "http_collections" => {
            json!({"collection":short(arguments["collection"].as_str().unwrap_or("")),"request":short(arguments["request"].as_str().unwrap_or(""))})
        }
        "write_file" => {
            json!({"path":short(arguments["path"].as_str().unwrap_or("")),"content_bytes":arguments["content"].as_str().map(str::len)})
        }
        "run_command" => {
            json!({"program":short(arguments["program"].as_str().unwrap_or("")),"argument_count":arguments["args"].as_array().map(Vec::len),"timeout_seconds":arguments["timeout_seconds"]})
        }
        _ => {
            json!({"path":short(arguments["path"].as_str().unwrap_or("")),"reference":short(arguments["reference"].as_str().unwrap_or("")),
            "stream":short(arguments["stream"].as_str().unwrap_or("")),"offset":arguments["offset"],"limit":arguments["limit"]})
        }
    }
}

/// UI-only detail. Keep the compact journal summary and model reply unchanged.
/// Arguments are quoted as JSON strings when necessary to preserve boundaries;
/// this is a display of argv, not a shell script.
pub fn tool_display_summary(name: &str, arguments: &Value, summary: &str) -> String {
    if name != "run_command" {
        return summary.into();
    }
    let Some(program) = arguments["program"].as_str().filter(|s| !s.is_empty()) else {
        return summary.into();
    };
    fn token(value: &str) -> String {
        if !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_alphanumeric() || "-_=./:".contains(c))
        {
            value.into()
        } else {
            serde_json::to_string(value).unwrap_or_default()
        }
    }
    let mut command = token(program);
    if let Some(args) = arguments["args"].as_array() {
        for arg in args {
            if let Some(arg) = arg.as_str() {
                command.push(' ');
                command.push_str(&token(arg));
            }
        }
    }
    format!("{summary}\n\nComando (programa y argumentos):\n{command}")
}

pub fn tool_summary(
    name: &str,
    arguments: &Value,
    reply: &str,
    elapsed_ms: Option<u128>,
) -> String {
    let value: Value = serde_json::from_str(reply).unwrap_or(Value::Null);
    let failed = value["ok"] == false || !value.is_object();
    let state = if let Some(state) = value["state"].as_str() {
        match state {
            "cancelled" => "Cancelado",
            "timed_out" => "Tiempo agotado",
            "denied" => "No aprobado",
            _ => "Interrumpido",
        }
    } else if failed {
        "Falló"
    } else {
        "Listo"
    };
    let path = short(arguments["path"].as_str().unwrap_or(""));
    let detail = match name {
        "list_files" => value["entries"]
            .as_array()
            .map(|rows| {
                format!(
                    "{} entradas{}",
                    rows.len(),
                    if value["truncated"] == true {
                        " · listado parcial"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default(),
        "read_file" | "inspect_result" => value["text"]
            .as_str()
            .map(|text| {
                let range = arguments["offset"]
                    .as_u64()
                    .map(|start| {
                        let end = value["next_offset"]
                            .as_u64()
                            .unwrap_or_else(|| start.saturating_add(text.len() as u64));
                        format!(" · rango [{start}, {end})")
                    })
                    .unwrap_or_default();
                format!(
                    "{} bytes{}{}",
                    text.len(),
                    range,
                    if value["more"] == true {
                        " · hay más"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default(),
        "write_file" => arguments["content"]
            .as_str()
            .map(|text| format!("{} bytes", text.len()))
            .unwrap_or_default(),
        "run_command" => short(arguments["program"].as_str().unwrap_or("")),
        "http_request" => value["status"]
            .as_u64()
            .map(|s| format!("HTTP {s}"))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let label = match name {
        "list_files" => "Listar carpeta",
        "read_file" => "Leer archivo",
        "write_file" => "Guardar archivo",
        "run_command" => "Ejecutar comando",
        "inspect_result" => "Consultar resultado",
        "http_collections" => "Colecciones HTTP",
        "http_request" => "Request HTTP",
        "register_image" | "register_capture" => "Guardar imagen",
        _ => "Herramienta",
    };
    format!(
        "{label}{} · {state}{}{}",
        if path.is_empty() {
            String::new()
        } else {
            format!(" · {path}")
        },
        if detail.is_empty() || failed || (state != "Listo" && name != "run_command") {
            String::new()
        } else {
            format!(" · {detail}")
        },
        elapsed_ms
            .map(|ms| format!(" · {ms} ms"))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_detail_preserves_arguments_without_changing_compact_summary() {
        let args =
            json!({"program":"cargo","args":["build","--offline","", "a b", "a\"b", "line\nnext"]});
        let summary = tool_summary(
            "run_command",
            &args,
            r#"{"ok":true,"ref":"private-ref"}"#,
            Some(12),
        );
        let display = tool_display_summary("run_command", &args, &summary);
        assert!(display.starts_with(&format!("{summary}\n\n")));
        assert!(display.contains("cargo build --offline \"\" \"a b\" \"a\\\"b\" \"line\\nnext\""));
        assert!(!summary.contains("--offline"));
        assert!(!display.contains("private-ref"));
        assert_eq!(tool_display_summary("read_file", &args, &summary), summary);
        assert_eq!(
            tool_display_summary("run_command", &Value::Null, "old"),
            "old"
        );
        let long = "x".repeat(300);
        assert!(
            tool_display_summary(
                "run_command",
                &json!({"program":"echo","args":[long]}),
                "ok"
            )
            .contains(&long)
        );
    }
    #[test]
    fn detail_budget_preserves_event_space_and_sync_is_batched() {
        let temp = tempfile::tempdir().unwrap();
        let mut journal = Journal::new(temp.path(), 1).unwrap();
        journal.bytes = CAP - 1024 * 1024;
        assert!(journal.tool_reply(0, "detail").is_none());
        assert!(journal.error.is_none());
        journal.record("tool_finished", json!({"ok": true}));
        assert!(journal.unsynced_bytes > 0);
        journal.record("approval_requested", json!({}));
        assert_eq!(journal.unsynced_bytes, 0);
    }
    #[test]
    fn oversized_detail_does_not_stop_events() {
        let temp = tempfile::tempdir().unwrap();
        let mut journal = Journal::new(temp.path(), 1).unwrap();
        assert!(journal.tool_reply(0, &"x".repeat(128 * 1024 + 1)).is_none());
        assert!(journal.error.is_none());
        journal.record("task_finished", json!({"ok": true}));
        let text = fs::read_to_string(journal.directory.join("events.jsonl")).unwrap();
        assert!(text.contains("tool_detail_omitted"));
        assert!(text.contains("task_finished"));
        assert_eq!(journal.unsynced_bytes, 0);
    }
    #[test]
    fn summaries_hide_content_and_refs_and_logs_preserve_bounded_details() {
        let args = json!({"path":"src/main.rs","content":"secret fixture"});
        let summary = tool_summary(
            "write_file",
            &args,
            r#"{"ok":true,"ref":"fixture-ref"}"#,
            Some(12),
        );
        assert!(summary.contains("Guardar archivo"));
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("fixture-ref"));
        let read = tool_summary(
            "read_file",
            &json!({"path":"README.md","offset":8192,"limit":8192}),
            r#"{"text":"abc","next_offset":8195,"more":true}"#,
            Some(0),
        );
        assert!(read.contains("README.md"));
        assert!(read.contains("rango [8192, 8195)"));
        assert!(!read.contains("abc"));
        let denied = tool_summary("write_file", &args, r#"{"state":"denied"}"#, Some(0));
        assert!(denied.contains("No aprobado"));
        assert!(!denied.contains("bytes"));
        assert!(
            !argument_metrics("write_file", &args)
                .to_string()
                .contains("secret")
        );
        let temp = tempfile::tempdir().unwrap();
        let mut log = Journal::new(temp.path(), 7).unwrap();
        log.record("tool", json!({"args":argument_metrics("write_file",&args)}));
        let name = log.tool_reply(0, r#"{"text":"bounded fixture"}"#).unwrap();
        assert!(
            fs::read_to_string(log.directory.join(name))
                .unwrap()
                .contains("bounded fixture")
        );
        assert!(
            !fs::read_to_string(log.directory.join("events.jsonl"))
                .unwrap()
                .contains("secret")
        );
        log.bytes = CAP;
        log.record("overflow", json!({}));
        assert!(log.error.is_some());
    }
}
