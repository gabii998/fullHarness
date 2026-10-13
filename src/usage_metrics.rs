//! Offline metrics: provider totals and estimated tool payloads are separate.
use crate::domain::{Provider, TokenUsage, UsageRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BinaryHeap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

const MAX_FILES: usize = 1000;
const MAX_CALLS: usize = 10_000;
const MAX_BYTES: usize = 48 * 1024 * 1024;
const MAX_LINE: u64 = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Call {
    pub chat: u64,
    pub chat_title: String,
    pub run: String,
    pub index: u64,
    pub timestamp_ms: u64,
    pub tool: String,
    pub group: String,
    pub target: String,
    pub argument_bytes: Option<u64>,
    pub reply_bytes: u64,
    pub estimated_tokens: u64,
    pub legacy: bool,
    pub execution_ms: u64,
    pub result_reference: String,
    pub inspected_reference: String,
    pub process: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub stream: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Group {
    pub label: String,
    pub calls: u64,
    pub estimated_tokens: u64,
    pub argument_bytes: u64,
    pub reply_bytes: u64,
    pub execution_ms: u64,
    pub legacy_calls: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub label: String,
    pub requests: u64,
    pub usage: TokenUsage,
    pub incomplete: bool,
    pub cache_incomplete: bool,
    pub reasoning_incomplete: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub chat_ids: Vec<u64>,
    pub calls: Vec<Call>,
    pub groups: Vec<Group>,
    pub processes: Vec<Group>,
    pub models: Vec<ModelUsage>,
    pub files: usize,
    pub partial: bool,
    pub invalid_lines: usize,
    pub missing_chats: usize,
    pub legacy_calls: usize,
    #[serde(default)]
    pub discovered_files: usize,
    #[serde(default)]
    pub repeated_reads: usize,
    #[serde(default)]
    pub inspected_calls: usize,
    #[serde(default)]
    pub read_reply_bytes: u64,
    #[serde(default)]
    pub earliest_timestamp_ms: Option<u64>,
    #[serde(default)]
    pub latest_timestamp_ms: Option<u64>,
    #[serde(default)]
    pub truncation_reasons: Vec<String>,
}
fn limited(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}
fn sum(target: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *target = Some(target.unwrap_or(0).saturating_add(value));
    }
}
pub fn models(samples: &[(u64, UsageRecord)]) -> Vec<ModelUsage> {
    let mut seen = HashSet::new();
    let mut groups = BTreeMap::<String, ModelUsage>::new();
    for (chat, record) in samples {
        let provider = match record.provider {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude",
        };
        if !seen.insert((*chat, provider, record.request_id.clone())) {
            continue;
        }
        let label = format!("{provider} · {}", limited(&record.model));
        let group = groups.entry(label.clone()).or_insert_with(|| ModelUsage {
            label,
            ..Default::default()
        });
        group.requests += 1;
        group.incomplete |= record.usage.input.is_none() || record.usage.output.is_none();
        group.cache_incomplete |= record.usage.cache_read.is_none();
        group.reasoning_incomplete |= record.usage.reasoning.is_none();
        sum(&mut group.usage.input, record.usage.input);
        sum(&mut group.usage.output, record.usage.output);
        sum(&mut group.usage.cache_read, record.usage.cache_read);
        sum(&mut group.usage.cache_write, record.usage.cache_write);
        sum(&mut group.usage.reasoning, record.usage.reasoning);
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by_key(|g| {
        std::cmp::Reverse(
            g.usage
                .input
                .unwrap_or(0)
                .saturating_add(g.usage.output.unwrap_or(0)),
        )
    });
    groups
}
fn call(chat: u64, title: &str, run: &str, line: u64, event: &Value) -> Option<Call> {
    if event["event"] != "tool_finished" {
        return None;
    }
    let d = &event["details"];
    let tool = limited(d["name"].as_str()?);
    let m = &d["payload_metrics"];
    let argument_bytes = m["argument_bytes"].as_u64();
    let reply_bytes = m["reply_bytes"].as_u64().or(d["reply_bytes"].as_u64())?;
    let legacy = m["version"] != 1
        || m["estimated_argument_tokens"].as_u64().is_none()
        || m["estimated_reply_tokens"].as_u64().is_none();
    let estimated_tokens = if legacy {
        reply_bytes.div_ceil(4)
    } else {
        m["estimated_argument_tokens"]
            .as_u64()?
            .saturating_add(m["estimated_reply_tokens"].as_u64()?)
    };
    let target = limited(if tool == "run_command" {
        d["arguments"]["program"].as_str().unwrap_or("")
    } else {
        d["arguments"]["path"].as_str().unwrap_or("")
    });
    let group = if tool == "run_command" && !target.is_empty() {
        format!("run_command · {target}")
    } else {
        tool.clone()
    };
    Some(Call {
        chat,
        chat_title: limited(title),
        run: run.into(),
        index: d["index"].as_u64().unwrap_or(line),
        timestamp_ms: event["timestamp_ms"].as_u64().unwrap_or(0),
        tool,
        group,
        target,
        argument_bytes,
        reply_bytes,
        estimated_tokens,
        legacy,
        execution_ms: d["execution_ms"].as_u64().unwrap_or(0),
        result_reference: limited(m["result_reference"].as_str().unwrap_or("")),
        inspected_reference: limited(d["arguments"]["reference"].as_str().unwrap_or("")),
        process: String::new(),
        offset: d["arguments"]["offset"].as_u64(),
        limit: d["arguments"]["limit"].as_u64(),
        stream: limited(d["arguments"]["stream"].as_str().unwrap_or("")),
    })
}
fn regular(path: &Path, directory: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        !m.file_type().is_symlink() && if directory { m.is_dir() } else { m.is_file() }
    })
}
pub fn scan(data: &Path, chats: &[(u64, String)], samples: &[(u64, UsageRecord)]) -> Report {
    let mut report = Report {
        chat_ids: chats.iter().map(|(id, _)| *id).collect(),
        models: models(samples),
        ..Default::default()
    };
    let logs = data.join("logs");
    if !regular(&logs, true) {
        report.missing_chats = chats.len();
        return report;
    }
    let mut files = BinaryHeap::new();
    for (id, title) in chats {
        let directory = logs.join(id.to_string());
        if !regular(&directory, true) {
            report.missing_chats += 1;
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            report.partial = true;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !regular(&path, true) {
                continue;
            }
            let file = path.join("events.jsonl");
            if !regular(&file, false) {
                continue;
            }
            report.discovered_files += 1;
            files.push(std::cmp::Reverse((
                fs::metadata(&file).and_then(|m| m.modified()).ok(),
                file,
                *id,
                title.clone(),
            )));
            if files.len() > MAX_FILES {
                files.pop();
                report.partial = true;
            }
        }
    }
    if report.discovered_files > MAX_FILES {
        report.truncation_reasons.push("file_limit".into());
    }
    let mut files = files.into_iter().map(|v| v.0).collect::<Vec<_>>();
    files.sort_by(|a, b| b.cmp(a));
    let mut bytes = 0usize;
    'files: for (_, file, chat, title) in files {
        if bytes >= MAX_BYTES || report.calls.len() >= MAX_CALLS {
            report.partial = true;
            report.truncation_reasons.push(
                if bytes >= MAX_BYTES {
                    "byte_limit"
                } else {
                    "call_limit"
                }
                .into(),
            );
            break;
        }
        let run = file
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Ok(file) = fs::File::open(file) else {
            report.partial = true;
            continue;
        };
        let mut reader = BufReader::new(file);
        report.files += 1;
        let mut line_index = 0;
        loop {
            let mut line = Vec::new();
            let remaining = MAX_BYTES.saturating_sub(bytes).min(MAX_LINE as usize) as u64;
            if remaining == 0 {
                report.partial = true;
                report.truncation_reasons.push("byte_limit".into());
                break 'files;
            }
            let result = reader
                .by_ref()
                .take(remaining + 1)
                .read_until(b'\n', &mut line);
            let Ok(n) = result else {
                report.partial = true;
                break;
            };
            if n == 0 {
                break;
            }
            bytes += n;
            if n as u64 > remaining {
                report.partial = true;
                report.truncation_reasons.push(
                    if remaining < MAX_LINE {
                        "byte_limit"
                    } else {
                        "line_limit"
                    }
                    .into(),
                );
                break;
            }
            line_index += 1;
            if !line.ends_with(b"\n") {
                report.partial = true;
                report.truncation_reasons.push("unfinished_line".into());
                break;
            } // Running/partial log, retry on refresh.
            let Ok(event) = serde_json::from_slice::<Value>(&line) else {
                report.invalid_lines += 1;
                continue;
            };
            if let Some(call) = call(chat, &title, &run, line_index, &event) {
                report.legacy_calls += usize::from(call.legacy);
                report.calls.push(call);
                if report.calls.len() >= MAX_CALLS {
                    report.partial = true;
                    report.truncation_reasons.push("call_limit".into());
                    break 'files;
                }
            } else if event["event"] == "tool_finished" {
                report.invalid_lines += 1;
            }
        }
    }
    let mut reads = HashSet::new();
    for c in &report.calls {
        if c.timestamp_ms > 0 {
            report.earliest_timestamp_ms = Some(
                report
                    .earliest_timestamp_ms
                    .map_or(c.timestamp_ms, |t| t.min(c.timestamp_ms)),
            );
            report.latest_timestamp_ms = Some(
                report
                    .latest_timestamp_ms
                    .map_or(c.timestamp_ms, |t| t.max(c.timestamp_ms)),
            );
        }
        if c.tool == "inspect_result" {
            report.inspected_calls += 1;
        }
        if c.tool == "read_file" || c.tool == "inspect_result" {
            report.read_reply_bytes = report.read_reply_bytes.saturating_add(c.reply_bytes);
            if let (Some(offset), Some(limit)) = (c.offset, c.limit) {
                let target = if c.tool == "read_file" {
                    &c.target
                } else {
                    &c.inspected_reference
                };
                if !target.is_empty()
                    && !reads.insert((c.chat, &c.run, &c.tool, target, &c.stream, offset, limit))
                {
                    report.repeated_reads += 1;
                }
            }
        }
    }
    report.truncation_reasons.sort();
    report.truncation_reasons.dedup();
    let sources = report
        .calls
        .iter()
        .filter(|c| c.tool == "run_command" && !c.result_reference.is_empty())
        .map(|c| ((c.chat, c.result_reference.clone()), c.target.clone()))
        .collect::<BTreeMap<_, _>>();
    for call in &mut report.calls {
        if call.tool == "run_command" {
            call.process = if call.target.is_empty() {
                "Comando no informado".into()
            } else {
                call.target.clone()
            };
        } else if call.tool == "inspect_result" {
            call.process = sources
                .get(&(call.chat, call.inspected_reference.clone()))
                .cloned()
                .unwrap_or_default();
        }
    }
    let mut groups = BTreeMap::<String, Group>::new();
    let mut processes = BTreeMap::<String, Group>::new();
    for call in &report.calls {
        let g = groups.entry(call.group.clone()).or_insert_with(|| Group {
            label: call.group.clone(),
            ..Default::default()
        });
        g.calls += 1;
        g.estimated_tokens = g.estimated_tokens.saturating_add(call.estimated_tokens);
        g.argument_bytes = g
            .argument_bytes
            .saturating_add(call.argument_bytes.unwrap_or(0));
        g.reply_bytes = g.reply_bytes.saturating_add(call.reply_bytes);
        g.execution_ms = g.execution_ms.saturating_add(call.execution_ms);
        g.legacy_calls += u64::from(call.legacy);
        if !call.process.is_empty() {
            let p = processes
                .entry(call.process.clone())
                .or_insert_with(|| Group {
                    label: call.process.clone(),
                    ..Default::default()
                });
            p.calls += 1;
            p.estimated_tokens = p.estimated_tokens.saturating_add(call.estimated_tokens);
            p.argument_bytes = p
                .argument_bytes
                .saturating_add(call.argument_bytes.unwrap_or(0));
            p.reply_bytes = p.reply_bytes.saturating_add(call.reply_bytes);
            p.execution_ms = p.execution_ms.saturating_add(call.execution_ms);
            p.legacy_calls += u64::from(call.legacy);
        }
    }
    report.groups = groups.into_values().collect();
    report.groups.sort_by(|a, b| {
        b.estimated_tokens
            .cmp(&a.estimated_tokens)
            .then(a.label.cmp(&b.label))
    });
    report.calls.sort_by(|a, b| {
        b.estimated_tokens
            .cmp(&a.estimated_tokens)
            .then(b.timestamp_ms.cmp(&a.timestamp_ms))
    });
    report.processes = processes.into_values().collect();
    report.processes.sort_by(|a, b| {
        b.estimated_tokens
            .cmp(&a.estimated_tokens)
            .then(a.label.cmp(&b.label))
    });
    report
}
pub fn report_folder(data: &Path, project: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = Sha256::digest(project.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    data.join("metrics").join(key)
}
pub fn save(data: &Path, project: &str, scope: &str, report: &Report) -> std::io::Result<PathBuf> {
    let folder = report_folder(data, project);
    fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{scope}-report.json"));
    let mut temp = tempfile::NamedTempFile::new_in(&folder)?;
    serde_json::to_writer(
        &mut temp,
        &serde_json::json!({"version":1,"generated_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),"project":project,"scope":scope,"tool_estimate_method":"characters_divided_by_4; legacy replies use bytes_divided_by_4","provider_totals_include_cache_and_reasoning_subsets":true,"report":report}),
    )?;
    temp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn newest_files_are_selected_globally_with_bounded_retention() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..=MAX_FILES {
            let dir = temp.path().join(format!(
                "logs/{}/run-{i:04}",
                if i == MAX_FILES { 2 } else { 1 }
            ));
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("events.jsonl");
            let event = json!({"event":"tool_finished","timestamp_ms":i as u64 + 1,
                "details":{"name":"read_file","reply_bytes":4,"arguments":{"path":"fixture"}}});
            fs::write(&path, format!("{event}\n")).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(1_700_000_000 + i as u64),
                )
                .unwrap();
        }
        let report = scan(temp.path(), &[(1, "Chat".into()), (2, "Newer".into())], &[]);
        assert_eq!(report.discovered_files, MAX_FILES + 1);
        assert_eq!(report.files, MAX_FILES);
        assert!(report.partial);
        assert_eq!(report.earliest_timestamp_ms, Some(2));
        assert_eq!(report.latest_timestamp_ms, Some(MAX_FILES as u64 + 1));
        assert!(report.truncation_reasons.iter().any(|r| r == "file_limit"));
    }
    #[test]
    fn repeated_reads_require_the_same_range_stream_and_run() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs/1/run");
        fs::create_dir_all(&dir).unwrap();
        let event = |offset, stream| {
            json!({"event":"tool_finished","details":{
            "name":"inspect_result","reply_bytes":4,
            "arguments":{"reference":"ref","offset":offset,"limit":4,"stream":stream}}})
        };
        fs::write(
            dir.join("events.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                event(0, "stdout"),
                event(0, "stdout"),
                event(4, "stdout"),
                event(0, "stderr")
            ),
        )
        .unwrap();
        let report = scan(temp.path(), &[(1, "Chat".into())], &[]);
        assert_eq!(report.repeated_reads, 1);
        assert_eq!(report.inspected_calls, 4);
        assert_eq!(report.read_reply_bytes, 16);
    }
    #[test]
    fn ranks_payloads_keeps_unknowns_and_does_not_add_cache_again() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs/1/run");
        fs::create_dir_all(&dir).unwrap();
        let a = json!({"event":"tool_finished","details":{"name":"read_file","reply_bytes":20,"arguments":{"path":"old.txt"}}});
        let b = json!({"event":"tool_finished","details":{"name":"run_command","arguments":{"program":"cargo"},"payload_metrics":{"version":1,"argument_bytes":400,"reply_bytes":8,"estimated_argument_tokens":100,"estimated_reply_tokens":2}}});
        fs::write(dir.join("events.jsonl"), format!("{a}\n{b}\nbroken\n")).unwrap();
        let record = UsageRecord {
            request_id: "r".into(),
            model: "fixture".into(),
            provider: Provider::Codex,
            usage: TokenUsage {
                input: Some(100),
                output: Some(20),
                cache_read: Some(80),
                reasoning: Some(10),
                ..Default::default()
            },
            context_input: None,
            context_window: None,
        };
        let report = scan(
            temp.path(),
            &[(1, "Chat".into()), (2, "Empty".into())],
            &[(1, record.clone()), (1, record)],
        );
        assert_eq!(report.groups[0].label, "run_command · cargo");
        assert_eq!(report.groups[0].estimated_tokens, 102);
        assert_eq!(report.calls[1].argument_bytes, None);
        assert_eq!(report.legacy_calls, 1);
        assert_eq!(report.invalid_lines, 1);
        assert_eq!(report.missing_chats, 1);
        assert_eq!(report.models[0].requests, 1);
        assert_eq!(report.models[0].usage.input, Some(100));
        assert_eq!(report.models[0].usage.output, Some(20));
        let export = save(temp.path(), "project", "project", &report).unwrap();
        assert!(export.exists());
        let other = scan(temp.path(), &[(2, "Empty".into())], &[]);
        assert!(other.calls.is_empty());
    }
    #[test]
    fn process_inspections_are_attributed_once_and_scoped_to_the_chat() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs/1/run");
        fs::create_dir_all(&dir).unwrap();
        let source = json!({"event":"tool_finished","details":{"name":"run_command","arguments":{"program":"cargo"},"payload_metrics":crate::diagnostics::payload_metrics(&json!({"program":"cargo"}),r#"{"ok":true,"ref":"reference"}"#)}});
        let read = json!({"event":"tool_finished","details":{"name":"inspect_result","arguments":{"reference":"reference"},"payload_metrics":crate::diagnostics::payload_metrics(&json!({"reference":"reference"}), &"x".repeat(400))}});
        fs::write(dir.join("events.jsonl"), format!("{source}\n{read}\n")).unwrap();
        let other = temp.path().join("logs/2/run");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("events.jsonl"), format!("{read}\n")).unwrap();
        let report = scan(temp.path(), &[(1, "One".into()), (2, "Two".into())], &[]);
        assert_eq!(report.calls.len(), 3);
        assert_eq!(report.processes.len(), 1);
        assert_eq!(report.processes[0].calls, 2);
        assert_eq!(
            report.processes[0].estimated_tokens,
            report
                .calls
                .iter()
                .filter(|c| c.chat == 1)
                .map(|c| c.estimated_tokens)
                .sum::<u64>()
        );
        assert!(
            report
                .calls
                .iter()
                .find(|c| c.chat == 2)
                .unwrap()
                .process
                .is_empty()
        );
        fs::write(
            dir.join("events.jsonl"),
            vec![b'x'; MAX_LINE as usize + 100],
        )
        .unwrap();
        let capped = scan(temp.path(), &[(1, "One".into())], &[]);
        assert!(capped.partial);
        assert!(capped.calls.is_empty());
    }
    #[test]
    fn partial_lines_are_not_counted_and_payloads_measure_unicode_without_content() {
        let metrics = crate::diagnostics::payload_metrics(&json!({"content":"á😀"}), "é😀");
        assert_eq!(metrics["reply_bytes"], 6);
        assert_eq!(metrics["reply_chars"], 2);
        assert!(!metrics.to_string().contains("á😀"));
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs/1/run");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), "{\"event\":\"tool_finished\"").unwrap();
        let report = scan(temp.path(), &[(1, "Chat".into())], &[]);
        assert!(report.partial);
        assert!(report.calls.is_empty());
        let unknown = UsageRecord {
            request_id: "r".into(),
            model: "m".into(),
            provider: Provider::Claude,
            usage: TokenUsage::default(),
            context_input: None,
            context_window: None,
        };
        let groups = models(&[(1, unknown)]);
        assert_eq!(groups[0].usage.input, None);
        assert!(groups[0].incomplete);
    }
}
