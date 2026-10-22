//! Official local Claude Code client; authentication stays with that client.
use crate::{
    domain::{Chat, Provider, TokenUsage, UsageRecord},
    runtime::{Event, Supervisor},
    settings::{self, AgentSettings, AuthStatus},
    tool_runtime::Tools,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::watch,
};

pub const EFFORTS: &[&str] = &["", "low", "medium", "high", "xhigh", "max"];
pub fn executable(config: &AgentSettings) -> Result<PathBuf, String> {
    let path = if config.executable.is_empty() {
        settings::detect_client(Provider::Claude)
            .ok_or("Instalá Claude Code y seleccioná su ejecutable.")?
    } else {
        PathBuf::from(&config.executable)
    };
    #[cfg(windows)]
    let path = if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        path.parent()
            .unwrap_or(Path::new("."))
            .join("node_modules/@anthropic-ai/claude-code/bin/claude.exe")
    } else {
        path
    };
    if !path.is_absolute() || !path.is_file() {
        return Err("No se encontró el ejecutable nativo de Claude Code.".into());
    }
    Ok(path)
}
pub fn verify(config: &AgentSettings) -> Result<String, String> {
    let path = executable(config)?;
    let (ok, version) = settings::run_official(&path, &["--version"], Duration::from_secs(15))?;
    if !ok || !version.contains("Claude Code") {
        return Err("El ejecutable no se identificó como Claude Code.".into());
    }
    let version_numbers: Vec<u32> = version
        .split_whitespace()
        .next()
        .unwrap_or("")
        .split('.')
        .filter_map(|n| n.parse().ok())
        .collect();
    if version_numbers.as_slice() < [2, 1, 259].as_slice() || version_numbers.len() != 3 {
        return Err("Actualiza Claude Code a 2.1.259 o posterior.".into());
    }
    let (ok, auth) = settings::run_official(&path, &["auth", "status"], Duration::from_secs(20))?;
    if settings::classify_auth(Provider::Claude, ok, &auth) != AuthStatus::Subscription {
        return Err("Iniciá sesión en el cliente oficial con tu cuenta de Claude. No se habilita una conexión con clave API.".into());
    }
    Ok("Claude Code verificado · sesión claude.ai en el cliente oficial.".into())
}
pub fn login(config: &AgentSettings) -> Result<String, String> {
    let mut native = config.clone();
    native.executable = executable(config)?.to_string_lossy().into_owned();
    settings::login_client(Provider::Claude, &native)?;
    verify(&native)
}
/// Read-only CLI control request. No user message, tools or inference request.
pub async fn account_usage(config: AgentSettings) -> Result<crate::quota::AccountUsage, String> {
    let checked = config.clone();
    tokio::task::spawn_blocking(move || verify(&checked))
        .await
        .map_err(|_| "No se pudo verificar Claude.")??;
    let path = executable(&config)?;
    let directory = tempfile::tempdir().map_err(|_| "No se pudo preparar la consulta de cuota.")?;
    let mut command = tokio::process::Command::from(settings::official_command(&path));
    command
        .current_dir(directory.path())
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.args([
        "--restricted",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--no-session-persistence",
        "--settings",
        r#"{"disableAllHooks":true,"apiKeyHelper":""}"#,
    ]);
    let mut child = command
        .spawn()
        .map_err(|_| "No se pudo consultar la cuota en Claude Code.")?;
    #[cfg(windows)]
    let _job =
        crate::tool_runtime::ProcessJob::attach(child.id().ok_or("Proceso sin identificador.")?)?;
    let mut input = child
        .stdin
        .take()
        .ok_or("Claude no abrió el canal de control.")?;
    let mut output = BufReader::new(child.stdout.take().ok_or("Claude no abrió la salida.")?);
    let query = async {
        for (id, request) in [
            ("quota-init", json!({"subtype":"initialize","hooks":{}})),
            ("quota-read", json!({"subtype":"get_usage"})),
        ] {
            let line = format!(
                "{}\n",
                json!({"type":"control_request","request_id":id,"request":request})
            );
            input
                .write_all(line.as_bytes())
                .await
                .map_err(|_| "No se pudo enviar la consulta de cuota.")?;
            let mut total = 0usize;
            loop {
                let mut frame = Vec::new();
                let count = (&mut output)
                    .take(1024 * 1024 + 1)
                    .read_until(b'\n', &mut frame)
                    .await
                    .map_err(|_| "No se pudo leer la cuota de Claude.")?;
                total += count;
                if count == 0 || frame.len() > 1024 * 1024 || total > 2 * 1024 * 1024 {
                    return Err("Claude cerró o excedió el canal de cuota.".into());
                }
                let value: Value = serde_json::from_slice(&frame)
                    .map_err(|_| "Claude informó una cuota inválida.")?;
                if value["type"] == "control_response" && value["response"]["request_id"] == id {
                    if value["response"]["subtype"] != "success" {
                        return Err("El cliente de Claude no admite la consulta estructurada de cuota. Actualizalo.".into());
                    }
                    if id == "quota-read" {
                        return crate::quota::AccountUsage::parse_claude(
                            &value["response"]["response"],
                        );
                    }
                    break;
                }
            }
        }
        Err("Claude no informó su cuota.".into())
    };
    let result = tokio::time::timeout(Duration::from_secs(45), query)
        .await
        .map_err(|_| "La consulta de cuota de Claude tardó más de 45 segundos.")?;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
pub fn usage(value: &Value) -> TokenUsage {
    let input = value["input_tokens"].as_u64();
    let read = value["cache_read_input_tokens"].as_u64();
    let write = value["cache_creation_input_tokens"].as_u64();
    TokenUsage {
        input: input.map(|n| {
            n.saturating_add(read.unwrap_or(0))
                .saturating_add(write.unwrap_or(0))
        }),
        output: value["output_tokens"].as_u64(),
        cache_read: read,
        cache_write: write,
        reasoning: None,
    }
}
fn text(message: &Value) -> String {
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
fn check_init(value: &Value) -> Result<(), String> {
    let tools = value["tools"]
        .as_array()
        .ok_or("Claude no informó sus herramientas.")?;
    for tool in tools {
        let name = tool.as_str().ok_or("Herramienta de Claude inválida.")?;
        if name != "EndConversation"
            && !crate::tool_runtime::schemas()[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| name == format!("mcp__harness__{}", t["name"].as_str().unwrap()))
        {
            return Err(format!(
                "Claude habilitó una herramienta externa: {name}. Se detuvo el turno."
            ));
        }
    }
    if !value["mcp_servers"].as_array().is_some_and(|servers| {
        servers
            .iter()
            .any(|s| s["name"] == "harness" && s["status"] == "connected")
    }) {
        return Err("Claude no pudo conectar las herramientas del harness.".into());
    }
    Ok(())
}
pub(crate) async fn run(
    supervisor: &Supervisor,
    chat: &Chat,
    config: AgentSettings,
    project: &str,
    tools: &Tools,
    mut cancel: watch::Receiver<bool>,
    emit: &Arc<dyn Fn(Event) + Send + Sync>,
    journal: &mut crate::diagnostics::Journal,
    incomplete: &mut bool,
) -> Result<(), String> {
    *incomplete = true;
    let checked = config.clone();
    tokio::task::spawn_blocking(move || verify(&checked))
        .await
        .map_err(|_| "Falló la comprobación de Claude.")??;
    if *cancel.borrow() {
        return Err("Cancelado.".into());
    }
    if !EFFORTS.contains(&config.reasoning_effort.as_str()) {
        return Err("Effort no compatible con Claude.".into());
    }
    let path = executable(&config)?;
    let mut bridge = crate::mcp::Bridge::new().await?;
    let session = chat
        .provider_session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid::Uuid::parse_str(&session).map_err(|_| "La sesión guardada de Claude es inválida.")?;
    let mcp = json!({"mcpServers":{"harness":{"type":"http","url":bridge.url,"headers":{"Authorization":"Bearer ${ASCURRA_MCP_TOKEN}"}}}}).to_string();
    let mut command = tokio::process::Command::from(settings::official_command(&path));
    command
        .current_dir(project)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let system_prompt = format!(
        "You are a programming agent working on the current repository. Use only the harness MCP tools. Prefer search_files for repository searches, file_outline for document headings or heuristic declarations, and git_read for read-only status/diff instead of command scripts. For targeted file excerpts prefer read_file_ranges to batch multiple line ranges or files in one call instead of Get-Content/Select-Object pipelines or helper scripts. For PowerShell scripts prefer powershell_run with script and timeout_seconds rather than repeating powershell -NoProfile -Command through run_command. Tool outputs are intentionally bounded; inspect a result artifact only when needed. For image outputs (screenshots, diagrams, charts, illustrations or other images), save PNG/JPEG files directly in the HARNESS_ARTIFACT_DIR environment directory provided to approved commands, or use register_image to copy a repository image into the chat gallery. Generated image artifacts stay out of model context. User-attached images are included in the user message. Prefer targeted reads; more=true means data is available, not that it must all be read. For structural changes to large documents, obtain a small heading outline and use an approved local script to move existing sections without copying the entire document through model context. Read section contents only when needed to make semantic edits. File changes, command execution and HTTP requests require approval in the harness UI. Answer the user in their language.{}",
        chat.project_context
    );
    command.args(["--restricted", "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--tools", "", "--strict-mcp-config", "--mcp-config", &mcp,
        "--allowedTools", "mcp__harness__browser,mcp__harness__list_files,mcp__harness__read_file,mcp__harness__read_file_ranges,mcp__harness__search_files,mcp__harness__file_outline,mcp__harness__git_read,mcp__harness__write_file,mcp__harness__run_command,mcp__harness__powershell_run,mcp__harness__inspect_result,mcp__harness__http_collections,mcp__harness__http_request,mcp__harness__register_image",
        "--system-prompt", &system_prompt,
        "--max-turns", "32"]);
    command.args([
        "--settings",
        r#"{"disableAllHooks":true,"apiKeyHelper":""}"#,
        "--permission-prompts",
        "none",
        "--no-chrome",
    ]);
    command
        .arg(if chat.provider_session_id.is_some() {
            "--resume"
        } else {
            "--session-id"
        })
        .arg(&session);
    if !config.model.is_empty() {
        command.args(["--model", &config.model]);
    }
    if !config.reasoning_effort.is_empty() {
        command.args(["--effort", &config.reasoning_effort]);
    }
    command
        .env("ASCURRA_MCP_TOKEN", &bridge.token)
        .env("MCP_TOOL_TIMEOUT", "43200000");
    let mut child = command
        .spawn()
        .map_err(|_| "No se pudo iniciar Claude Code.")?;
    #[cfg(windows)]
    let _job = crate::tool_runtime::ProcessJob::attach(
        child.id().ok_or("Proceso Claude sin identificador.")?,
    )?;
    let stderr = child.stderr.take().ok_or("Claude no abrió stderr.")?;
    // Drain without retaining raw diagnostics or account data. Exceeding the cap closes the pipe.
    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr.take(1024 * 1024);
        tokio::io::copy(&mut reader, &mut tokio::io::sink()).await
    });
    struct Drain(tokio::task::JoinHandle<std::io::Result<u64>>);
    impl Drop for Drain {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _drain = Drain(stderr_task);
    let mut stdin = child.stdin.take().ok_or("Claude no abrió stdin.")?;
    let prompt = chat.messages.last().ok_or("Mensaje vacío.")?.text.clone();
    let content = crate::attachments::claude_content(
        &prompt,
        chat.message_images
            .get(&(chat.messages.len() - 1))
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        tools.data_root(),
        chat.id,
    )?;
    let input = format!(
        "{}\n",
        json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":session})
    );
    stdin
        .write_all(input.as_bytes())
        .await
        .map_err(|_| "No se pudo enviar el mensaje a Claude.")?;
    stdin
        .shutdown()
        .await
        .map_err(|_| "No se pudo cerrar la entrada de Claude.")?;
    drop(stdin);
    let mut stdout = BufReader::new(child.stdout.take().ok_or("Claude no abrió stdout.")?);
    let mut frame = Vec::new();
    let mut total = 0usize;
    let mut initialized = false;
    let mut visible = false;
    let mut seen = std::collections::HashSet::new();
    let mut tool_index = 0;
    let mut result = None;
    let mut latest_context = None;
    let mut latest_window = None;
    let mut latest_model = config.model.clone();
    let mut pending = String::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut first_text_ms = None;
    let started = std::time::Instant::now();
    loop {
        let mut limited = (&mut stdout).take((2 * 1024 * 1024 + 1 - frame.len()) as u64);
        tokio::select! {
            _ = crate::codex::cancelled(&mut cancel) => return Err("Cancelado.".into()),
            _ = ticker.tick(), if !pending.is_empty() => { emit(Event::Text { chat:chat.id, delta:std::mem::take(&mut pending) }); }
            call = bridge.calls.recv(), if initialized => {
                let Some(call) = call else { return Err("El puente MCP se cerró.".into()) };
                if !pending.is_empty() { emit(Event::Text { chat:chat.id, delta:std::mem::take(&mut pending) }); }
                let reply = supervisor.host_tool(chat.id, tools, &call.name, &call.arguments, cancel.clone(), emit, journal, tool_index).await;
                tool_index += 1;
                let _ = call.reply.send(reply);
            }
            read = tokio::time::timeout(Duration::from_secs(180), limited.read_until(b'\n', &mut frame)) => {
                let count = read.map_err(|_| "Claude no respondió durante tres minutos.")?.map_err(|_| "No se pudo leer Claude.")?;
                if count == 0 { break; }
                total += count;
                if frame.len() > 2 * 1024 * 1024 || total > 16 * 1024 * 1024 { return Err("Claude excedió el límite de salida del turno.".into()); }
                let value: Value = serde_json::from_slice(&frame).map_err(|_| "Claude envió un evento JSON inválido.")?;
                frame.clear();
                match value["type"].as_str().unwrap_or("") {
                    "system" if value["subtype"] == "init" => {
                        check_init(&value)?;
                        if value["session_id"] != session { return Err("Claude informó una sesión diferente al chat.".into()); }
                        initialized = true;
                        emit(Event::Session { chat:chat.id, session_id:session.clone() });
                        journal.record("client_initialized", json!({"provider":"claude","model":value["model"],"duration_ms":started.elapsed().as_millis()}));
                    }
                    "stream_event" if initialized => {
                        if value["event"]["type"] == "message_start" { emit(Event::GenerationStarted { chat:chat.id }); }
                        if let Some(delta) = value["event"]["delta"]["text"].as_str() {
                            visible |= !delta.trim().is_empty();
                            first_text_ms.get_or_insert(started.elapsed().as_millis());
                            pending.push_str(delta);
                        }
                    }
                    "assistant" if initialized => {
                        if !pending.is_empty() { emit(Event::Text { chat:chat.id, delta:std::mem::take(&mut pending) }); }
                        let message = &value["message"];
                        latest_model = message["model"].as_str().unwrap_or(&config.model).to_string();
                        let answer = text(message);
                        if !answer.is_empty() { visible |= !answer.trim().is_empty(); emit(Event::ResponseText { chat:chat.id, text:answer }); }
                        if let Some(id) = message["id"].as_str() {
                            if message["usage"].is_object() {
                                seen.insert(id.to_string());
                                let usage = usage(&message["usage"]);
                                latest_context = usage.input;
                                journal.record("generation_finished", json!({"request_id":id,"model":message["model"],"usage":usage,"elapsed_ms":started.elapsed().as_millis()}));
                                emit(Event::Usage { chat:chat.id, record:UsageRecord { request_id:id.into(), model:message["model"].as_str().unwrap_or(&config.model).into(), provider:Provider::Claude, context_input:usage.input, context_window:None, usage } });
                            }
                        }
                    }
                    "result" if initialized => {
                        latest_window = value["modelUsage"][&latest_model]["contextWindow"].as_u64();
                        result = Some(value);
                    }
                    _ => {}
                }
            }
        }
    }
    if !pending.is_empty() {
        emit(Event::Text {
            chat: chat.id,
            delta: pending,
        });
    }
    let status = tokio::select! { _ = crate::codex::cancelled(&mut cancel) => return Err("Cancelado.".into()), value = tokio::time::timeout(Duration::from_secs(10), child.wait()) => value.map_err(|_| "Claude no cerró su proceso.")?.map_err(|_| "No se pudo esperar a Claude.")? };
    let value =
        result.ok_or("Claude terminó sin informar un resultado; el turno está incompleto.")?;
    // The final usage is authoritative and aggregate. Replace per-message records by a
    // single turn record in the UI using a dedicated event to avoid counting twice.
    let measured = usage(&value["usage"]);
    *incomplete = !value["usage"].is_object();
    if !*incomplete {
        let request_id = value["uuid"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        emit(Event::TurnUsage {
            chat: chat.id,
            request_ids: seen.into_iter().collect(),
            record: UsageRecord {
                request_id,
                model: latest_model,
                provider: Provider::Claude,
                usage: measured,
                context_input: latest_context,
                context_window: latest_window,
            },
        });
    }
    journal.record("client_finished", json!({"subtype":value["subtype"],"is_error":value["is_error"],"usage":value["usage"],"duration_ms":started.elapsed().as_millis(),"output_bytes":total,"tool_calls":tool_index,"exit_code":status.code(),"first_text_ms":first_text_ms}));
    if !status.success() || value["is_error"] == true || value["subtype"] != "success" {
        return Err("Claude no completó el turno. Revisá autenticación, límites o disponibilidad del modelo en el cliente oficial.".into());
    }
    if !visible {
        let answer = value["result"].as_str().unwrap_or("");
        if answer.trim().is_empty() {
            return Err("Claude terminó sin una respuesta visible.".into());
        }
        emit(Event::ResponseText {
            chat: chat.id,
            text: answer.into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_is_included_in_context_but_reasoning_is_unknown() {
        let u = usage(
            &json!({"input_tokens":10,"cache_read_input_tokens":30,"cache_creation_input_tokens":20,"output_tokens":4}),
        );
        assert_eq!(u.input, Some(60));
        assert_eq!(u.output, Some(4));
        assert_eq!(u.reasoning, None);
        assert_eq!(usage(&json!({})).input, None);
    }
    #[test]
    fn unexpected_builtin_tool_is_rejected() {
        assert!(
            check_init(
                &json!({"tools":["Bash"],"mcp_servers":[{"name":"harness","status":"connected"}]})
            )
            .is_err()
        );
        assert!(check_init(&json!({"tools":["mcp__harness__read_file"],"mcp_servers":[{"name":"harness","status":"connected"}]})).is_ok());
    }
}
