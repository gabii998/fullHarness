use ascurra_harness::{
    domain::{AppState, Message, Provider, RunState},
    runtime::{Event, Supervisor},
    settings::AgentSettings,
};
use std::{path::Path, sync::Arc, time::Duration};

fn fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join(if cfg!(windows) {
        "claude-fixture.exe"
    } else {
        "claude-fixture"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/claude_fixture.rs");
    let rustc = std::env::var_os("RUSTC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("CARGO_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(
                        std::env::var_os("USERPROFILE")
                            .or_else(|| std::env::var_os("HOME"))
                            .unwrap(),
                    )
                    .join(".cargo")
                })
                .join(if cfg!(windows) {
                    "bin/rustc.exe"
                } else {
                    "bin/rustc"
                })
        });
    let output = std::process::Command::new(rustc)
        .arg(source)
        .args(["--edition", "2024", "-o"])
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}
#[test]
fn native_adapter_approvals_resume_isolation_usage_and_cancellation() {
    let temp = tempfile::tempdir().unwrap();
    let binary = fixture(temp.path());
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let supervisor = Supervisor::new(&data).unwrap();
    let mut app = AppState::default();
    let config = AgentSettings {
        enabled: true,
        executable: binary.to_string_lossy().into_owned(),
        concurrency: 2,
        ..Default::default()
    };
    let quota = supervisor
        .executor
        .block_on(ascurra_harness::claude::account_usage(config.clone()))
        .unwrap();
    assert_eq!(quota.windows.len(), 2);
    assert_eq!(quota.windows[0].remaining_percent, 75.0);
    assert_eq!(quota.windows[1].remaining_percent, 20.0);
    let mut sessions: Vec<(u64, String)> = Vec::new();
    for (index, prompt, approve) in [
        (0, "write", true),
        (1, "deny", false),
        (0, "resume", true),
        (2, "cancel", false),
    ] {
        let repo = temp.path().join(format!("repo-{index}"));
        std::fs::create_dir_all(&repo).unwrap();
        let id = if index == 0 && prompt == "resume" {
            sessions[0].0
        } else {
            app.add_project(format!("repo-{index}"), repo.to_string_lossy().into_owned());
            app.selected_chat.unwrap()
        };
        let chat = app.chat_mut(id).unwrap();
        chat.provider = Provider::Claude;
        chat.project_context = "Project fixture instructions".into();
        chat.messages.push(Message {
            role: "Vos".into(),
            text: prompt.into(),
        });
        if prompt == "write" {
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(8, 6, image::Rgba([1, 2, 3, 255]))
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let image = ascurra_harness::attachments::save(&data, id, "fixture.png", png.get_ref())
                .unwrap();
            chat.message_images
                .insert(chat.messages.len() - 1, vec![image]);
        }
        let (sender, receive) = std::sync::mpsc::channel();
        supervisor
            .start_claude(
                chat.clone(),
                repo.to_string_lossy().into_owned(),
                data.clone(),
                config.clone(),
                Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
            )
            .unwrap();
        let mut approvals = 0;
        let mut tools = 0;
        let mut answer = String::new();
        let mut aggregate = false;
        loop {
            let event = receive
                .recv_timeout(Duration::from_secs(20))
                .expect("Native fixture stalled");
            match event {
                Event::Session { session_id, .. } => {
                    if prompt == "resume" {
                        assert_eq!(session_id, sessions[0].1);
                    } else {
                        assert!(!sessions.iter().any(|(_, previous)| previous == &session_id));
                        sessions.push((id, session_id.clone()));
                    }
                    app.chat_mut(id).unwrap().provider_session_id = Some(session_id);
                    if prompt == "cancel" {
                        supervisor.cancel(id);
                    }
                }
                Event::Approval { .. } => {
                    approvals += 1;
                    supervisor.approve(id, approve);
                }
                Event::ToolResult { summary, .. } => {
                    tools += 1;
                    assert!(summary.contains("fixture.txt"));
                }
                Event::ResponseText { text, .. } => answer = text,
                Event::Usage { record, .. } => {
                    app.chat_mut(id).unwrap().record_usage(record);
                }
                Event::TurnUsage {
                    request_ids,
                    record,
                    ..
                } => {
                    assert_eq!(record.usage.input, Some(60));
                    assert_eq!(record.context_input, Some(60));
                    assert_eq!(record.context_window, Some(200000));
                    let chat = app.chat_mut(id).unwrap();
                    chat.usage.retain(|u| !request_ids.contains(&u.request_id));
                    chat.record_usage(record);
                    aggregate = true;
                }
                Event::Finished { state, message, .. } => {
                    assert_eq!(
                        state,
                        if prompt == "cancel" {
                            RunState::Cancelled
                        } else {
                            RunState::Succeeded
                        },
                        "{message}"
                    );
                    break;
                }
                _ => {}
            }
        }
        if prompt != "cancel" {
            assert!(aggregate);
            assert_eq!(approvals, 1);
            assert_eq!(tools, if approve { 2 } else { 1 });
            assert_eq!(
                answer,
                if prompt == "resume" {
                    "Reanudado"
                } else {
                    "Respuesta final"
                }
            );
            assert_eq!(
                app.chat(id).unwrap().usage.len(),
                if prompt == "resume" { 2 } else { 1 }
            );
        }
        assert!(!supervisor.running());
    }
}
