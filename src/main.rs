use ascurra_harness::{
    codex::ModelChoice,
    domain::{AppState, Message, Provider, RunState},
    runtime::{Event, Supervisor},
    settings::{self, AgentSettings},
    storage::Store,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

slint::include_modules!();
mod attachments_ui;
mod captures_ui;
mod docker_ui;
mod explorer_ui;
mod search_ui;
mod window_ui;
mod scroll_profile;
mod git_ui;
mod http_ui;
mod lint_ui;
mod logcat_ui;
mod markdown_ui;
mod prisma_ui;
mod project_ui;
mod queue_ui;
mod tasks_ui;
mod terminal_ui;
mod usage_ui;

fn refresh(ui: &AppWindow, state: &AppState) {
    attachments_ui::display(ui, state);
    ui.set_codex_configured(
        ui.get_codex_authenticated()
            && state.settings.codex.enabled
            && !state.settings.codex.model.is_empty(),
    );
    let agents = available_providers(ui, state);
    let selected_provider = state
        .selected_chat
        .and_then(|id| state.chat(id))
        .map(|c| c.provider)
        .unwrap_or(Provider::Codex);
    let mut labels: Vec<slint::SharedString> =
        agents.iter().map(|p| provider_name(*p).into()).collect();
    if labels.is_empty() {
        labels.push("Sin agentes".into());
    }
    use slint::Model;
    if ui.get_available_agents().iter().collect::<Vec<_>>() != labels {
        ui.set_available_agents(ModelRc::new(VecModel::from(labels)));
    }
    ui.set_provider_index(
        agents
            .iter()
            .position(|p| *p == selected_provider)
            .map(|i| i as i32)
            .unwrap_or(-1),
    );
    ui.set_selected_agent_configured(agents.contains(&selected_provider));
    let quota_key = state
        .selected_chat
        .map(|id| {
            format!(
                "{id}:{}:{}:{}",
                provider_name(selected_provider),
                ui.get_codex_authenticated(),
                ui.get_claude_authenticated()
            )
        })
        .unwrap_or_default();
    if ui.get_quota_chat_key().as_str() != quota_key {
        ui.set_quota_chat_key(quota_key.into());
        ui.set_quota_provider(
            if ui.get_claude_authenticated()
                && (selected_provider == Provider::Claude || !ui.get_codex_authenticated())
            {
                1
            } else {
                0
            },
        );
    }
    ui.set_provider_editable(
        state
            .selected_chat
            .and_then(|id| state.chat(id))
            .is_some_and(|c| c.messages.is_empty())
            && !agents.is_empty(),
    );
    let efforts = chat_efforts(state, selected_provider);
    let effort_labels = efforts
        .iter()
        .map(|e| slint::SharedString::from(if e.is_empty() { "Effort: auto" } else { e }))
        .collect::<Vec<_>>();
    if ui.get_chat_efforts().iter().collect::<Vec<_>>() != effort_labels {
        ui.set_chat_efforts(ModelRc::new(VecModel::from(effort_labels)));
    }
    let mut navigation = Vec::new();
    for project in &state.projects {
        navigation.push(NavigationRow {
            id: format!("p{}", project.id).into(),
            title: project.name.clone().into(),
            project: true,
            selected: state.selected_project == Some(project.id),
            status: "".into(),
        });
        for chat in &project.chats {
            navigation.push(NavigationRow {
                id: chat.id.to_string().into(),
                title: chat.title.clone().into(),
                project: false,
                selected: state.selected_chat == Some(chat.id),
                status: if chat.state == RunState::Running {
                    "running"
                } else {
                    "idle"
                }
                .into(),
            });
        }
    }
    ui.set_navigation(ModelRc::new(VecModel::from(navigation)));
    let mut tabs = state
        .open_tabs
        .iter()
        .filter_map(|id| state.chat(*id))
        .map(|chat| TabRow {
            id: chat.id.to_string().into(),
            title: chat.title.clone().into(),
            selected: state.selected_chat == Some(chat.id),
        })
        .collect::<Vec<_>>();
    tabs.extend(
        state
            .open_project_tabs
            .iter()
            .filter_map(|id| state.projects.iter().find(|p| p.id == *id))
            .map(|project| TabRow {
                id: format!("p{}", project.id).into(),
                title: project.name.clone().into(),
                selected: state.selected_project == Some(project.id),
            }),
    );
    ui.set_tabs(ModelRc::new(VecModel::from(tabs)));
    let selected = state.selected_chat.and_then(|id| state.chat(id));
    let project = state.projects.iter().find(|project| {
        state.selected_project == Some(project.id)
            || project
                .chats
                .iter()
                .any(|chat| Some(chat.id) == state.selected_chat)
    });
    let overview = state.selected_project.is_some() && project.is_some();
    ui.set_has_project(project.is_some());
    ui.set_usage_selected_chat(
        state
            .selected_chat
            .map(|id| id.to_string())
            .unwrap_or_default()
            .into(),
    );
    ui.set_project_overview(overview);
    tasks_ui::display(ui, state);
    ui.set_overview_name(project.map(|p| p.name.as_str()).unwrap_or("").into());
    ui.set_overview_description(
        project
            .map(|p| p.profile.description.as_str())
            .unwrap_or("")
            .into(),
    );
    ui.set_overview_path(
        project
            .map(|p| p.path.strip_prefix(r"\\?\").unwrap_or(&p.path))
            .unwrap_or("")
            .into(),
    );
    let in_progress = |state| {
        matches!(
            state,
            RunState::Running | RunState::WaitingForPermission | RunState::WaitingForInput
        )
    };
    ui.set_project_chat_count(project.map(|p| p.chats.len()).unwrap_or(0) as i32);
    ui.set_project_running_count(
        project
            .map(|p| p.chats.iter().filter(|c| in_progress(c.state)).count())
            .unwrap_or(0) as i32,
    );
    ui.set_project_chats(ModelRc::new(VecModel::from(
        project
            .filter(|_| overview)
            .map(|p| {
                p.chats
                    .iter()
                    .rev()
                    .map(|chat| ProjectChatRow {
                        id: chat.id.to_string().into(),
                        title: chat.title.clone().into(),
                        detail: format!(
                            "{}{}",
                            provider_name(chat.provider),
                            if chat.model.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", chat.model)
                            }
                        )
                        .into(),
                        status: match chat.state {
                            RunState::Running => "En curso",
                            RunState::WaitingForPermission => "Espera aprobación",
                            RunState::WaitingForInput => "Espera respuesta",
                            RunState::Succeeded => "Completado",
                            RunState::Failed => "Error",
                            RunState::Cancelled => "Cancelado",
                            RunState::TimedOut => "Tiempo agotado",
                            RunState::Interrupted => "Interrumpido",
                            RunState::Idle => "Disponible",
                        }
                        .into(),
                        running: in_progress(chat.state),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    )));
    ui.set_git_project(project.map(|p| p.path.as_str()).unwrap_or("").into());
    ui.set_title_project_path(window_ui::display_path(project.map(|p| p.path.as_str()).unwrap_or("" )).into());
    let choices = chat_models(state);
    let model = selected
        .filter(|chat| !chat.model.is_empty())
        .map(|chat| chat.model.as_str())
        .unwrap_or(&state.settings.agent(selected_provider).model);
    let labels = choices
        .iter()
        .map(|choice| slint::SharedString::from(choice.label.as_str()))
        .collect::<Vec<_>>();
    if ui.get_chat_models().iter().collect::<Vec<_>>() != labels {
        ui.set_chat_models(ModelRc::new(VecModel::from(labels)));
    }
    ui.set_chat_model_index(
        choices
            .iter()
            .position(|choice| choice.slug == model)
            .unwrap_or(0) as i32,
    );
    let effort = selected
        .and_then(|chat| chat.reasoning_effort.as_deref())
        .unwrap_or(&state.settings.agent(selected_provider).reasoning_effort);
    let effort = if selected_provider == Provider::Codex {
        settings::codex_effort(model, effort)
    } else {
        effort
    };
    ui.set_chat_effort_index(
        efforts
            .iter()
            .position(|value| *value == effort)
            .unwrap_or(0) as i32,
    );
    ui.set_has_chat(selected.is_some());
    ui.set_can_retry_message(selected.is_some_and(|chat| {
        chat.state == RunState::Failed && chat.messages.iter().any(|m| m.role == "Vos")
    }));
    let (context_label, context_fraction) = context_meter(selected, model, &choices);
    ui.set_context_summary(context_label.into());
    ui.set_context_fraction(context_fraction);
    ui.set_chat_running(
        selected
            .is_some_and(|c| matches!(c.state, RunState::Running | RunState::WaitingForPermission)),
    );
    ui.set_approval_description(selected.map(|c| c.approval.as_str()).unwrap_or("").into());
    ui.set_auto_approve_enabled(state.selected_chat.is_some_and(|id|
        ui.get_auto_approve_chats().iter().any(|value| value.as_str() == id.to_string())));
    ui.set_token_summary(
        selected
            .map(token_summary)
            .unwrap_or_else(|| "Tokens: sin datos".into())
            .into(),
    );
    if let Some(chat) = selected {
        ui.set_draft(chat.draft.clone().into());
        ui.set_chat_title(chat.title.clone().into());
        let project = state
            .projects
            .iter()
            .find(|p| p.chats.iter().any(|c| c.id == chat.id))
            .unwrap();
        let display_path = project.path.strip_prefix(r"\\?\").unwrap_or(&project.path);
        ui.set_project_path(format!("{}  ›  {}", project.name, display_path).into());
        if matches!(chat.state, RunState::Running)
            && (chat.last_status.starts_with("Codex conectado directamente")
                || chat.last_status == "Acción aprobada automáticamente para este chat.")
        {
            ui.set_notice("".into());
        } else if !chat.last_status.is_empty() {
            ui.set_notice(chat.last_status.clone().into());
        }
    } else {
        ui.set_draft("".into());
        ui.set_chat_title("Tu espacio de trabajo".into());
        ui.set_project_path("Abrí un proyecto para empezar".into());
    }
    follow_chat_update(ui);
    let cached_messages = ui.get_messages().iter().collect::<Vec<_>>();
    let messages = selected
        .map(|chat| {
            chat.messages
                .iter()
                .enumerate()
                .map(|(index, m)| {
                    let mut body = display_message(m, &chat.wire_history);
                    if let Some(images) = chat.message_images.get(&index) {
                        if !body.is_empty() {
                            body.push_str("\n\n");
                        }
                        body.push_str(&format!(
                            "Imágenes adjuntas: {}",
                            images
                                .iter()
                                .map(|i| i.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    let role = if m.role.starts_with("Herramienta") {
                        "Herramienta"
                    } else {
                        &m.role
                    };
                    let blocks = cached_messages
                        .get(index)
                        .filter(|row| row.body == body.as_str() && row.role == role)
                        .map(|row| row.blocks.clone())
                        .unwrap_or_else(|| markdown_ui::blocks(role, &body));
                    MessageRow {
                        blocks,
                        role: role.into(),
                        summary: body.lines().next().unwrap_or_default().into(), body: body.into(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Preserve row instances and scroll geometry while streaming. Only notify
    // the repeater about rows whose text changed, rather than replacing it.
    let current = ui.get_messages();
    if let Some(rows) = current.as_any().downcast_ref::<VecModel<MessageRow>>() {
        while rows.row_count() > messages.len() {
            rows.remove(rows.row_count() - 1);
        }
        for (index, row) in messages.into_iter().enumerate() {
            let is_current_chat = state.selected_chat.is_some_and(|id| ui.get_displayed_chat_key().as_str() == id.to_string());
            if is_current_chat && !row.body.is_empty()
                && cached_messages.get(index).is_none_or(|old| old.body.is_empty())
            {
                animate_chat_entry(ui, index);
            }
            if index >= rows.row_count() {
                rows.push(row);
            } else if cached_messages
                .get(index)
                .is_none_or(|old| old.body != row.body || old.role != row.role)
            {
                rows.set_row_data(index, row);
            }
        }
    } else {
        ui.set_messages(ModelRc::new(VecModel::from(messages)));
    }
    let key = state.selected_chat.map(|id| id.to_string()).unwrap_or_default();
    if ui.get_displayed_chat_key().as_str() != key {
        ui.set_chat_entry_animate(false);
        ui.set_chat_entry_progress(1.0);
        ui.set_chat_entry_index(-1);
        ui.set_displayed_chat_key(key.into());
        scroll_opened_chat(ui);
    }}

// Capture proximity before changing rows, including the final (non-running) message.
fn follow_chat_update(ui: &AppWindow) {
    if !ui.get_has_chat() || !ui.get_chat_content_visible() || ui.get_chat_follow_pending()
        || (ui.get_chat_scroll_offset() - ui.get_chat_scroll_bottom()).abs() > 80.0
    { return; }
    start_chat_scroll(ui, false);
}

fn start_chat_scroll(ui: &AppWindow, opening: bool) {
    let generation = ui.get_chat_scroll_generation().wrapping_add(1);
    ui.set_chat_scroll_generation(generation);
    ui.set_chat_opening_scroll(opening);
    ui.set_chat_follow_pending(true);
    settle_chat_scroll(ui.as_weak(), ui.get_displayed_chat_key(), generation, 0, 0, f32::NAN);
}

// A virtualized list discovers row heights over several layout passes. Keep
// following the actual bottom until it settles; never mutate scroll from layout.
fn settle_chat_scroll(
    weak: slint::Weak<AppWindow>, key: slint::SharedString, generation: i32,
    passes: u32, stable: u32, previous_bottom: f32,
) {
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
        let Some(ui) = weak.upgrade() else { return; };
        // A timer from a previous tab must not clear a newer tab's pending state.
        if ui.get_chat_scroll_generation() != generation { return; }
        if ui.get_displayed_chat_key() != key || !ui.get_has_chat() || !ui.get_chat_content_visible() {
            ui.set_chat_follow_pending(false);
            ui.set_chat_opening_scroll(false);
            return;
        }
        let bottom = ui.get_chat_scroll_bottom();
        let stable = if (bottom - previous_bottom).abs() < 0.5
            && (ui.get_chat_scroll_offset() - bottom).abs() < 1.0 { stable + 1 } else { 0 };
        ui.invoke_scroll_chat_to_end();
        if passes >= 59 || (passes >= 11 && stable >= 4) {
            ui.set_chat_follow_pending(false);
            ui.set_chat_opening_scroll(false);
        } else {
            settle_chat_scroll(weak, key, generation, passes + 1, stable, bottom);
        }
    });
}

fn animate_chat_entry(ui: &AppWindow, index: usize) {
    let generation = ui.get_chat_entry_generation().wrapping_add(1);
    ui.set_chat_entry_generation(generation);
    ui.set_chat_entry_animate(false);
    ui.set_chat_entry_progress(0.0);
    ui.set_chat_entry_index(index.min(i32::MAX as usize) as i32);
    let key = ui.get_displayed_chat_key();
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_chat_entry_generation() == generation && ui.get_displayed_chat_key() == key {
                ui.set_chat_entry_animate(true);
                ui.set_chat_entry_progress(1.0);
            }
        }
    });
}
fn scroll_opened_chat(ui: &AppWindow) {
    if !ui.get_has_chat() || !ui.get_chat_content_visible() { return; }
    start_chat_scroll(ui, true);
}
// Text deltas only touch the last row. Navigation and model selectors are stable
// for the duration of a stream; avoid walking the transcript on every fragment.
fn refresh_stream_tail(ui: &AppWindow, state: &AppState) {
    use slint::Model;
    let Some(chat) = state.selected_chat.and_then(|id| state.chat(id)) else {
        return;
    };
    let Some(message) = chat.messages.last() else {
        return;
    };
    follow_chat_update(ui);
    let current = ui.get_messages();
    let Some(rows) = current.as_any().downcast_ref::<VecModel<MessageRow>>() else {
        return;
    };
    let index = chat.messages.len() - 1;
    let body = display_message(message, &chat.wire_history);
    if rows
        .row_data(index)
        .is_some_and(|row| row.body == body.as_str())
    {
        return;
    }
    if !body.is_empty() && rows.row_data(index).is_none_or(|old| old.body.is_empty()) {
        animate_chat_entry(ui, index);
    }
    let row = MessageRow {
        role: message.role.as_str().into(),
        blocks: markdown_ui::blocks(&message.role, &body),
        summary: body.lines().next().unwrap_or_default().into(), body: body.into(),
    };
    if index < rows.row_count() {
        rows.set_row_data(index, row);
    } else if index == rows.row_count() {
        rows.push(row);
    }
}

fn display_message(message: &Message, history: &[serde_json::Value]) -> String {
    // Existing tool messages stored raw JSON. Render a summary without changing
    // the preserved provider history or copying that detail into new context.
    if message.role.starts_with("Herramienta") {
        if let Some((name, reply)) = message
            .role
            .split_once(": ")
            .map(|(_, name)| (name, message.text.as_str()))
            .or_else(|| message.text.split_once(": "))
        {
            if serde_json::from_str::<serde_json::Value>(reply).is_ok() {
                let arguments = history
                    .iter()
                    .find(|item| {
                        item["type"] == "function_call_output"
                            && item["output"].as_str() == Some(reply)
                    })
                    .and_then(|output| {
                        history.iter().find(|call| {
                            call["type"] == "function_call" && call["call_id"] == output["call_id"]
                        })
                    })
                    .and_then(|call| call["arguments"].as_str())
                    .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
                    .unwrap_or_default();
                let summary = ascurra_harness::diagnostics::tool_summary(name, &arguments, reply, None);
                return ascurra_harness::diagnostics::tool_display_summary(name, &arguments, &summary);
            }
        }
    }
    message.text.clone()
}

fn render_message(role: &str, text: &str) -> slint::StyledText {
    if text.is_empty() {
        return slint::StyledText::from_plain_text("Pensando…");
    }
    if role != "Codex" && role != "Claude" {
        return slint::StyledText::from_plain_text(text);
    }
    // Slint's native CommonMark subset renders bold, italics, lists and inline
    // code. Convert headings and fenced code, which it does not render natively.
    let mut normalized = String::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let first = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed.chars().take_while(|c| *c == first).count();
        if (first == '`' || first == '~') && count >= 3 {
            if fence.is_none() {
                fence = Some((first, count));
                continue;
            }
            if fence.is_some_and(|(mark, len)| mark == first && count >= len) {
                fence = None;
                normalized.push('\n');
                continue;
            }
        }
        if fence.is_some() {
            let longest = line.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let delimiter = "`".repeat(longest + 1);
            normalized.push_str(&format!("{delimiter} {line} {delimiter}  \n"));
        } else if trimmed.starts_with('#')
            && count <= 6
            && trimmed.as_bytes().get(count) == Some(&b' ')
        {
            normalized.push_str(&format!("**{}**\n\n", &trimmed[count + 1..]));
        } else {
            normalized.push_str(line);
            normalized.push('\n');
        }
    }
    slint::StyledText::from_markdown(&normalized)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

fn provider_name(provider: Provider) -> &'static str {
    if provider == Provider::Codex {
        "Codex"
    } else {
        "Claude"
    }
}
fn provider_efforts(provider: Provider) -> &'static [&'static str] {
    if provider == Provider::Codex {
        settings::REASONING_EFFORTS
    } else {
        ascurra_harness::claude::EFFORTS
    }
}
fn chat_efforts(state: &AppState, provider: Provider) -> &'static [&'static str] {
    if provider == Provider::Codex {
        let model = state
            .selected_chat
            .and_then(|id| state.chat(id))
            .filter(|chat| !chat.model.is_empty())
            .map(|chat| chat.model.as_str())
            .unwrap_or(&state.settings.codex.model);
        settings::codex_efforts(model)
    } else {
        provider_efforts(provider)
    }
}
fn available_providers(ui: &AppWindow, state: &AppState) -> Vec<Provider> {
    let mut result = Vec::new();
    if ui.get_codex_configured() {
        result.push(Provider::Codex);
    }
    if state.settings.claude.enabled
        && ui.get_claude_authenticated()
        && ui.get_claude_verified_path().as_str() == state.settings.claude.executable
    {
        result.push(Provider::Claude);
    }
    result
}
fn chat_models(state: &AppState) -> Vec<ModelChoice> {
    if let Some(chat) = state
        .selected_chat
        .and_then(|id| state.chat(id))
        .filter(|c| c.provider == Provider::Claude)
    {
        let mut choices: Vec<ModelChoice> = [
            ("", "Predeterminado"),
            ("sonnet", "Sonnet"),
            ("opus", "Opus"),
            ("haiku", "Haiku"),
        ]
        .iter()
        .map(|(slug, label)| ModelChoice {
            slug: (*slug).into(),
            label: (*label).into(),
            ..Default::default()
        })
        .collect();
        let model = if chat.model.is_empty() {
            &state.settings.claude.model
        } else {
            &chat.model
        };
        if !choices.iter().any(|m| m.slug == *model) {
            choices.push(ModelChoice {
                slug: model.clone(),
                label: model.clone(),
                ..Default::default()
            });
        }
        return choices;
    }
    if !state.settings.codex.models.is_empty() {
        return state.settings.codex.models.clone();
    }
    if !state.settings.codex.model.is_empty() {
        return vec![ModelChoice {
            slug: state.settings.codex.model.clone(),
            label: state.settings.codex.model.clone(),
            ..Default::default()
        }];
    }
    vec![ModelChoice {
        slug: String::new(),
        label: "Sin modelo".into(),
        ..Default::default()
    }]
}

/// A provider measurement of the last request, never cumulative spending or a
/// byte-based estimate of encrypted history. New output/draft is not counted.
fn context_meter(
    chat: Option<&ascurra_harness::domain::Chat>,
    model: &str,
    choices: &[ModelChoice],
) -> (String, f32) {
    let capacity = choices
        .iter()
        .find(|choice| choice.slug == model)
        .and_then(|choice| choice.context_window)
        .filter(|n| *n > 0);
    let record = chat.and_then(|chat| chat.usage.last()).filter(|record| {
        chat.is_some_and(|c| c.provider == record.provider)
            && (record.model == model
                || (record.provider == Provider::Claude
                    && (model.is_empty()
                        || (["sonnet", "opus", "haiku"].contains(&model)
                            && record.model.contains(model)))))
    });
    let capacity = record
        .and_then(|r| r.context_window)
        .filter(|n| *n > 0)
        .or(capacity);
    match (
        record.and_then(|record| {
            if record.provider == Provider::Claude {
                record.context_input
            } else {
                record.usage.input
            }
        }),
        capacity,
    ) {
        (Some(input), Some(capacity)) => {
            let fraction = input as f64 / capacity as f64;
            (
                format!("Ctx: {} / {} ({:.1}%)", abbreviated_tokens(input), abbreviated_tokens(capacity), fraction * 100.0),
                fraction.clamp(0.0, 1.0) as f32,
            )
        }
        (Some(input), None) => (
            format!("Ctx: {} / ?", abbreviated_tokens(input)),
            0.0,
        ),
        (None, Some(capacity)) => (
            format!("Ctx: sin medición / {}", abbreviated_tokens(capacity)),
            0.0,
        ),
        (None, None) => ("Ctx: sin medición".into(), 0.0),
    }
}

fn abbreviated_tokens(value: u64) -> String {
    if value < 1_000 { return value.to_string(); }
    let (divisor, suffix) = if value >= 999_950 { (1_000_000.0, "M") } else { (1_000.0, "k") };
    let number = format!("{:.1}", value as f64 / divisor);
    format!("{}{}", number.trim_end_matches(".0").replace('.', ","), suffix)
}
fn token_summary(chat: &ascurra_harness::domain::Chat) -> String {
    if chat.usage.is_empty() {
        return if chat.usage_incomplete {
            "Tokens: consumo sin confirmar"
        } else {
            "Tokens: sin datos"
        }
        .into();
    }
    let sum = |extract: fn(&ascurra_harness::domain::TokenUsage) -> Option<u64>| {
        let values = chat
            .usage
            .iter()
            .map(|r| extract(&r.usage))
            .collect::<Vec<_>>();
        if values.iter().all(Option::is_none) {
            "?".into()
        } else {
            format!(
                "{}{}",
                abbreviated_tokens(values.iter().flatten().fold(0u64, |sum, value| sum.saturating_add(*value))),
                if values.iter().any(Option::is_none) {
                    "+?"
                } else {
                    ""
                }
            )
        }
    };
    format!(
        "Entrada {} · Salida {} · Caché {}{}",
        sum(|u| u.input),
        sum(|u| u.output),
        sum(|u| u.cache_read),
        if chat.usage_incomplete {
            " · parcial"
        } else {
            ""
        }
    )
}

fn preserve_composer(ui: &AppWindow, state: &mut AppState) {
    if let Some(id) = state.selected_chat {
        if let Some(chat) = state.chat_mut(id) {
            chat.draft = ui.get_draft().to_string();
        }
    }
}

fn save_and_refresh(ui: &AppWindow, state: &mut AppState, store: &Store) {
    if let Some(id) = state.selected_chat {
        if let Err(error) = store.hydrate(state, id) {
            ui.set_notice(format!("No se pudo cargar el chat: {error}").into());
            return;
        }
    }
    match store.save(state) {
        Ok(()) => store.release_inactive(state),
        Err(error) => ui.set_notice(format!("No se pudo guardar: {error}").into()),
    }
    refresh(ui, state);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args
        .get(1)
        .is_some_and(|arg| arg == "--check-renderer" || arg == "--measure-renderer")
    {
        let measure = args[1] == "--measure-renderer";
        if args.iter().any(|arg| arg == "--desktop-gl") {
            slint::BackendSelector::new()
                .require_opengl_with_version(3, 3)
                .select()?;
        }
        let ui = AppWindow::new()?;
        let rendered = Rc::new(std::cell::Cell::new(false));
        let observed = rendered.clone();
        let notifier = ui.window().set_rendering_notifier(move |state, api| {
            if matches!(state, slint::RenderingState::RenderingSetup) {
                if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                    let address = get_proc_address(c"glGetString");
                    if !address.is_null() {
                        // The notifier runs with the GL context current. glGetString
                        // returns a driver-owned, NUL-terminated string for these enums.
                        let get_string: unsafe extern "system" fn(u32) -> *const std::ffi::c_char =
                            unsafe { std::mem::transmute(address) };
                        for (name, parameter) in [
                            ("vendor", 0x1F00),
                            ("renderer", 0x1F01),
                            ("version", 0x1F02),
                        ] {
                            let value = unsafe { get_string(parameter) };
                            if !value.is_null() {
                                println!(
                                    "OpenGL {name}: {}",
                                    unsafe { std::ffi::CStr::from_ptr(value) }.to_string_lossy()
                                );
                            }
                        }
                        let address = get_proc_address(c"glGetIntegerv");
                        if !address.is_null() {
                            let get_integer: unsafe extern "system" fn(u32, *mut i32) =
                                unsafe { std::mem::transmute(address) };
                            for (name, parameter) in [
                                ("depth_bits", 0x0D56),
                                ("stencil_bits", 0x0D57),
                                ("samples", 0x80A9),
                            ] {
                                let mut value = 0;
                                unsafe { get_integer(parameter, &mut value) };
                                println!("OpenGL {name}: {value}");
                            }
                        }
                    }
                }
            }
            if matches!(state, slint::RenderingState::AfterRendering)
                && matches!(api, slint::GraphicsAPI::NativeOpenGL { .. })
            {
                observed.set(true);
            }
        });
        if !measure {
            notifier
                .map_err(|error| format!("No se pudo verificar el renderizador GPU: {error:?}"))?;
        }
        slint::Timer::single_shot(
            std::time::Duration::from_millis(if measure { 2000 } else { 750 }),
            || {
                let _ = slint::quit_event_loop();
            },
        );
        ui.run()?;
        if !rendered.get() && !measure {
            return Err("No se observó ningún cuadro renderizado con OpenGL".into());
        }
        println!(
            "{}",
            if rendered.get() {
                "FemtoVG/OpenGL: cuadro renderizado correctamente"
            } else {
                "Renderizador software: ventana presentada"
            }
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|arg| arg == "--check-agent") {
        let provider = match args.get(2).map(String::as_str) {
            Some("codex") => Provider::Codex,
            Some("claude") => Provider::Claude,
            _ => return Err("Usá --check-agent codex o --check-agent claude".into()),
        };
        let path = settings::detect_client(provider).ok_or("No se encontró el cliente oficial.")?;
        let config = AgentSettings {
            executable: path.to_string_lossy().into_owned(),
            ..Default::default()
        };
        println!(
            "{}",
            if provider == Provider::Claude {
                ascurra_harness::claude::verify(&config)?
            } else {
                settings::check_client(provider, &config)?
            }
        );
        return Ok(());
    }
    let root = std::env::var_os("HARNESS_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("AscurraHarness"))
                .unwrap_or_else(|| PathBuf::from(".harness-data"))
        });
    if args.get(1).is_some_and(|arg| {
        [
            "--check-usage",
            "--connect-codex",
            "--check-models",
            "--check-codex",
        ]
        .contains(&arg.as_str())
    }) {
        let connection = ascurra_harness::codex::Connection::new(&root)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        if args[1] == "--connect-codex" {
            let (_cancel, receiver) = tokio::sync::watch::channel(false);
            println!(
                "{}",
                executor.block_on(connection.login(receiver, |notice| println!("{notice}")))?
            );
            println!(
                "Modelos disponibles: {}",
                executor.block_on(connection.models())?.len()
            );
        }
        if args[1] == "--check-models" || args[1] == "--check-codex" {
            let models = executor.block_on(connection.models())?;
            if args[1] == "--check-models" {
                println!("{}", serde_json::to_string(&models)?);
                return Ok(());
            }
            let model = models
                .iter()
                .find(|choice| choice.slug == "gpt-6-luna")
                .unwrap_or(&models[0]);
            let (_cancel, receiver) = tokio::sync::watch::channel(false);
            let body = serde_json::json!({"model":model.slug,"instructions":"Respondé solamente OK.",
                "input":[{"role":"user","content":"Verificar conexión."}],"store":false,"stream":true,
                "include":["reasoning.encrypted_content"],"tools":ascurra_harness::tool_runtime::schemas(),"reasoning":{"effort":"low"}});
            let result = executor
                .block_on(connection.generate(body, receiver, |_| {}))
                .map_err(|error| error.message)?;
            if !result.completed
                || ascurra_harness::codex::visible_text(
                    result.response["output"]
                        .as_array()
                        .ok_or("Salida inválida.")?,
                )
                .is_empty()
            {
                return Err("La prueba no produjo una respuesta visible completa.".into());
            }
            println!(
                "{}",
                serde_json::json!({"completed":true,"model":model.slug,"text":ascurra_harness::codex::visible_text(result.response["output"].as_array().ok_or("Salida inválida.")?),"usage":result.response["usage"]})
            );
        }
        let usage = executor.block_on(connection.account_usage())?;
        println!("{}", serde_json::to_string(&usage)?);
        return Ok(());
    }
    if args.get(1).is_some_and(|arg| arg == "--check-claude") {
        let repo = tempfile::tempdir()?;
        std::fs::write(repo.path().join("README.md"), "HARNESS_CLAUDE_OK")?;
        let data = root.join("smoke").join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&data)?;
        let service = Supervisor::new(&data)?;
        let config = AgentSettings {
            enabled: true,
            executable: settings::detect_client(Provider::Claude)
                .ok_or("Claude Code no instalado.")?
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        println!("{}", ascurra_harness::claude::verify(&config)?);
        let mut state = AppState::default();
        state.add_project(
            "Prueba Claude".into(),
            repo.path().to_string_lossy().into_owned(),
        );
        let id = state.selected_chat.unwrap();
        let chat = state.chat_mut(id).unwrap();
        chat.provider = Provider::Claude;
        chat.messages.push(Message { role:"Vos".into(), text:"Usa la herramienta MCP read_file para leer README.md (offset 0, limit 100). Responde solamente con el texto de ese archivo. No modifiques archivos ni ejecutes comandos.".into() });
        let (sender, receive) = std::sync::mpsc::channel();
        service.start_claude(
            chat.clone(),
            repo.path().to_string_lossy().into_owned(),
            data.clone(),
            config,
            Arc::new(move |event| {
                let _ = sender.send(event);
            }),
        )?;
        let mut calls = 0;
        let mut answer = String::new();
        let mut tokens = None;
        loop {
            match receive.recv_timeout(std::time::Duration::from_secs(200)) {
                Ok(Event::ToolResult { summary, .. }) => {
                    calls += 1;
                    println!("{summary}");
                }
                Ok(Event::Approval { .. }) => service.approve(id, false),
                Ok(Event::ResponseText { text, .. }) => answer = text,
                Ok(Event::TurnUsage { record, .. }) => tokens = Some(record),
                Ok(Event::Finished { state, message, .. }) => {
                    if state != RunState::Succeeded {
                        return Err(message.into());
                    }
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    service.cancel(id);
                    return Err("La prueba de Claude no termino a tiempo.".into());
                }
            }
        }
        if calls == 0 || !answer.contains("HARNESS_CLAUDE_OK") {
            return Err("Claude no completo la prueba de lectura MCP.".into());
        }
        println!(
            "{}",
            serde_json::json!({"completed":true,"tool_calls":calls,"text":answer,"usage":tokens,"logs":data})
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|arg| arg == "--check-claude-usage") {
        let config = AgentSettings {
            executable: settings::detect_client(Provider::Claude)
                .ok_or("Claude Code no instalado.")?
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        println!(
            "{}",
            serde_json::to_string(
                &runtime.block_on(ascurra_harness::claude::account_usage(config))?
            )?
        );
        return Ok(());
    }
    let store = Rc::new(Store::new(root)?);
    // A corrupt state is reported, never silently replaced with an empty one.
    let state = Rc::new(RefCell::new(store.load()?));
    let ui = build_app(store.clone(), state.clone())?;
    ui.run()?;
    preserve_composer(&ui, &mut state.borrow_mut());
    store.save(&state.borrow())?;
    Ok(())
}

fn build_app(
    store: Rc<Store>,
    state: Rc<RefCell<AppState>>,
) -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;
    let weak_scroll = ui.as_weak();
    ui.on_chat_scroll_requested(move || {
        if let Some(ui) = weak_scroll.upgrade() {
            scroll_opened_chat(&ui);
        }
    });
    // Table layouts need a completed frame before their offscreen height can
    // be cached. Coalesce requests; no timer runs once measurement settles.
    let pending_measure = Rc::new(std::cell::Cell::new(false));
    let weak_measure = ui.as_weak();
    ui.on_chat_measure_requested(move || {
        if pending_measure.replace(true) {
            return;
        }
        let pending = pending_measure.clone();
        let weak = weak_measure.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
            pending.set(false);
            if let Some(ui) = weak.upgrade() {
                ui.set_chat_measurement_epoch(ui.get_chat_measurement_epoch().wrapping_add(1));
            }
        });
    });
    let supervisor = Supervisor::new(store.root()).map_err(slint::PlatformError::Other)?;
    match supervisor.connection.has_session() {
        Ok(connected) => ui.set_codex_authenticated(connected),
        Err(error) => ui.set_notice(error.into()),
    }
    refresh(&ui, &state.borrow());
    configure_quota_callback(&ui, supervisor.clone());
    tasks_ui::configure(&ui, state.clone(), store.clone());
    project_ui::configure(&ui, state.clone(), store.clone());
    queue_ui::configure(&ui, state.clone(), store.clone(), supervisor.clone());
    git_ui::configure(&ui, supervisor.clone());
    prisma_ui::configure(&ui, supervisor.clone());
    docker_ui::configure(&ui, supervisor.clone());
    lint_ui::configure(&ui, supervisor.clone(), store.clone());
    terminal_ui::configure(&ui, supervisor.clone());
    usage_ui::configure(&ui, state.clone(), store.clone(), supervisor.clone());
    http_ui::configure(&ui, store.clone(), supervisor.clone());
    captures_ui::configure(&ui, state.clone(), store.clone(), supervisor.clone());
    attachments_ui::configure(&ui, state.clone(), store.clone(), supervisor.clone());
    let logcat = logcat_ui::configure(&ui, supervisor.clone());
    explorer_ui::configure(&ui, supervisor.clone());
    search_ui::configure(&ui);
    window_ui::configure(&ui, scroll_profile::configure(&ui, store.root()));
    configure_settings_callbacks(&ui, store.clone(), state.clone(), supervisor.clone());
    configure_runtime_callbacks(&ui, store.clone(), state.clone(), supervisor.clone());
    if ui.get_codex_authenticated() {
        ui.set_codex_model(state.borrow().settings.codex.model.clone().into());
        ui.invoke_check_agent(0, false);
    }
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_open_logs(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(id) = data.borrow().selected_chat else {
            return;
        };
        let path = disk.root().join("logs").join(id.to_string());
        if !path.is_dir() {
            ui.set_notice("Este chat todavía no tiene logs de ejecución.".into());
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if std::process::Command::new("explorer.exe")
                .arg(&path)
                .creation_flags(0x08000000)
                .spawn()
                .is_err()
            {
                ui.set_notice("No se pudo abrir la carpeta de logs.".into());
            }
        }
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_pick_chat_provider(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = data.borrow_mut();
        let Some(provider) = available_providers(&ui, &state)
            .get(index as usize)
            .copied()
        else {
            refresh(&ui, &state);
            return;
        };
        preserve_composer(&ui, &mut state);
        if let Some(chat) = state.selected_chat.and_then(|id| state.chat_mut(id)) {
            if chat.messages.is_empty()
                && !matches!(
                    chat.state,
                    RunState::Running | RunState::WaitingForPermission
                )
            {
                chat.provider = provider;
                chat.model.clear();
                chat.reasoning_effort = None;
            }
        }
        save_and_refresh(&ui, &mut state, &disk);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_pick_chat_model(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_selected_agent_configured() || ui.get_chat_running() {
            return;
        }
        let mut state = data.borrow_mut();
        preserve_composer(&ui, &mut state);
        let Some(model) = chat_models(&state).get(index as usize).cloned() else {
            return;
        };
        if let Some(id) = state.selected_chat {
            if let Some(chat) = state.chat_mut(id) {
                chat.model = model.slug;
            }
        }
        save_and_refresh(&ui, &mut state, &disk);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_pick_chat_effort(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_selected_agent_configured() || ui.get_chat_running() {
            return;
        }
        let provider = data
            .borrow()
            .selected_chat
            .and_then(|id| data.borrow().chat(id).map(|c| c.provider))
            .unwrap_or(Provider::Codex);
        let Some(effort) = chat_efforts(&data.borrow(), provider).get(index as usize) else {
            return;
        };
        let mut state = data.borrow_mut();
        preserve_composer(&ui, &mut state);
        if let Some(id) = state.selected_chat {
            if let Some(chat) = state.chat_mut(id) {
                chat.reasoning_effort = Some((*effort).into());
            }
        }
        save_and_refresh(&ui, &mut state, &disk);
    });
    let shutdown = supervisor.clone();
    let close_cleanup = Rc::new(move || {
        logcat.stop();
        shutdown.cancel_all();
    });
    let cleanup = close_cleanup.clone();
    ui.window().on_close_requested(move || {
        cleanup();
        slint::CloseRequestResponse::HideWindow
    });
    let weak = ui.as_weak();
    ui.on_window_close(move || {
        close_cleanup();
        if let Some(ui) = weak.upgrade() { let _ = ui.hide(); }
    });

    let weak = ui.as_weak();
    ui.on_browse_folder(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_folder_picker_open() {
            return;
        }
        ui.set_folder_picker_open(true);
        let initial_path = PathBuf::from(ui.get_new_path().as_str());
        let handle = ui.as_weak();
        // The native picker waits on its own thread so other chats remain responsive.
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Seleccionar carpeta del proyecto");
            if initial_path.is_dir() {
                dialog = dialog.set_directory(initial_path);
            }
            let selected = dialog.pick_folder();
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_folder_picker_open(false);
                if let Some(path) = selected {
                    if ui.get_project_name().trim().is_empty() {
                        if let Some(name) = path.file_name() {
                            ui.set_project_name(name.to_string_lossy().as_ref().into());
                        }
                    }
                    ui.set_new_path(path.to_string_lossy().as_ref().into());
                }
            });
        });
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_add_project(move |name, path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let candidate = PathBuf::from(path.trim());
        if !candidate.is_absolute() || !candidate.is_dir() {
            ui.set_notice(
                "La ruta debe ser absoluta y corresponder a un directorio existente.".into(),
            );
            return;
        }
        let Ok(canonical) = candidate.canonicalize() else {
            return;
        };
        let mut state = data.borrow_mut();
        if state
            .projects
            .iter()
            .any(|p| PathBuf::from(&p.path) == canonical)
        {
            ui.set_notice("Ese proyecto ya está abierto.".into());
            return;
        }
        preserve_composer(&ui, &mut state);
        state.add_project(name.trim().into(), canonical.to_string_lossy().into_owned());
        ui.set_draft("".into());
        ui.set_notice("Proyecto abierto. Configurá tu conexión de Codex para empezar.".into());
        save_and_refresh(&ui, &mut state, &disk);
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_select_row(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        ui.set_usage_open(false);
        preserve_composer(&ui, &mut state);
        ui.set_file_preview_open(false);
        if let Some(project_id) = id.strip_prefix('p').and_then(|s| s.parse::<u64>().ok()) {
            state.select_project(project_id);
            ui.set_draft("".into());
            save_and_refresh(&ui, &mut state, &disk);
            return;
        }
        let target = id.parse::<u64>().ok();
        if let Some(id) = target {
            if let Err(error) = disk.hydrate(&mut state, id) {
                ui.set_notice(format!("No se pudo cargar el chat: {error}").into());
                return;
            }
            state.select_chat(id);
        }
        ui.set_draft("".into());
        save_and_refresh(&ui, &mut state, &disk);
        scroll_opened_chat(&ui);
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_close_tab(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if let Some(project_id) = id.strip_prefix('p').and_then(|id| id.parse::<u64>().ok()) {
            let mut state = data.borrow_mut();
            preserve_composer(&ui, &mut state);
            state.close_project_tab(project_id);
            save_and_refresh(&ui, &mut state, &disk);
            return;
        }
        if let Ok(id) = id.parse() {
            let mut state = data.borrow_mut();
            preserve_composer(&ui, &mut state);
            if state.selected_chat == Some(id) {
                if let Some(next) = state.open_tabs.iter().rev().copied().find(|tab| *tab != id) {
                    if let Err(error) = disk.hydrate(&mut state, next) {
                        ui.set_notice(
                            format!("No se pudo cargar el siguiente chat: {error}").into(),
                        );
                        return;
                    }
                }
            }
            state.close_tab(id);
            ui.set_draft("".into());
            save_and_refresh(&ui, &mut state, &disk);
        }
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_activate_file_project(move |path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        let Some(project) = state.projects.iter().find(|p| p.path == path.as_str()) else {
            return;
        };
        if state.selected_project == Some(project.id) {
            return;
        }
        if project
            .chats
            .iter()
            .any(|chat| Some(chat.id) == state.selected_chat)
        {
            return;
        }
        let target = state
            .open_tabs
            .iter()
            .rev()
            .find(|id| project.chats.iter().any(|chat| chat.id == **id))
            .copied()
            .or_else(|| project.chats.first().map(|chat| chat.id));
        preserve_composer(&ui, &mut state);
        if let Some(id) = target {
            if let Err(error) = disk.hydrate(&mut state, id) {
                ui.set_notice(format!("No se pudo cargar el chat: {error}").into());
                return;
            }
            state.select_chat(id);
        }
        save_and_refresh(&ui, &mut state, &disk);
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_new_chat(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        preserve_composer(&ui, &mut state);
        ui.set_file_preview_open(false);
        let project_id = state
            .projects
            .iter()
            .find(|p| p.chats.iter().any(|c| Some(c.id) == state.selected_chat))
            .map(|p| p.id)
            .or(state.selected_project);
        if let Some(id) = project_id {
            state.add_chat(id);
        }
        ui.set_draft("".into());
        save_and_refresh(&ui, &mut state, &disk);
    });

    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    let supervisor = supervisor.clone();
    let submit = Rc::new(move |text: slint::SharedString, retry: bool| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_attachment_busy() {
            ui.set_notice("Esperá a que se guarden las imágenes adjuntas.".into());
            return;
        }
        let text = text.trim();
        let mut state = data.borrow_mut();
        let Some(id) = state.selected_chat else {
            return;
        };
        if let Err(error) = disk.hydrate(&mut state, id) {
            ui.set_notice(format!("No se pudo cargar el chat: {error}").into());
            return;
        }
        let Some(chat) = state.chat_mut(id) else {
            return;
        };
        if retry
            && (chat.state != RunState::Failed || !chat.messages.iter().any(|m| m.role == "Vos"))
        {
            return;
        }
        if !retry && text.is_empty() && chat.draft_images.is_empty() {
            return;
        }
        if matches!(
            chat.state,
            RunState::Running | RunState::WaitingForPermission
        ) {
            ui.set_notice("Este chat ya está ejecutando una tarea.".into());
            return;
        }
        let provider = state.chat(id).unwrap().provider;
        if !available_providers(&ui, &state).contains(&provider) {
            ui.set_notice("Configurá y verificá este agente antes de enviar.".into());
            return;
        }
        let mut config = state.settings.agent(provider).clone();
        if let Some(chat) = state.chat(id) {
            if !chat.model.is_empty() {
                config.model = chat.model.clone();
            }
            if let Some(effort) = &chat.reasoning_effort {
                config.reasoning_effort = effort.clone();
            }
        }
        if !config.enabled || (provider == Provider::Codex && config.model.is_empty()) {
            ui.set_notice("Habilitá Codex y elegí un modelo desde Configurar agentes.".into());
            return;
        }
        if provider == Provider::Codex && !supervisor.connection.has_session().unwrap_or(false) {
            ui.set_notice("Iniciá sesión con ChatGPT desde Configurar agentes.".into());
            return;
        }
        let project = state
            .projects
            .iter()
            .find(|p| p.chats.iter().any(|c| c.id == id))
            .unwrap()
            .path
            .clone();
        let profile = &state
            .projects
            .iter()
            .find(|p| p.chats.iter().any(|c| c.id == id))
            .unwrap()
            .profile;
        let project_context = match profile.context(std::path::Path::new(&project), provider) {
            Ok(context) => context,
            Err(error) => {
                ui.set_notice(error.into());
                return;
            }
        };
        let before = state.chat(id).unwrap().clone();
        let chat = state.chat_mut(id).unwrap();
        if chat.messages.is_empty() {
            chat.title = if text.is_empty() {
                "Consulta con imágenes".into()
            } else {
                text.chars().take(48).collect()
            };
        }
        if !retry {
            let images = std::mem::take(&mut chat.draft_images);
            if !images.is_empty() {
                chat.message_images
                    .insert(chat.messages.len(), images.clone());
            }
            chat.messages.push(Message {
                role: "Vos".into(),
                text: text.into(),
            });
            if provider == Provider::Codex && !chat.wire_history.is_empty() {
                ascurra_harness::runtime::repair_history(&mut chat.wire_history);
                chat.wire_history
                    .push(ascurra_harness::attachments::codex_message(text, &images));
            }
        }
        chat.state = RunState::Running;
        chat.last_status = "Codex conectado directamente · Ejecutando…".into();
        let mut previous = chat.clone();
        if retry && provider == Provider::Claude {
            // The CLI takes the last message as its prompt. Keep the displayed
            // transcript intact while retrying the latest user message.
            let index = previous
                .messages
                .iter()
                .rposition(|m| m.role == "Vos")
                .unwrap();
            previous.messages.truncate(index + 1);
        }
        previous.project_context = project_context;
        if let Err(error) = disk.save(&state) {
            *state.chat_mut(id).unwrap() = before;
            ui.set_notice(
                format!("No se inició la tarea porque no se pudo guardar: {error}").into(),
            );
            return;
        }
        let handle = ui.as_weak();
        let emit = Arc::new(move |event: Event| {
            if let Ok(event) = serde_json::to_string(&event) {
                let _ = handle.upgrade_in_event_loop(move |ui| {
                    ui.invoke_agent_event(event.into());
                });
            }
        });
        let started = if provider == Provider::Claude {
            supervisor.start_claude(previous, project, disk.root().into(), config.clone(), emit)
        } else {
            supervisor.start(
                previous,
                project,
                disk.root().into(),
                config.model,
                config.reasoning_effort,
                config.concurrency as usize,
                emit,
            )
        };
        if let Err(error) = started {
            *state.chat_mut(id).unwrap() = before;
            let _ = disk.save(&state);
            ui.set_notice(error.into());
            return;
        }
        let chat = state.chat_mut(id).unwrap();
        chat.state = RunState::Running;
        chat.last_status = "Codex conectado directamente · Ejecutando…".into();
        if !retry {
            chat.draft.clear();
            ui.set_draft("".into());
        }
        save_and_refresh(&ui, &mut state, &disk);
    });
    let send = submit.clone();
    ui.on_send_message(move |text, _provider_index| send(text, false));
    ui.on_retry_message(move || submit("".into(), true));
    Ok(ui)
}

fn configure_quota_callback(ui: &AppWindow, supervisor: Arc<Supervisor>) {
    let weak = ui.as_weak();
    let service = supervisor.clone();
    let last_claude_attempt = Rc::new(RefCell::new(None::<std::time::Instant>));
    ui.on_refresh_claude_quota(move |manual| {
        let Some(ui) = weak.upgrade() else { return };
        if !ui.get_claude_authenticated() {
            return;
        }
        if ui.get_claude_quota_busy() {
            if manual {
                ui.set_claude_quota_refresh_pending(true);
            }
            return;
        }
        if !manual
            && last_claude_attempt
                .borrow()
                .is_some_and(|at| at.elapsed().as_secs() < 60)
        {
            return;
        }
        *last_claude_attempt.borrow_mut() = Some(std::time::Instant::now());
        let path = ui.get_claude_verified_path().to_string();
        let config = AgentSettings {
            executable: path.clone(),
            ..Default::default()
        };
        ui.set_claude_quota_busy(true);
        let handle = ui.as_weak();
        service.executor.spawn(async move {
            let result = ascurra_harness::claude::account_usage(config).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_claude_quota_busy(false);
                if ui.get_claude_verified_path().as_str() != path || !ui.get_claude_authenticated()
                {
                    return;
                }
                match result {
                    Ok(usage) => {
                        ui.set_claude_quota_windows(ModelRc::new(VecModel::from(
                            usage
                                .windows
                                .iter()
                                .map(|w| QuotaRow {
                                    caption: w.caption(usage.checked_at).into(),
                                    remaining: w.remaining_percent as f32,
                                })
                                .collect::<Vec<_>>(),
                        )));
                        ui.set_claude_quota_status(
                            format!(
                                "Plan {} - Datos al consultar.",
                                usage.plan.as_deref().unwrap_or("Claude")
                            )
                            .into(),
                        );
                    }
                    Err(error) => {
                        ui.set_claude_quota_windows(ModelRc::new(VecModel::from(
                            Vec::<QuotaRow>::new(),
                        )));
                        ui.set_claude_quota_status(error.into());
                    }
                }
                if ui.get_claude_quota_refresh_pending() {
                    ui.set_claude_quota_refresh_pending(false);
                    ui.invoke_refresh_claude_quota(true);
                }
            });
        });
    });
    let last_attempt = Rc::new(RefCell::new(None::<std::time::Instant>));
    let weak = ui.as_weak();
    ui.on_refresh_quota(move |manual| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_codex_authenticated() {
            return;
        }
        if ui.get_quota_busy() {
            if manual {
                ui.set_quota_refresh_pending(true);
            }
            return;
        }
        if !manual && !ui.get_quota_auto() {
            return;
        }
        // Automatic updates cost a small HTTP read, never model inference.
        // No idle polling. Turn completions force a fresh read; concurrent reads coalesce.
        if !manual
            && last_attempt
                .borrow()
                .is_some_and(|at| at.elapsed().as_secs() < 60)
        {
            return;
        }
        *last_attempt.borrow_mut() = Some(std::time::Instant::now());
        let generation = ui.get_quota_generation();
        ui.set_quota_busy(true);
        let handle = ui.as_weak();
        let connection = supervisor.connection.clone();
        supervisor.executor.spawn(async move {
            let result = connection.account_usage().await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if ui.get_quota_generation() != generation {
                    return;
                }
                ui.set_quota_busy(false);
                match result {
                    Ok(usage) => {
                        ui.set_quota_auto(true);
                        ui.set_quota_windows(ModelRc::new(VecModel::from(
                            usage
                                .windows
                                .iter()
                                .map(|window| QuotaRow {
                                    caption: window.caption(usage.checked_at).into(),
                                    remaining: window.remaining_percent as f32,
                                })
                                .collect::<Vec<_>>(),
                        )));
                        ui.set_quota_status(
                            format!(
                                "Plan {} · cuota compartida. Datos al consultar.",
                                usage.plan.as_deref().unwrap_or("ChatGPT")
                            )
                            .into(),
                        );
                    }
                    Err(error) => {
                        ui.set_quota_auto(false);
                        ui.set_quota_windows(ModelRc::new(VecModel::from(Vec::<QuotaRow>::new())));
                        ui.set_quota_status(error.into());
                    }
                }
                if ui.get_quota_refresh_pending() {
                    ui.set_quota_refresh_pending(false);
                    ui.invoke_refresh_quota(true);
                }
            });
        });
    });
    ui.invoke_refresh_quota(false);
}

fn settings_from_ui(ui: &AppWindow, provider: Provider) -> AgentSettings {
    match provider {
        Provider::Codex => AgentSettings {
            enabled: ui.get_codex_enabled(),
            executable: String::new(), // Direct provider: no executable path.
            model: ui.get_codex_model().trim().into(),
            models: Vec::new(),
            reasoning_effort: settings::REASONING_EFFORTS
                .get(ui.get_codex_effort_index() as usize)
                .copied()
                .unwrap_or("")
                .into(),
            concurrency: ui.get_codex_concurrency().clamp(1, 4) as u8,
        },
        Provider::Claude => AgentSettings {
            enabled: ui.get_claude_enabled(),
            executable: ui.get_claude_path().trim().into(),
            model: ui.get_claude_model().trim().into(),
            models: Vec::new(),
            reasoning_effort: String::new(),
            concurrency: ui.get_claude_concurrency().clamp(1, 4) as u8,
        },
    }
}

fn set_agent_path(ui: &AppWindow, provider: Provider, path: &std::path::Path) {
    let path = path.to_string_lossy();
    match provider {
        Provider::Codex => {
            ui.set_codex_path(path.as_ref().into());
            ui.set_codex_status("Ruta seleccionada. Falta verificar el cliente.".into());
        }
        Provider::Claude => {
            if ui.get_claude_verified_path().as_str() != path.as_ref() {
                ui.set_claude_authenticated(false);
            }
            ui.set_claude_path(path.as_ref().into());
            ui.set_claude_status("Ruta seleccionada. Falta verificar el cliente.".into());
        }
    }
}

fn set_agent_status(ui: &AppWindow, provider: Provider, status: String) {
    match provider {
        Provider::Codex => ui.set_codex_status(status.into()),
        Provider::Claude => ui.set_claude_status(status.into()),
    }
}

fn configure_runtime_callbacks(
    ui: &AppWindow,
    store: Rc<Store>,
    state: Rc<RefCell<AppState>>,
    supervisor: Arc<Supervisor>,
) {
    let weak = ui.as_weak();
    let service = supervisor.clone();
    let data = state.clone();
    ui.on_cancel_chat(move || {
        if weak.upgrade().is_some() {
            if let Some(id) = data.borrow().selected_chat {
                service.cancel(id);
            }
        }
    });
    // Each callback operates on the selected chat; workers always carry their original id.
    let weak = ui.as_weak();
    let service = supervisor.clone();
    let data = state.clone();
    ui.on_toggle_auto_approve({
        let weak = ui.as_weak();
        let service = supervisor.clone();
        let data = state.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return; };
            let mut state = data.borrow_mut();
            let Some(id) = state.selected_chat else { return; };
            let key = id.to_string();
            let mut ids = ui.get_auto_approve_chats().iter().collect::<Vec<_>>();
            let enable = !ids.iter().any(|value| value.as_str() == key);
            ids.retain(|value| value.as_str() != key);
            if enable {
                ids.push(key.into());
                if let Some(chat) = state.chat_mut(id) {
                    if !chat.approval.is_empty() {
                        service.approve(id, true);
                        chat.approval.clear();
                        chat.state = RunState::Running;
                    }
                }
            }
            ui.set_auto_approve_chats(ModelRc::new(VecModel::from(ids)));
            preserve_composer(&ui, &mut state);
            refresh(&ui, &state);
        }
    });
    ui.on_approve_tool(move |allowed| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        if let Some(id) = state.selected_chat {
            service.approve(id, allowed);
            if let Some(chat) = state.chat_mut(id) {
                chat.approval.clear();
                chat.state = RunState::Running;
            }
            preserve_composer(&ui, &mut state);
            refresh(&ui, &state);
        }
    });
    let weak = ui.as_weak();
    let stream_pending = Rc::new(std::cell::Cell::new(false));
    ui.on_agent_event(move |event| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(event) = serde_json::from_str::<Event>(&event) else {
            return;
        };
        let event_chat = match &event {
            Event::Session { chat, .. }
            | Event::TurnUsage { chat, .. }
            | Event::GenerationStarted { chat }
            | Event::Text { chat, .. }
            | Event::ResponseText { chat, .. }
            | Event::Usage { chat, .. }
            | Event::Checkpoint { chat, .. }
            | Event::Approval { chat, .. }
            | Event::ToolResult { chat, .. }
            | Event::Finished { chat, .. } => *chat,
        };
        if let Err(error) = store.hydrate(&mut state.borrow_mut(), event_chat) {
            supervisor.cancel(event_chat);
            ui.set_notice(format!("No se pudo recuperar el historial: {error}").into());
            return;
        }
        if let Event::Text { chat: id, delta } = &event {
            let mut data = state.borrow_mut();
            if let Some(chat) = data.chat_mut(*id) {
                if let Some(message) = chat
                    .messages
                    .last_mut()
                    .filter(|m| m.role == provider_name(chat.provider))
                {
                    message.text.push_str(delta);
                }
            }
            if data.selected_chat == Some(*id) && !stream_pending.replace(true) {
                let weak = ui.as_weak();
                let data = state.clone();
                let pending = stream_pending.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
                    pending.set(false);
                    if let Some(ui) = weak.upgrade() {
                        let state = data.borrow();
                        refresh_stream_tail(&ui, &state);
                    }
                });
            }
            return;
        }
        let mut state = state.borrow_mut();
        preserve_composer(&ui, &mut state);
        let finished = matches!(event, Event::Finished { .. });
        let persist = !matches!(event, Event::Text { .. } | Event::GenerationStarted { .. });
        match event {
            Event::Session { chat, session_id } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.provider_session_id = Some(session_id);
                }
            }
            Event::TurnUsage {
                chat,
                request_ids,
                record,
            } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.usage.retain(|u| {
                        u.provider != Provider::Claude || !request_ids.contains(&u.request_id)
                    });
                    chat.record_usage(record);
                }
            }
            Event::GenerationStarted { chat } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.state = RunState::Running;
                    chat.messages.push(Message {
                        role: provider_name(chat.provider).into(),
                        text: String::new(),
                    });
                }
            }
            Event::Text { chat, delta } => {
                if let Some(chat) = state.chat_mut(chat) {
                    if let Some(message) = chat
                        .messages
                        .last_mut()
                        .filter(|m| m.role == provider_name(chat.provider))
                    {
                        message.text.push_str(&delta);
                    }
                }
            }
            Event::ResponseText { chat, text } => {
                if let Some(chat) = state.chat_mut(chat) {
                    if let Some(message) = chat
                        .messages
                        .last_mut()
                        .filter(|m| m.role == provider_name(chat.provider))
                    {
                        message.text = text;
                    } else {
                        chat.messages.push(Message {
                            role: provider_name(chat.provider).into(),
                            text,
                        });
                    }
                }
            }
            Event::Usage { chat, record } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.record_usage(record);
                }
            }
            Event::Checkpoint { chat, history } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.wire_history = history;
                }
            }
            Event::Approval { chat, description } => {
                if ui.get_auto_approve_chats().iter().any(|value| value.as_str() == chat.to_string()) {
                    supervisor.approve(chat, true);
                    if let Some(chat) = state.chat_mut(chat) {
                        chat.approval.clear();
                        chat.state = RunState::Running;
                        chat.last_status = "Acción aprobada automáticamente para este chat.".into();
                    }
                } else {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.approval = description;
                    chat.state = RunState::WaitingForPermission;
                    chat.last_status = "Codex espera tu aprobación en este chat.".into();
                }
                }
            }
            Event::ToolResult {
                chat,
                name,
                reply,
                summary,
            } => {
                if let Some(chat) = state.chat_mut(chat) {
                    chat.approval.clear();
                    chat.state = RunState::Running;
                    chat.last_status = "Codex conectado directamente · Ejecutando…".into();
                    chat.messages.retain(|m| !m.text.is_empty());
                    chat.messages.push(Message {
                        role: "Herramienta".into(),
                        text: if summary.is_empty() {
                            ascurra_harness::diagnostics::tool_summary(
                                &name,
                                &serde_json::Value::Null,
                                &reply,
                                None,
                            )
                        } else {
                            summary
                        },
                    });
                }
            }
            Event::Finished {
                chat,
                state: final_state,
                message,
                history,
                incomplete_usage,
            } => {
                if state
                    .chat(chat)
                    .is_some_and(|c| c.provider == Provider::Claude)
                {
                    ui.invoke_refresh_claude_quota(true);
                } else {
                    ui.invoke_refresh_quota(true);
                }
                if let Some(chat) = state.chat_mut(chat) {
                    chat.state = final_state;
                    chat.approval.clear();
                    chat.last_status = message;
                    chat.wire_history = history;
                    chat.usage_incomplete |= incomplete_usage;
                    chat.messages.retain(|m| !m.text.is_empty());
                }
            }
        }
        state.sync_task_run(event_chat);
        let mut next_queue = if finished {
            state.finish_queued_chat(event_chat)
        } else {
            None
        };
        if persist {
            if let Err(error) = store.save(&state) {
                if let Some(project) = next_queue.take() {
                    state.pause_task_queue(project);
                }
                supervisor.cancel_all();
                ui.set_notice(
                    format!("Se detuvo la ejecución porque no se pudo guardar: {error}").into(),
                );
            } else {
                store.release_inactive(&mut state);
            }
        }
        refresh(&ui, &state);
        if finished {
            ui.invoke_refresh_git();
            if ui.get_database_open() {
                ui.invoke_refresh_database(false);
            }
        }
        drop(state);
        if finished && ui.get_usage_open() {
            ui.invoke_refresh_usage();
        }
        if finished && ui.get_captures_open() {
            ui.invoke_refresh_captures();
        }
        if let Some(project) = next_queue {
            ui.invoke_advance_task_queue(project.to_string().into());
        }
    });
}

fn configure_settings_callbacks(
    ui: &AppWindow,
    store: Rc<Store>,
    state: Rc<RefCell<AppState>>,
    supervisor: Arc<Supervisor>,
) {
    let weak = ui.as_weak();
    let data = state.clone();
    let connection = supervisor.connection.clone();
    ui.on_close_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_settings_busy() {
            return;
        }
        let mut state = data.borrow_mut();
        preserve_composer(&ui, &mut state);
        ui.set_codex_authenticated(connection.has_session().unwrap_or(false));
        ui.set_settings_open(false);
        refresh(&ui, &state);
    });
    let models = Rc::new(RefCell::new(state.borrow().settings.codex.models.clone()));
    ui.set_codex_models(ModelRc::new(VecModel::from(
        models
            .borrow()
            .iter()
            .map(|model| slint::SharedString::from(model.label.as_str()))
            .collect::<Vec<_>>(),
    )));
    let cancel_auth = Arc::new(Mutex::new(None::<tokio::sync::watch::Sender<bool>>));
    let weak = ui.as_weak();
    let choices = models.clone();
    ui.on_pick_codex_model(move |index| {
        if let (Some(ui), Some(model)) = (weak.upgrade(), choices.borrow().get(index as usize)) {
            ui.set_codex_model(model.slug.clone().into());
        }
    });
    let weak = ui.as_weak();
    let choices = models.clone();
    let auth = cancel_auth.clone();
    let connection = supervisor.connection.clone();
    let data = state.clone();
    let disk = store.clone();
    ui.on_auth_event(move |event| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&event) else {
            return;
        };
        ui.set_codex_status(
            value["status"]
                .as_str()
                .unwrap_or("Respuesta de autenticación inválida.")
                .into(),
        );
        if value["done"].as_bool() == Some(true) {
            ui.set_codex_authenticated(connection.has_session().unwrap_or(false));
            // Discard quota requests issued for a previous authentication state.
            ui.set_quota_generation(ui.get_quota_generation().wrapping_add(1));
            ui.set_quota_busy(false);
            ui.set_quota_windows(ModelRc::new(VecModel::from(Vec::<QuotaRow>::new())));
            ui.set_quota_status("Conectá ChatGPT para consultar tu cuota.".into());
            ui.invoke_refresh_quota(true);
            let mut state = data.borrow_mut();
            ui.set_codex_configured(
                ui.get_codex_authenticated()
                    && state.settings.codex.enabled
                    && !state.settings.codex.model.is_empty(),
            );
            ui.set_settings_busy(false);
            auth.lock().unwrap().take();
            if let Some(list) = value.get("models") {
                if let Ok(new_models) = serde_json::from_value::<Vec<ModelChoice>>(list.clone()) {
                    state.settings.codex.models = new_models.clone();
                    if let Err(error) = disk.save(&state) {
                        ui.set_settings_notice(
                            format!("No se pudo guardar el catálogo: {error}").into(),
                        );
                    }
                    *choices.borrow_mut() = new_models;
                    ui.set_codex_models(ModelRc::new(VecModel::from(
                        choices
                            .borrow()
                            .iter()
                            .map(|m| slint::SharedString::from(m.label.as_str()))
                            .collect::<Vec<_>>(),
                    )));
                    let index = choices
                        .borrow()
                        .iter()
                        .position(|m| m.slug == ui.get_codex_model().as_str())
                        .unwrap_or(0);
                    ui.set_codex_model_index(index as i32);
                    if let Some(model) = choices.borrow().get(index) {
                        ui.set_codex_model(model.slug.clone().into());
                    }
                }
            }
            refresh(&ui, &state);
        }
    });
    let auth = cancel_auth.clone();
    ui.on_cancel_auth(move || {
        if let Some(cancel) = auth.lock().unwrap().as_ref() {
            let _ = cancel.send(true);
        }
    });
    let weak = ui.as_weak();
    let service = supervisor.clone();
    let auth = cancel_auth.clone();
    ui.on_disconnect_codex(move || {
        let Some(ui) = weak.upgrade() else { return; };
        if ui.get_settings_busy() { return; }
        if service.running() { ui.set_settings_notice("Detené los chats activos antes de desconectar la cuenta.".into()); return; }
        ui.set_settings_busy(true);
        let (cancel, mut receiver) = tokio::sync::watch::channel(false); *auth.lock().unwrap() = Some(cancel);
        let connection = service.connection.clone(); let handle = ui.as_weak();
        service.executor.spawn(async move {
            let result = tokio::select! { _ = ascurra_harness::codex::cancelled(&mut receiver) => Err("Desconexión cancelada.".into()), value = connection.logout() => value };
            let event = serde_json::json!({"done":true,"status":result.unwrap_or_else(|error|error),"models":[]}).to_string();
            let _ = handle.upgrade_in_event_loop(move |ui| { ui.invoke_auth_event(event.into()); });
        });
    });
    let weak = ui.as_weak();
    ui.on_invalidate_agent(move |index| {
        if let Some(ui) = weak.upgrade() {
            if index == 1 {
                ui.set_claude_authenticated(false);
            }
            set_agent_status(
                &ui,
                if index == 0 {
                    Provider::Codex
                } else {
                    Provider::Claude
                },
                "Ruta modificada. Verificá nuevamente el cliente.".into(),
            );
        }
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let service = supervisor.clone();
    ui.on_open_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_settings_busy() {
            return;
        }
        let state = data.borrow();
        let codex = &state.settings.codex;
        ui.set_codex_enabled(codex.enabled);
        ui.set_codex_path(codex.executable.clone().into());
        ui.set_codex_model(codex.model.clone().into());
        ui.set_codex_effort_index(settings::REASONING_EFFORTS.iter().position(|effort| *effort == codex.reasoning_effort).unwrap_or(0) as i32);
        ui.set_codex_concurrency(codex.concurrency.into());
        let claude = &state.settings.claude;
        ui.set_claude_enabled(claude.enabled);
        ui.set_claude_path(if claude.executable.is_empty() { settings::detect_client(Provider::Claude).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into() } else { claude.executable.clone().into() });
        ui.set_claude_model(claude.model.clone().into());
        ui.set_claude_concurrency(claude.concurrency.into());
        for provider in [Provider::Codex, Provider::Claude] {
            if provider == Provider::Codex {
                set_agent_status(
                    &ui,
                    provider,
                    service.connection.status().unwrap_or_else(|error| error),
                );
                continue;
            }
            set_agent_status(&ui, provider, if ui.get_claude_authenticated() { "Cliente oficial verificado." } else { "Detecta Claude Code y verifica la sesion del cliente oficial." }.into());
            continue;
        }
        ui.set_settings_notice(
            "Codex: OAuth directo y sesión protegida por Windows. Claude: cliente oficial con herramientas del harness."
                .into(),
        );
        ui.set_settings_open(true);
        drop(state);
        // Restore account-specific choices using the saved grant, without reauthorizing.
        if service.connection.has_session().unwrap_or(false) && !service.running() {
            ui.invoke_check_agent(0, false);
        }
    });

    let weak = ui.as_weak();
    let data = state.clone();
    ui.on_claude_auth_event(move |event| {
        let Some(ui) = weak.upgrade() else { return };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&event) else {
            return;
        };
        ui.set_claude_verified_path(value["path"].as_str().unwrap_or("").into());
        ui.set_claude_authenticated(value["ok"] == true);
        ui.set_claude_status(
            value["status"]
                .as_str()
                .unwrap_or("No se pudo verificar Claude.")
                .into(),
        );
        refresh(&ui, &data.borrow());
        if ui.get_claude_authenticated() {
            ui.invoke_refresh_claude_quota(false);
        } else {
            ui.set_claude_quota_windows(ModelRc::new(VecModel::from(Vec::<QuotaRow>::new())));
        }
    });
    let config = state.borrow().settings.claude.clone();
    if config.enabled {
        let handle = ui.as_weak();
        std::thread::spawn(move || {
            let result = ascurra_harness::claude::verify(&config);
            let event = serde_json::json!({"ok":result.is_ok(),"path":config.executable,"status":result.unwrap_or_else(|e| e)}).to_string();
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.invoke_claude_auth_event(event.into());
            });
        });
    }
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    let connection = supervisor.connection.clone();
    let choices = models.clone();
    ui.on_save_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_settings_busy() {
            return;
        }
        let mut codex = settings_from_ui(&ui, Provider::Codex);
        codex.models = choices.borrow().clone();
        let claude = settings_from_ui(&ui, Provider::Claude);
        if claude.enabled && ascurra_harness::claude::executable(&claude).is_err() {
            ui.set_settings_notice("Seleccioná el ejecutable nativo de Claude Code.".into());
            return;
        }
        if codex.enabled && codex.model.is_empty() {
            ui.set_settings_notice(
                "Elegí un modelo de Codex desde el catálogo de tu cuenta.".into(),
            );
            return;
        }
        let mut state = data.borrow_mut();
        let previous = state.settings.clone();
        state.settings.codex = codex;
        state.settings.claude = claude;
        // Preserve the current chat draft while committing independent settings.
        preserve_composer(&ui, &mut state);
        if let Err(error) = disk.save(&state) {
            state.settings = previous;
            ui.set_settings_notice(format!("No se pudo guardar la configuración: {error}").into());
            return;
        }
        ui.set_settings_open(false);
        ui.set_codex_authenticated(connection.has_session().unwrap_or(false));
        refresh(&ui, &state);
        ui.set_notice(
            "Configuración guardada. Codex usa conexión directa y herramientas del harness.".into(),
        );
    });

    let weak = ui.as_weak();
    ui.on_detect_agent(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let provider = if index == 0 {
            Provider::Codex
        } else {
            Provider::Claude
        };
        if let Some(path) = settings::detect_client(provider) {
            set_agent_path(&ui, provider, &path);
        } else {
            set_agent_status(
                &ui,
                provider,
                "No se encontró el cliente. Instalalo o seleccioná su archivo con «Buscar…»."
                    .into(),
            );
        }
    });

    let weak = ui.as_weak();
    ui.on_browse_agent(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_settings_busy() {
            return;
        }
        let provider = if index == 0 {
            Provider::Codex
        } else {
            Provider::Claude
        };
        let config = settings_from_ui(&ui, provider);
        ui.set_settings_busy(true);
        let handle = ui.as_weak();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Seleccionar cliente oficial");
            #[cfg(windows)]
            {
                dialog = dialog.add_filter("Clientes", &["exe", "cmd", "ps1"]);
            }
            if let Some(parent) = std::path::Path::new(&config.executable)
                .parent()
                .filter(|p| p.is_dir())
            {
                dialog = dialog.set_directory(parent);
            }
            let path = dialog.pick_file();
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_settings_busy(false);
                if let Some(path) = path {
                    set_agent_path(&ui, provider, &path);
                }
            });
        });
    });

    let weak = ui.as_weak();
    let service = supervisor.clone();
    let auth = cancel_auth.clone();
    ui.on_check_agent(move |index, login| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_settings_busy() {
            return;
        }
        if index == 0 {
            if service.running() { ui.set_settings_notice("Terminá o detené las tareas antes de modificar la conexión.".into()); return; }
            ui.set_settings_busy(true);
            ui.set_codex_status("Conectando con ChatGPT…".into());
            let (cancel, mut receiver) = tokio::sync::watch::channel(false); *auth.lock().unwrap() = Some(cancel);
            let connection = service.connection.clone(); let handle = ui.as_weak();
            service.executor.spawn(async move {
                let notice_handle = handle.clone();
                let notice = move |status: String| { let value = serde_json::json!({"status":status}).to_string(); let _ = notice_handle.upgrade_in_event_loop(move |ui| { ui.invoke_auth_event(value.into()); }); };
                let login_cancel = receiver.clone();
                let result: Result<(String, Vec<ModelChoice>), String> = tokio::select! {
                    _ = ascurra_harness::codex::cancelled(&mut receiver) => Err("Conexión cancelada.".into()),
                    value = async {
                        if login { connection.login(login_cancel, notice).await?; }
                        let status = connection.status()?;
                        match connection.models().await { Ok(models) => Ok((status, models)), Err(error) => Err(format!("{status}\n{error}")) }
                    } => value
                };
                let value = match result { Ok((status, models)) => serde_json::json!({"done":true,"status":status,"models":models}), Err(error) => serde_json::json!({"done":true,"status":error}) }.to_string();
                let _ = handle.upgrade_in_event_loop(move |ui| { ui.invoke_auth_event(value.into()); });
            });
            return;
        }
        let provider = if index == 0 {
            Provider::Codex
        } else {
            Provider::Claude
        };
        let config = settings_from_ui(&ui, provider);
        if let Err(error) = settings::validate_settings(&config) {
            set_agent_status(&ui, provider, error);
            return;
        }
        ui.set_settings_busy(true);
        set_agent_status(
            &ui,
            provider,
            if login {
                "Completá el inicio de sesión en el navegador del cliente oficial."
            } else {
                "Comprobando instalación y autenticación…"
            }
            .into(),
        );
        let handle = ui.as_weak();
        std::thread::spawn(move || {
            let result = if login {
                ascurra_harness::claude::login(&config)
            } else { ascurra_harness::claude::verify(&config) };
            let event = serde_json::json!({"ok":result.is_ok(),"path":config.executable,"status":result.unwrap_or_else(|e| e)}).to_string();
            let _ = handle.upgrade_in_event_loop(move |ui| { ui.set_settings_busy(false); ui.invoke_claude_auth_event(event.into()); });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    #[test]
    fn context_uses_last_input_including_cache_and_never_total_spending() {
        use ascurra_harness::domain::{TokenUsage, UsageRecord};
        let mut state = AppState::default();
        let project = state.add_project("Fixture".into(), "fixture".into());
        let id = state.add_chat(project).unwrap();
        let chat = state.chat_mut(id).unwrap();
        for (request_id, input) in [("first", Some(900)), ("last", Some(250))] {
            chat.record_usage(UsageRecord {
                request_id: request_id.into(),
                model: "fixture".into(),
                provider: Provider::Codex,
                context_input: None,
                context_window: None,
                usage: TokenUsage {
                    input,
                    output: Some(100),
                    cache_read: Some(200),
                    ..Default::default()
                },
            });
        }
        let choices = vec![ModelChoice {
            slug: "fixture".into(),
            context_window: Some(1000),
            ..Default::default()
        }];
        let (label, fraction) = context_meter(Some(chat), "fixture", &choices);
        assert!(label.contains("250 / 1k"));
        assert_eq!(fraction, 0.25);
        assert!(
            context_meter(Some(chat), "other", &choices)
                .0
                .contains("sin medición")
        );
        chat.usage.last_mut().unwrap().usage.input = None;
        assert!(
            context_meter(Some(chat), "fixture", &choices)
                .0
                .contains("sin medición")
        );
        let legacy: ModelChoice = serde_json::from_str(r#"{"slug":"old","label":"Old"}"#).unwrap();
        assert_eq!(legacy.context_window, None);
    }

    #[test]
    fn markdown_and_legacy_tool_summaries_preserve_format_and_filename() {
        let rendered = render_message("Codex", "**Rust + Slint** y `cargo run`");
        assert_ne!(
            rendered,
            slint::StyledText::from_plain_text("**Rust + Slint** y `cargo run`")
        );
        assert_eq!(
            render_message("Vos", "**texto literal**"),
            slint::StyledText::from_plain_text("**texto literal**")
        );
        let reply = r#"{"text":"file fixture","more":false}"#;
        let history = vec![
            serde_json::json!({"type":"function_call","call_id":"fixture-call","name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}),
            serde_json::json!({"type":"function_call_output","call_id":"fixture-call","output":reply}),
        ];
        let display = display_message(
            &Message {
                role: "Herramienta".into(),
                text: format!("read_file: {reply}"),
            },
            &history,
        );
        assert!(display.contains("src/main.rs"));
        assert!(!display.contains("file fixture"));
        assert!(!display.contains("0 ms"));
    }

    #[test]
    fn chat_scroll_reaches_settled_bottom_on_open_and_idle_message() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()), mock_time: true, ..Default::default()
            },
        ))).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let store = Rc::new(Store::new(temp.path().join("data")).unwrap());
        let state = Rc::new(RefCell::new(AppState::default()));
        let ui = build_app(store, state).unwrap();
        ui.set_has_chat(true);
        ui.set_chat_running(false);
        ui.set_project_overview(false);
        ui.set_displayed_chat_key("first".into());
        let row = |lines: usize| {
            let body = "A line of agent response.\n".repeat(lines);
            MessageRow { role: "Asistente".into(), blocks: markdown_ui::blocks("Asistente", &body), summary: body.lines().next().unwrap_or_default().into(), body: body.into() }
        };
        let rows = Rc::new(VecModel::from((0..12).map(|_| row(12)).collect::<Vec<_>>()));
        ui.set_messages(ModelRc::from(rows.clone()));
        ui.show().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(900, 620));
        let tick = || {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(20);
        };
        scroll_opened_chat(&ui);
        for _ in 0..4 { tick(); }
        // A late height update must not leave the scroll at the previous bottom.
        rows.set_row_data(11, row(90));
        for _ in 0..65 { tick(); }
        assert!(ui.get_chat_scroll_bottom() < -100.0);
        assert!((ui.get_chat_scroll_offset() - ui.get_chat_scroll_bottom()).abs() < 2.0);
        assert!(!ui.get_chat_follow_pending());
        // Final messages must follow even though the task is no longer running.
        follow_chat_update(&ui);
        rows.push(row(35));
        for _ in 0..65 { tick(); }
        assert!((ui.get_chat_scroll_offset() - ui.get_chat_scroll_bottom()).abs() < 2.0);
        // Returning from another workspace tab requests a fresh settled scroll.
        ui.set_usage_open(true);
        tick();
        ui.set_usage_open(false);
        ui.set_displayed_chat_key("second".into());
        scroll_opened_chat(&ui);
        for _ in 0..65 { tick(); }
        assert!((ui.get_chat_scroll_offset() - ui.get_chat_scroll_bottom()).abs() < 2.0);
        assert!(!ui.get_chat_follow_pending());
        assert!(!ui.get_chat_opening_scroll());
    }
    #[test]
    fn ui_navigation_preserves_drafts_and_sessions_and_renders() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                ..Default::default()
            },
        )))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let store = Rc::new(Store::new(temp.path().join("data")).unwrap());
        let state = Rc::new(RefCell::new(AppState::default()));
        let ui = build_app(store.clone(), state.clone()).unwrap();
        assert!(!ui.get_codex_configured());
        ui.show().unwrap();
        // Check the empty view and resized client area at Windows DPI scales.
        // Every bottom-edge pixel must be the window background, never white.
        for (width, height, scale) in [
            (1280, 820, 1.0),
            (1250, 1030, 1.25),
            (1920, 1080, 1.5),
            (900, 620, 1.0),
        ] {
            ui.window()
                .dispatch_event_with_result(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                })
                .unwrap();
            ui.window()
                .set_size(slint::PhysicalSize::new(width, height));
            // TestingBackend assumes scale 1 when dispatching its resize event.
            // Supply the correct logical size for the simulated display scale.
            ui.window()
                .dispatch_event_with_result(slint::platform::WindowEvent::Resized {
                    size: slint::LogicalSize::new(width as f32 / scale, height as f32 / scale),
                })
                .unwrap();
            let snapshot = ui.window().take_snapshot().unwrap();
            let bottom =
                &snapshot.as_slice()[((snapshot.height() - 1) * snapshot.width()) as usize..];
            assert!(
                bottom
                    .iter()
                    .all(|pixel| (pixel.r, pixel.g, pixel.b, pixel.a) == (16, 24, 34, 255)),
                "Unpainted bottom edge at {width}x{height}, scale {scale}"
            );
            if scale == 1.25 {
                image::save_buffer(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-empty-preview.png"),
                    snapshot.as_bytes(),
                    snapshot.width(),
                    snapshot.height(),
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
        ui.invoke_add_project(
            "Harness".into(),
            temp.path().to_string_lossy().as_ref().into(),
        );
        let first = state.borrow().selected_chat.unwrap();
        ui.invoke_send_message("Diseñemos herramientas con respuestas mínimas.".into(), 0);
        // Sending with no enabled connection must not create a request or lose input.
        assert!(state.borrow().chat(first).unwrap().messages.is_empty());
        state
            .borrow_mut()
            .chat_mut(first)
            .unwrap()
            .messages
            .push(Message {
                role: "Vos".into(),
                text: "Fixture local de navegación".into(),
            });
        state.borrow_mut().chat_mut(first).unwrap().state = RunState::Running;
        ui.set_draft("Borrador pendiente".into());
        let project_id = state.borrow().projects[0].id;
        ui.invoke_select_row(format!("p{project_id}").into());
        assert!(ui.get_project_overview());
        assert!(ui.get_has_project());
        assert!(!ui.get_has_chat());
        assert_eq!(state.borrow().selected_chat, None);
        assert_eq!(
            state.borrow().chat(first).unwrap().draft,
            "Borrador pendiente"
        );
        assert_eq!(state.borrow().chat(first).unwrap().state, RunState::Running);
        assert_eq!(ui.get_project_chats().row_count(), 1);
        assert_eq!(ui.get_project_running_count(), 1);
        ui.invoke_select_row(format!("p{project_id}").into());
        assert_eq!(
            state.borrow().open_project_tabs,
            vec![project_id],
            "La pestaña de proyecto se reutiliza"
        );
        assert!(
            ui.get_tabs()
                .iter()
                .any(|tab| tab.id == format!("p{project_id}") && tab.selected)
        );
        ui.window().set_size(slint::PhysicalSize::new(1280, 820));
        let preview = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-project-preview.png"),
            preview.as_bytes(),
            preview.width(),
            preview.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("AGENTS.md"),
            "Reglas de Codex del proyecto de prueba.",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("CLAUDE.md"),
            "Reglas de Claude del proyecto de prueba.",
        )
        .unwrap();
        ui.invoke_open_project_config();
        assert!(ui.get_project_settings_view());
        assert!(ui.get_project_rules().contains("Reglas de Codex"));
        assert!(ui.get_project_rules().contains("Reglas de Claude"));
        let imported = ui.get_project_rules();
        ui.invoke_import_project_rules(2);
        assert_eq!(ui.get_project_rules(), imported);
        ui.set_project_rules("Regla editada".into());
        ui.invoke_import_project_rules(0);
        assert!(ui.get_project_rules().contains("Regla editada"));
        assert!(ui.get_project_rules().contains("Reglas de Codex"));
        assert!(!ui.get_project_rules().contains("Reglas de Claude"));
        ui.set_project_description("Harness de escritorio para agentes.".into());
        ui.set_project_test_command("cargo test --offline".into());
        ui.set_project_rules("Salidas de herramientas acotadas.".into());
        ui.invoke_add_build_command();
        ui.invoke_save_project_config();
        assert!(ui.get_project_config_message().contains("Completá"));
        ui.invoke_edit_build_command(0, "cargo build".into(), "Desarrollo local".into());
        ui.invoke_add_build_command();
        ui.invoke_edit_build_command(1, "cargo build --release".into(), "Distribución".into());
        ui.invoke_save_project_config();
        assert!(ui.get_project_config_message().contains("guardada"));
        let restored = store.load().unwrap();
        let profile = &restored
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .unwrap()
            .profile;
        assert_eq!(profile.builds.len(), 2);
        assert_eq!(profile.test_command, "cargo test --offline");
        assert_eq!(profile.rules, "Salidas de herramientas acotadas.");
        assert!(
            !profile
                .context(temp.path(), Provider::Codex)
                .unwrap()
                .contains("Reglas de Codex")
        );
        assert!(
            profile
                .context(temp.path(), Provider::Codex)
                .unwrap()
                .contains("Desarrollo local")
        );
        ui.set_project_description("Cambio sin guardar".into());
        refresh(&ui, &state.borrow());
        assert_eq!(ui.get_project_description(), "Cambio sin guardar");
        ui.invoke_open_project_config();
        assert_eq!(
            ui.get_project_description(),
            "Harness de escritorio para agentes."
        );
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-project-config-preview.png",
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_project_settings_view(false);
        ui.set_project_tasks_view(true);
        ui.invoke_edit_task("".into());
        assert!(ui.get_task_dialog_open());
        ui.set_task_title(" ".into());
        ui.invoke_save_task();
        assert!(ui.get_task_dialog_open());
        assert!(!ui.get_task_error().is_empty());
        let mut task_ids = Vec::new();
        let change_task_column = |id: u64, column: i32| {
            ui.invoke_edit_task(id.to_string().into());
            ui.set_task_status(column);
            ui.invoke_save_task();
        };
        for (title, objective, acceptance, severity, column) in [
            (
                "Reducir consumo de memoria",
                "Mantener el consumo estable al abrir archivos grandes.",
                "El visor conserva la caché acotada y el scroll fluido.",
                3,
                0,
            ),
            (
                "Resaltar diferencias",
                "Distinguir líneas añadidas y eliminadas.",
                "Los cambios se muestran en verde y rojo.",
                2,
                1,
            ),
            (
                "Guardar proyectos",
                "Recuperar el espacio de trabajo al iniciar.",
                "Las pestañas y los chats reaparecen al abrir la aplicación.",
                0,
                2,
            ),
        ] {
            ui.invoke_edit_task("".into());
            ui.set_task_title(title.into());
            ui.set_task_objective(objective.into());
            ui.set_task_acceptance(acceptance.into());
            ui.set_task_severity(severity);
            ui.invoke_save_task();
            assert!(!ui.get_task_dialog_open());
            let id = state.borrow().projects[0].tasks.last().unwrap().id;
            task_ids.push(id);
            change_task_column(id, column);
        }
        assert_eq!(ui.get_tasks_todo().row_count(), 1);
        assert_eq!(ui.get_tasks_progress().row_count(), 1);
        assert_eq!(ui.get_tasks_done().row_count(), 1);
        ui.invoke_edit_task(task_ids[0].to_string().into());
        ui.set_task_status(2);
        ui.set_task_dialog_open(false);
        assert_eq!(
            ui.get_tasks_todo().row_count(),
            1,
            "Cancelar no cambia la columna"
        );
        assert_eq!(ui.get_tasks_done().row_count(), 1);
        let loaded = store.load().unwrap();
        assert_eq!(loaded.projects[0].tasks.len(), 3);
        assert_eq!(
            loaded.projects[0].tasks[2].status,
            ascurra_harness::domain::TaskStatus::Done
        );
        let board = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-tasks-preview.png"),
            board.as_bytes(),
            board.width(),
            board.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.invoke_edit_task(task_ids[1].to_string().into());
        assert_eq!(ui.get_task_title(), "Resaltar diferencias");
        let editor = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-task-editor-preview.png"),
            editor.as_bytes(),
            editor.width(),
            editor.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_task_acceptance("Texto actualizado de aceptación".into());
        ui.invoke_save_task();
        assert_eq!(
            ui.get_tasks_progress().row_data(0).unwrap().acceptance,
            "Texto actualizado de aceptación"
        );
        change_task_column(task_ids[0], 1);
        assert_eq!(ui.get_tasks_todo().row_count(), 0);
        assert_eq!(ui.get_tasks_progress().row_count(), 2);
        change_task_column(task_ids[0], 0);
        ui.set_project_tasks_view(false);
        ui.invoke_new_chat();
        assert!(!ui.get_project_overview());
        ui.invoke_select_row(format!("p{project_id}").into());
        ui.invoke_close_tab(format!("p{project_id}").into());
        assert!(!ui.get_project_overview());
        assert!(state.borrow().open_project_tabs.is_empty());
        let second = state.borrow().selected_chat.unwrap();
        assert_ne!(first, second);
        assert_eq!(
            state.borrow().chat(first).unwrap().draft,
            "Borrador pendiente"
        );
        ui.invoke_select_row(first.to_string().into());
        assert_eq!(ui.get_draft(), "Borrador pendiente");
        // Exercise the actual × hit area, rather than invoking its callback directly.
        ui.window().set_size(slint::PhysicalSize::new(1280, 820));
        let _ = ui.window().take_snapshot().unwrap();
        let position = slint::LogicalPosition::new(522.0, 78.0);
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
        assert!(
            !state.borrow().open_tabs.contains(&first),
            "El clic en × debe cerrar la pestaña"
        );
        assert_eq!(state.borrow().chat(first).unwrap().state, RunState::Running);
        ui.invoke_select_row(first.to_string().into());
        assert_eq!(store.load().unwrap().chat(first).unwrap().messages.len(), 1);
        state.borrow_mut().chat_mut(first).unwrap().messages.extend([
            Message { role: "Codex".into(), text: "El proyecto usa **Rust + Slint**.\n\n## Implementación\n\n- **Tokio**: tareas en segundo plano.\n- **Slint**: interfaz gráfica en `ui/app.slint`.\n\n```rust\nfn main() {\n    println!(\"Hola\");\n}\n```".into() },
            Message { role: "Herramienta: write_file".into(), text: "{\"ok\":true,\"ref\":\"resultado-local\"}".into() },
            Message { role: "Vos".into(), text: "Perfecto. Conservá las respuestas de herramientas acotadas.\nMostrá los detalles solo cuando hagan falta.".into() },
        ]);
        refresh(&ui, &state.borrow());
        ui.window().set_size(slint::PhysicalSize::new(1280, 820));
        ui.show().unwrap();
        let snapshot = ui.window().take_snapshot().unwrap();
        assert_eq!(snapshot.width(), 1280);
        assert_eq!(snapshot.height(), 820);
        let destination = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-preview.png");
        image::save_buffer(
            destination,
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.invoke_open_settings();
        assert!(ui.get_settings_open());
        i_slint_backend_testing::testing_backend::mock_elapsed_time(160);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-settings-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_codex_path(
            temp.path()
                .join("missing.exe")
                .to_string_lossy()
                .as_ref()
                .into(),
        );
        ui.set_codex_enabled(true);
        ui.invoke_save_settings();
        assert!(ui.get_settings_open());
        assert!(!state.borrow().settings.codex.enabled);
        let client_fixture = temp.path().join("client.exe");
        std::fs::write(&client_fixture, "Path fixture only; never executed").unwrap();
        ui.set_codex_path(client_fixture.to_string_lossy().as_ref().into());
        ui.set_codex_concurrency(2);
        ui.set_codex_model("fixture-model".into());
        ui.set_codex_effort_index(3); // low
        ui.set_claude_path("".into());
        ui.set_claude_enabled(false);
        ui.invoke_save_settings();
        assert!(!ui.get_settings_open());
        assert_eq!(store.load().unwrap().settings.codex.concurrency, 2);
        assert_eq!(store.load().unwrap().settings.codex.reasoning_effort, "low");
        ui.set_codex_effort_index(0);
        ui.invoke_open_settings();
        assert_eq!(ui.get_codex_effort_index(), 3);
        ui.set_settings_open(false);
        assert!(
            !ui.get_codex_configured(),
            "Un modelo guardado sin sesión no habilita el selector"
        );
        ui.set_codex_authenticated(true);
        refresh(&ui, &state.borrow());
        assert!(ui.get_codex_configured());
        ui.invoke_open_settings();
        ui.invoke_close_settings();
        assert!(!ui.get_settings_open());
        assert_eq!(ui.get_draft(), "Borrador pendiente");
        assert!(
            !ui.get_codex_configured(),
            "Desconectar retira el agente del selector"
        );
        ui.set_codex_authenticated(true);
        {
            let mut state = state.borrow_mut();
            state.chat_mut(first).unwrap().state = RunState::Succeeded;
            state.settings.codex.models = vec![
                ModelChoice {
                    slug: "fixture-model".into(),
                    label: "Fixture A".into(),
                    ..Default::default()
                },
                ModelChoice {
                    slug: "fixture-second".into(),
                    label: "Fixture B".into(),
                    ..Default::default()
                },
            ];
            refresh(&ui, &state);
        }
        ui.invoke_pick_chat_model(1);
        ui.invoke_pick_chat_effort(5);
        assert_eq!(
            store.load().unwrap().chat(first).unwrap().model,
            "fixture-second"
        );
        assert_eq!(
            store
                .load()
                .unwrap()
                .chat(first)
                .unwrap()
                .reasoning_effort
                .as_deref(),
            Some("high")
        );
        ui.invoke_select_row(second.to_string().into());
        assert_eq!(ui.get_chat_model_index(), 0);
        assert_eq!(ui.get_chat_effort_index(), 3);
        ui.invoke_select_row(first.to_string().into());
        assert_eq!(ui.get_chat_model_index(), 1);
        assert_eq!(ui.get_chat_effort_index(), 5);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(160);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-chat-controls-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        for event in [
            Event::GenerationStarted { chat: second },
            Event::Text {
                chat: second,
                delta: "Respuesta parcial".into(),
            },
            Event::ResponseText {
                chat: second,
                text: "Respuesta completa sin duplicación.".into(),
            },
        ] {
            ui.invoke_agent_event(serde_json::to_string(&event).unwrap().into());
        }
        assert_eq!(state.borrow().selected_chat, Some(first));
        assert_eq!(ui.get_draft(), "Borrador pendiente");
        let mut recovered = store.load().unwrap();
        assert!(!recovered.chat(second).unwrap().history_loaded);
        store.hydrate(&mut recovered, second).unwrap();
        assert_eq!(
            recovered
                .chat(second)
                .unwrap()
                .messages
                .last()
                .unwrap()
                .text,
            "Respuesta completa sin duplicación."
        );
        let quota = ascurra_harness::quota::AccountUsage::parse(
            &serde_json::json!({"plan_type":"plus", "rate_limit": {
                "primary_window":{"used_percent":25,"limit_window_seconds":18000},
                "secondary_window":{"used_percent":70,"limit_window_seconds":604800}
            }}),
        )
        .unwrap();
        ui.set_quota_windows(ModelRc::new(VecModel::from(
            quota
                .windows
                .iter()
                .map(|window| QuotaRow {
                    caption: window.caption(quota.checked_at).into(),
                    remaining: window.remaining_percent as f32,
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_quota_status("Datos de prueba · no son el uso de tu cuenta.".into());
        assert_eq!(ui.get_quota_windows().row_count(), 2);
        assert_eq!(ui.get_quota_windows().row_data(0).unwrap().remaining, 75.0);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-quota-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        state.borrow_mut().chat_mut(first).unwrap().state = RunState::Running;
        refresh(&ui, &state.borrow());
        ui.invoke_agent_event(
            serde_json::to_string(&Event::GenerationStarted { chat: first })
                .unwrap()
                .into(),
        );
        let stable_navigation = ui.get_navigation();
        let stable_tabs = ui.get_tabs();
        for _ in 0..80 {
            ui.invoke_agent_event(
                serde_json::to_string(&Event::Text {
                    chat: first,
                    delta: "Línea de respuesta en streaming.\n".into(),
                })
                .unwrap()
                .into(),
            );
        }
        assert_eq!(
            ui.get_messages().iter().last().unwrap().body,
            "",
            "Los fragmentos deben agruparse antes de renderizar"
        );
        assert!(std::ptr::eq(
            stable_navigation.as_any(),
            ui.get_navigation().as_any()
        ));
        assert!(std::ptr::eq(stable_tabs.as_any(), ui.get_tabs().as_any()));
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let _ = ui.window().take_snapshot().unwrap();
        let before = ui.get_chat_scroll_bottom();
        assert!(
            before < -100.0,
            "La respuesta debe exceder la altura visible"
        );
        assert!(
            (ui.get_chat_scroll_offset() - before).abs() < 2.0,
            "El streaming debe seguir el final"
        );
        let stable_model = ui.get_messages();
        ui.invoke_agent_event(
            serde_json::to_string(&Event::Text {
                chat: first,
                delta: "Más contenido nuevo.\n".repeat(30),
            })
            .unwrap()
            .into(),
        );
        assert!(
            std::ptr::eq(stable_model.as_any(), ui.get_messages().as_any()),
            "El streaming debe actualizar filas sin recrear la lista"
        );
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(40);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_chat_follow_pending(), "Follow waits for virtualized layout to settle");
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_chat_scroll_bottom() < before);
        assert!((ui.get_chat_scroll_offset() - ui.get_chat_scroll_bottom()).abs() < 2.0);
        ui.set_git_project("".into());
        let before_panel = ui.window().take_snapshot().unwrap();
        // The selected panel name changes; the title and configuration button stay fixed.
        let static_header = |snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            let width = snapshot.width() as usize;
            (14..46)
                .flat_map(|row| {
                    let pixels = &snapshot.as_bytes()[row * width * 4..(row + 1) * width * 4];
                    pixels[..400 * 4]
                        .iter()
                        .chain(pixels[(width - 330) * 4..(width - 220) * 4].iter())
                        .copied()
                })
                .collect::<Vec<_>>()
        };
        ui.set_git_open(true);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(110);
        let during_panel = ui.window().take_snapshot().unwrap();
        assert_eq!(
            static_header(&before_panel) == static_header(&during_panel),
            true,
            "Los botones de la cabecera no se desplazan durante la transición"
        );
        i_slint_backend_testing::testing_backend::mock_elapsed_time(140);
        let after_panel = ui.window().take_snapshot().unwrap();
        assert_eq!(
            static_header(&before_panel) == static_header(&after_panel),
            true,
            "La cabecera conserva su posición con el panel abierto"
        );
        ui.set_git_busy(false);
        ui.set_git_known(true);
        ui.set_git_repository(true);
        ui.set_git_remote(true);
        ui.set_git_has_staged(true);
        ui.set_git_branches(ModelRc::new(VecModel::from(vec![
            "main".into(),
            "feature".into(),
        ])));
        ui.set_git_branch_index(0);
        ui.set_git_dirty(true);
        ui.set_git_status("".into());
        ui.set_git_files(ModelRc::new(VecModel::from(vec![
            GitRow {
                header: true,
                prepared: true,
                name: "Preparados (1)".into(),
                ..Default::default()
            },
            GitRow {
                path: "ui/app.slint".into(),
                target: "ui/app.slint".into(),
                name: "app.slint".into(),
                directory: "ui".into(),
                staged: true,
                prepared: true,
                code: "M".into(),
                added: "+12".into(),
                removed: "−4".into(),
                ..Default::default()
            },
            GitRow {
                header: true,
                name: "Cambios (1)".into(),
                ..Default::default()
            },
            GitRow {
                path: "src/main.rs".into(),
                target: "src/main.rs".into(),
                name: "main.rs".into(),
                directory: "src".into(),
                unstaged: true,
                code: "M".into(),
                added: "+25".into(),
                removed: "−8".into(),
                ..Default::default()
            },
        ])));
        ui.set_context_summary(
            "Última entrada · 25.0% · 25000 / 100000 tokens · datos de prueba".into(),
        );
        ui.set_context_fraction(0.25);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-git-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        let git_actions = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
        let recorded = git_actions.clone();
        ui.on_git_action(move |kind, target| {
            recorded
                .borrow_mut()
                .push((kind.to_string(), target.to_string()))
        });
        let git_diffs = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
        let recorded = git_diffs.clone();
        ui.on_git_file_diff(move |target, prepared| {
            recorded.borrow_mut().push((target.to_string(), prepared))
        });
        let click_git_row = |x: f32, y: f32| {
            for pressed in [true, false] {
                let position = slint::LogicalPosition::new(x, y);
                ui.window().dispatch_event(if pressed {
                    slint::platform::WindowEvent::PointerPressed {
                        position,
                        button: slint::platform::PointerEventButton::Left,
                    }
                } else {
                    slint::platform::WindowEvent::PointerReleased {
                        position,
                        button: slint::platform::PointerEventButton::Left,
                    }
                });
            }
        };
        let checkbox_x = ui.window().size().width as f32 / ui.window().scale_factor()
            - ui.get_side_panel_visible_width()
            - 14.0
            + 31.0;
        // Locate the prepared checkbox in the rendered fixture rather than
        // coupling hit tests to the optional status/toolbar heights.
        let checkbox_column = checkbox_x.round() as usize;
        let checkbox_pixels = (110..snapshot.height() as usize - 150)
            .filter(|y| {
                let offset = (*y * snapshot.width() as usize + checkbox_column) * 4;
                snapshot.as_bytes()[offset..offset + 3] == [0x65, 0x57, 0xce]
            })
            .collect::<Vec<_>>();
        let checkbox_y =
            (*checkbox_pixels.first().unwrap() + *checkbox_pixels.last().unwrap()) as f32 / 2.0;
        click_git_row(checkbox_x, checkbox_y);
        click_git_row(checkbox_x, checkbox_y + 70.0);
        assert_eq!(
            *git_actions.borrow(),
            vec![
                ("unstage".to_string(), "ui/app.slint".to_string()),
                ("stage".to_string(), "src/main.rs".to_string()),
            ],
            "Los checkboxes deben retirar o preparar exclusivamente el archivo elegido"
        );
        let select_all_x = ui.window().size().width as f32 / ui.window().scale_factor() - 70.0;
        click_git_row(select_all_x, checkbox_y + 35.0);
        assert_eq!(
            git_actions.borrow().last(),
            Some(&("stage-all".to_string(), String::new()))
        );
        let actions_before = git_actions.borrow().len();
        ui.set_git_busy(true);
        click_git_row(select_all_x, checkbox_y + 35.0);
        assert_eq!(
            git_actions.borrow().len(),
            actions_before,
            "Seleccionar todos se bloquea durante una operación Git"
        );
        ui.set_git_busy(false);
        click_git_row(checkbox_x + 70.0, checkbox_y);
        assert_eq!(
            *git_diffs.borrow(),
            vec![("ui/app.slint".to_string(), true)],
            "Pulsar el nombre abre su diff sin cambiar la selección para commit"
        );
        let pending_diff = git_ui::pending_diff_tab(&ui, "", "src/main.rs", false, Some(false));
        assert!(ui.get_file_preview_open());
        assert_eq!(ui.get_file_preview_status(), "Leyendo diferencias…");
        assert!(!ui.get_git_action_message().contains("Leyendo diferencias"));
        let patch = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-línea anterior\n+línea agregada\n";
        git_ui::complete_diff_tab(&ui, &pending_diff, Ok(patch.into()));
        assert_eq!(ui.get_file_preview_text(), patch);
        assert_eq!(ui.get_file_preview_lines().row_data(4).unwrap().kind, 2);
        assert_eq!(ui.get_file_preview_lines().row_data(5).unwrap().kind, 1);
        let screenshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-git-diff-preview.png"),
            screenshot.as_bytes(),
            screenshot.width(),
            screenshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_git_remote_url("https://example.invalid/repo.git".into());
        ui.set_git_user_name("Autor del proyecto".into());
        ui.set_git_user_email("autor@example.invalid".into());
        ui.set_git_settings_open(true);
        let screenshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-git-settings-preview.png"),
            screenshot.as_bytes(),
            screenshot.width(),
            screenshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_git_settings_open(false);
        assert!(ui.get_file_preview_status().contains("Solo lectura"));
        let other_tab = FileTabRow {
            project: "".into(),
            path: "other.txt".into(),
            title: "Otro".into(),
            text: "Otro archivo".into(),
            status: "Solo lectura".into(),
        };
        let mut file_tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
        file_tabs.push(other_tab.clone());
        ui.set_file_tabs(ModelRc::new(VecModel::from(file_tabs)));
        ui.invoke_select_file_tab(other_tab.project.clone(), other_tab.path.clone());
        git_ui::complete_diff_tab(&ui, &pending_diff, Err("Error de lectura de prueba".into()));
        assert_eq!(ui.get_file_preview_path(), other_tab.path);
        assert_eq!(
            ui.get_file_preview_text(),
            "Otro archivo",
            "Completar un diff inactivo no debe robar el foco"
        );
        assert!(
            ui.get_file_tabs()
                .iter()
                .any(|tab| tab.path == pending_diff.path && tab.text.contains("Error de lectura"))
        );
        ui.invoke_close_file_tab(pending_diff.project.clone(), pending_diff.path.clone());
        git_ui::complete_diff_tab(&ui, &pending_diff, Ok("resultado tardío".into()));
        assert!(
            !ui.get_file_tabs()
                .iter()
                .any(|tab| tab.path == pending_diff.path),
            "Completar un diff cerrado no debe reabrirlo"
        );
        ui.invoke_close_file_tab(other_tab.project, other_tab.path);
        ui.set_git_history(ModelRc::new(VecModel::from(vec![GitCommitRow {
            id: "0123456789012345678901234567890123456789".into(),
            title: "Agregar panel de Git con historial".into(),
            detail: "01234567 · Gabriel · 2026-10-09".into(),
        }])));
        ui.set_git_commit_message("Resumen del commit\n\nDetalle pendiente".into());
        ui.set_git_history_view(true);
        assert_eq!(
            ui.get_git_commit_message(),
            "Resumen del commit\n\nDetalle pendiente"
        );
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(150);
        let history = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-git-history-preview.png"),
            history.as_bytes(),
            history.width(),
            history.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_git_history_view(false);
        assert!(ui.get_git_commit_message().contains("Detalle pendiente"));
        assert_eq!(ui.get_draft(), "Borrador pendiente");
        ui.set_files_open(true);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(
            !ui.get_git_open(),
            "Los paneles comparten el espacio lateral"
        );
        let entries = vec![
            ascurra_harness::explorer::Entry {
                path: "src".into(),
                name: "src".into(),
                directory: true,
                expanded: true,
                depth: 0,
            },
            ascurra_harness::explorer::Entry {
                path: "src/main.rs".into(),
                name: "main.rs".into(),
                directory: false,
                expanded: false,
                depth: 1,
            },
            ascurra_harness::explorer::Entry {
                path: "Cargo.toml".into(),
                name: "Cargo.toml".into(),
                directory: false,
                expanded: false,
                depth: 0,
            },
        ];
        ui.invoke_explorer_event(
            serde_json::json!({"root":"","path":"","kind":"root","result":entries})
                .to_string()
                .into(),
        );
        assert_eq!(ui.get_explorer_rows().row_count(), 3);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-files-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.invoke_select_file("src".into());
        assert_eq!(
            ui.get_explorer_rows().row_count(),
            2,
            "Cerrar una carpeta retira solo sus descendientes"
        );
        let history = state.borrow().chat(first).unwrap().wire_history.clone();
        ui.set_file_tabs(ModelRc::new(VecModel::from(vec![FileTabRow {
            project: "".into(),
            path: "src/main.rs".into(),
            title: "main.rs".into(),
            text: "".into(),
            status: "Leyendo archivo…".into(),
        }])));
        ui.set_file_preview_project("".into());
        ui.set_file_preview_path("src/main.rs".into());
        ui.set_file_preview_open(true);
        ui.invoke_explorer_event(serde_json::json!({"root":"","path":"src/main.rs","kind":"preview","result":{"text":"fn main() {\n    println!(\"Vista previa local\");\n}\n","truncated":false}}).to_string().into());
        assert!(ui.get_file_preview_text().contains("Vista previa local"));
        assert_eq!(
            state.borrow().chat(first).unwrap().wire_history,
            history,
            "Leer desde el explorador no cambia el contexto del agente"
        );
        ui.invoke_explorer_event(serde_json::json!({"root":"otro-proyecto","path":"src/main.rs","kind":"preview","result":{"text":"Contenido de otro proyecto","truncated":false}}).to_string().into());
        assert!(!ui.get_file_preview_text().contains("otro proyecto"));
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-file-content-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        let draft = ui.get_draft();
        ui.invoke_select_row(first.to_string().into());
        assert!(!ui.get_file_preview_open());
        assert_eq!(ui.get_file_tabs().row_count(), 1);
        ui.invoke_select_file_tab("".into(), "src/main.rs".into());
        assert!(ui.get_file_preview_open());
        assert!(ui.get_file_preview_text().contains("Vista previa local"));
        ui.invoke_select_file_tab("".into(), "src/main.rs".into());
        assert_eq!(
            ui.get_file_tabs().row_count(),
            1,
            "El archivo abierto se reutiliza"
        );
        assert_eq!(
            ui.get_draft(),
            draft,
            "Alternar archivos conserva el borrador del chat"
        );
        let large_file = (0..12_000)
            .map(|i| format!("fila {i}\n"))
            .collect::<String>();
        ui.invoke_explorer_event(serde_json::json!({"root":"","path":"src/main.rs","kind":"preview","result":{"text":large_file,"truncated":false}}).to_string().into());
        assert_eq!(ui.get_file_preview_lines().row_count(), 12_001);
        let before_scroll = ui.window().take_snapshot().unwrap();
        let scroll_started = std::time::Instant::now();
        for _ in 0..10 {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(600.0, 400.0),
                    delta_x: 0.0,
                    delta_y: -320.0,
                });
            i_slint_backend_testing::testing_backend::mock_elapsed_time(20);
            let _ = ui.window().take_snapshot().unwrap();
        }
        let after_scroll = ui.window().take_snapshot().unwrap();
        assert_ne!(
            before_scroll.as_bytes(),
            after_scroll.as_bytes(),
            "El visor debe desplazar el archivo grande"
        );
        eprintln!(
            "Visor de 12.000 líneas: diez desplazamientos y renders en {:?}",
            scroll_started.elapsed()
        );
        ui.invoke_close_file_tab("".into(), "src/main.rs".into());
        assert!(!ui.get_file_preview_open());
        assert_eq!(ui.get_file_tabs().row_count(), 0);
        ui.invoke_explorer_event(serde_json::json!({"root":"","path":"src/main.rs","kind":"preview","result":{"text":"Respuesta tardía","truncated":false}}).to_string().into());
        assert_eq!(
            ui.get_file_tabs().row_count(),
            0,
            "Una lectura tardía no reabre una pestaña cerrada"
        );
        ui.set_files_open(false);
        let project_id = state.borrow().projects[0].id;
        let claude_chat = state.borrow_mut().add_chat(project_id).unwrap();
        {
            let mut state = state.borrow_mut();
            state.settings.claude.enabled = true;
            state.settings.claude.executable = "verified-client.exe".into();
        }
        refresh(&ui, &state.borrow());
        assert!(
            !ui.get_available_agents()
                .iter()
                .any(|label| label == "Claude"),
            "Unverified clients are not selectable"
        );
        ui.invoke_claude_auth_event(
            serde_json::json!({"ok":true,"path":"verified-client.exe","status":"Verified fixture"})
                .to_string()
                .into(),
        );
        let index = ui
            .get_available_agents()
            .iter()
            .position(|label| label == "Claude")
            .unwrap();
        ui.invoke_pick_chat_provider(index as i32);
        assert_eq!(
            state.borrow().chat(claude_chat).unwrap().provider,
            Provider::Claude
        );
        assert!(ui.get_selected_agent_configured());
        assert_eq!(ui.get_quota_provider(), 1, "Claude chat shows Claude quota");
        ui.set_claude_quota_windows(ModelRc::new(VecModel::from(vec![
            QuotaRow {
                caption: "5 horas - 75% libre - datos de prueba".into(),
                remaining: 75.0,
            },
            QuotaRow {
                caption: "Semanal - 20% libre - datos de prueba".into(),
                remaining: 20.0,
            },
        ])));
        ui.set_claude_quota_status("Datos de prueba, no cuota real".into());
        ui.set_quota_provider(0);
        assert_eq!(ui.get_quota_windows().row_count(), 2);
        ui.set_quota_provider(1);
        assert_eq!(ui.get_claude_quota_windows().row_count(), 2);
        assert!(!ui.get_chat_efforts().iter().any(|label| label == "minimal"));
        ui.invoke_agent_event(
            serde_json::to_string(&Event::Session {
                chat: claude_chat,
                session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            })
            .unwrap()
            .into(),
        );
        ui.invoke_agent_event(
            serde_json::to_string(&Event::GenerationStarted { chat: claude_chat })
                .unwrap()
                .into(),
        );
        ui.invoke_agent_event(
            serde_json::to_string(&Event::ResponseText {
                chat: claude_chat,
                text: "**Claude** con herramientas del harness".into(),
            })
            .unwrap()
            .into(),
        );
        assert_eq!(ui.get_messages().row_data(0).unwrap().role, "Claude");
        assert_eq!(
            ui.get_messages()
                .row_data(0)
                .unwrap()
                .blocks
                .row_data(0)
                .unwrap()
                .rendered,
            render_message("Codex", "**Claude** con herramientas del harness")
        );
        assert!(
            !ui.get_provider_editable(),
            "Existing conversations keep their provider"
        );
        ui.invoke_pick_chat_provider(0);
        assert_eq!(
            state.borrow().chat(claude_chat).unwrap().provider,
            Provider::Claude
        );
        assert_eq!(
            store
                .load()
                .unwrap()
                .chat(claude_chat)
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        ui.set_codex_authenticated(true); // Render the two-provider pager.
        refresh(&ui, &state.borrow());
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(200);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-claude-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        let refreshes = Rc::new(RefCell::new(Vec::new()));
        let record = refreshes.clone();
        ui.on_refresh_quota(move |force| {
            if force {
                record.borrow_mut().push(Provider::Codex);
            }
        });
        let record = refreshes.clone();
        ui.on_refresh_claude_quota(move |force| {
            if force {
                record.borrow_mut().push(Provider::Claude);
            }
        });
        for id in [first, claude_chat] {
            ui.invoke_agent_event(
                serde_json::to_string(&Event::Finished {
                    chat: id,
                    state: RunState::Succeeded,
                    message: "Done".into(),
                    history: Vec::new(),
                    incomplete_usage: false,
                })
                .unwrap()
                .into(),
            );
        }
        assert_eq!(
            *refreshes.borrow(),
            vec![Provider::Codex, Provider::Claude],
            "Each completed turn refreshes its provider immediately"
        );
        // Exercise the panel without touching a real device or starting ADB.
        ui.on_refresh_logcat_devices(|| {});
        ui.set_logcat_devices(ModelRc::new(VecModel::from(vec![
            "Pixel 8 · emulator-5554 · device".into(),
        ])));
        ui.set_logcat_device_ready(true);
        ui.set_logcat_status("En vivo · 300 líneas · datos de prueba".into());
        ui.set_logcat_lines(ModelRc::new(VecModel::from(
            (0..300)
                .map(|index| LogcatRow {
                    text: format!(
                        "{}/Harness( 123): evento {index}",
                        if index % 5 == 0 { "E" } else { "I" }
                    )
                    .into(),
                    level: if index % 5 == 0 { "E" } else { "I" }.into(),
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_logcat_open(true);
        ui.set_logcat_running(true);
        for _ in 0..3 {
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
            let _ = ui.window().take_snapshot().unwrap();
        }
        assert!(!ui.get_git_open() && !ui.get_files_open());
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-logcat-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        let header = 14 * snapshot.width() as usize * 4..46 * snapshot.width() as usize * 4;
        let header_pixels = snapshot.as_bytes()[header.clone()].to_vec();
        let drag_panel = |delta: f32| {
            let initial_panel = ui.get_side_panel_visible_width();
            let initial_workspace = ui.get_workspace_visible_width();
            let x = ui.window().size().width as f32 / ui.window().scale_factor()
                - 14.0
                - ui.get_side_panel_visible_width();
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                    position: slint::LogicalPosition::new(x, 400.0),
                    button: slint::platform::PointerEventButton::Left,
                });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(x + delta, 400.0),
                });
            i_slint_backend_testing::testing_backend::mock_elapsed_time(30);
            let _ = ui.window().take_snapshot().unwrap();
            assert!(
                ((initial_workspace - ui.get_workspace_visible_width())
                    - (ui.get_side_panel_visible_width() - initial_panel))
                    .abs()
                    < 2.0,
                "El área de trabajo debe ajustarse durante el arrastre, antes de soltar"
            );
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(x + delta, 400.0),
                    button: slint::platform::PointerEventButton::Left,
                });
            i_slint_backend_testing::testing_backend::mock_elapsed_time(30);
            let _ = ui.window().take_snapshot().unwrap();
        };
        let original_tab_width = ui.get_tab_visible_width();
        drag_panel(-120.0);
        assert!(
            ui.get_tab_visible_width() < original_tab_width,
            "Las pestañas deben reducir su ancho junto con el área de trabajo"
        );
        assert!(
            (ui.get_side_panel_visible_width() - 430.0).abs() < 2.0,
            "El borde debe redimensionar el panel"
        );
        let resized = ui.window().take_snapshot().unwrap();
        assert_eq!(
            &resized.as_bytes()[header],
            header_pixels.as_slice(),
            "Redimensionar conserva la cabecera de la ventana"
        );
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-panel-resized-preview.png"),
            resized.as_bytes(),
            resized.width(),
            resized.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        let narrow_tab_width = ui.get_tab_visible_width();
        drag_panel(1000.0);
        assert!(
            ui.get_tab_visible_width() > narrow_tab_width,
            "Las pestañas deben recuperar ancho al reducir el panel"
        );
        assert_eq!(
            ui.get_side_panel_visible_width(),
            280.0,
            "El panel debe respetar su mínimo"
        );
        drag_panel(-2000.0);
        assert_eq!(
            ui.get_side_panel_visible_width(),
            ui.get_side_panel_max_width(),
            "El panel debe respetar su máximo y reservar espacio para trabajar"
        );
        ui.on_refresh_git(|| {});
        ui.on_refresh_explorer(|_| {});
        ui.set_git_open(true);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        assert!(!ui.get_logcat_open() && !ui.get_logcat_running());
        assert_eq!(
            ui.get_side_panel_visible_width(),
            ui.get_side_panel_max_width()
        );
        ui.set_files_open(true);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        assert!(!ui.get_git_open());
        assert_eq!(
            ui.get_side_panel_visible_width(),
            ui.get_side_panel_max_width()
        );
        ui.set_side_panel_width(310.0);
        ui.set_files_open(false);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);

        let _ = ui.window().take_snapshot().unwrap();
        let before_projects_resize = ui.get_workspace_visible_width();
        let before_projects_tabs = ui.get_tabs_viewport_width();
        let divider_x = ui.get_projects_panel_visible_width() + 20.0;
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(divider_x, 400.0),
                button: slint::platform::PointerEventButton::Left,
            });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(divider_x + 100.0, 400.0),
            });
        let _ = ui.window().take_snapshot().unwrap();
        assert!((ui.get_projects_panel_visible_width() - 380.0).abs() < 2.0);
        assert!((before_projects_resize - ui.get_workspace_visible_width() - 100.0).abs() < 2.0);
        assert!((before_projects_tabs - ui.get_tabs_viewport_width() - 100.0).abs() < 2.0);
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(divider_x + 100.0, 400.0),
                button: slint::platform::PointerEventButton::Left,
            });
        ui.set_projects_panel_width(10.0);
        let _ = ui.window().take_snapshot().unwrap();
        assert_eq!(ui.get_projects_panel_visible_width(), 260.0);
        ui.set_projects_panel_width(2000.0);
        let _ = ui.window().take_snapshot().unwrap();
        assert_eq!(ui.get_projects_panel_visible_width(), 560.0);
        ui.set_projects_panel_width(280.0);
        let _ = ui.window().take_snapshot().unwrap();
        ui.invoke_select_panel(1);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_git_open());
        assert_eq!(ui.get_active_panel_index(), 1);
        ui.invoke_select_panel(2);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_files_open() && !ui.get_git_open());
        assert_eq!(ui.get_active_panel_index(), 2);
        ui.invoke_select_panel(0);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(!ui.get_files_open() && !ui.get_git_open());
        assert_eq!(ui.get_active_panel_index(), 0);
        let expanded_workspace = ui.get_workspace_visible_width();
        let selected_before_compact = state.borrow().selected_chat;
        ui.set_projects_compact(true);
        let compact_snapshot = ui.window().take_snapshot().unwrap();
        assert_eq!(ui.get_projects_panel_visible_width(), 72.0);
        assert!((ui.get_workspace_visible_width() - expanded_workspace - 208.0).abs() < 2.0);
        assert_eq!(state.borrow().selected_chat, selected_before_compact);
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/ui-projects-compact-preview.png"),
            compact_snapshot.as_bytes(),
            compact_snapshot.width(),
            compact_snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_projects_compact(false);
        let _ = ui.window().take_snapshot().unwrap();
        assert_eq!(ui.get_projects_panel_visible_width(), 280.0);
        let panel_menu_x = ui.window().size().width as f32 / ui.window().scale_factor() - 100.0;
        click_git_row(panel_menu_x, 30.0);
        let panel_menu = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-panel-menu-preview.png"),
            panel_menu.as_bytes(),
            panel_menu.width(),
            panel_menu.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        click_git_row(panel_menu_x, 110.0);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(
            ui.get_git_open(),
            "El menú con iconos debe abrir el panel seleccionado"
        );
        ui.invoke_select_panel(0);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        let _ = ui.window().take_snapshot().unwrap();

        // A long transcript retains exact row heights, but only realizes text
        // and bubbles near the viewport after its initial measurement pass.
        ui.set_git_open(false);
        ui.set_files_open(false);
        ui.set_file_preview_open(false);
        ui.set_chat_running(false);
        let mounted = Rc::new(RefCell::new(std::collections::HashMap::new()));
        let observed = mounted.clone();
        ui.on_chat_bubble_visibility(move |index, active| {
            observed.borrow_mut().insert(index, active);
        });
        ui.set_messages(ModelRc::new(VecModel::from((0..200).map(|index| {
            let body = if index % 20 == 0 {
                format!("Mensaje {index}\n\n| Archivo | Cambio |\n| --- | --- |\n| `src/main.rs` | **Actualizado** con contenido largo para comprobar el ajuste de líneas y la altura de la tabla. |\n| `Cargo.toml` | Dependencias |")
            } else {
                format!("Mensaje {index}: **contenido** de una conversación larga.\nOtra línea para comprobar la altura.")
            };
            MessageRow { role: "Codex".into(), blocks: markdown_ui::blocks("Codex", &body), summary: body.lines().next().unwrap_or_default().into(), body: body.into() }
        }).collect::<Vec<_>>())));
        for _ in 0..4 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(600.0, 350.0),
                delta_x: 0.0,
                delta_y: 100_000.0,
            });
        for _ in 0..4 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let active = || mounted.borrow().values().filter(|active| **active).count();
        assert!(
            active() > 0 && active() < 25,
            "Solo deben montarse burbujas cercanas al viewport: {}",
            active()
        );
        let initially_mounted = active();
        let initial = mounted
            .borrow()
            .iter()
            .filter(|(_, active)| **active)
            .map(|(index, _)| *index)
            .min()
            .unwrap();
        let bottom = ui.get_chat_scroll_bottom();
        // ListView may destroy or recycle delegates; only observe this scroll batch.
        mounted.borrow_mut().clear();
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(600.0, 350.0),
                delta_x: 0.0,
                delta_y: -4000.0,
            });
        for _ in 0..4 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let shifted = mounted
            .borrow()
            .iter()
            .filter(|(_, active)| **active)
            .map(|(index, _)| *index)
            .min()
            .unwrap();
        assert!(
            shifted > initial,
            "El scroll debe montar otro tramo del historial"
        );
        assert!(active() > 0 && active() < 25);
        assert!(
            ui.get_chat_scroll_bottom().is_finite() && ui.get_chat_scroll_bottom() < 0.0,
            "La lista virtualizada debe conservar un rango de scroll válido"
        );
        std::fs::write(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-virtualization-check.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "messages":200, "mounted_at_top":initially_mounted,
                "mounted_after_scroll":active(), "scroll_bottom_before":bottom,
                "scroll_bottom_after":ui.get_chat_scroll_bottom(),
            }))
            .unwrap(),
        )
        .unwrap();
        ui.window().set_size(slint::PhysicalSize::new(900, 820));
        for _ in 0..4 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        assert!(
            active() > 0 && active() < 25,
            "El nuevo ancho debe volver a medir y liberar las burbujas lejanas"
        );
        // Check the table at a narrow window width with wrapped cells.
        let table_body = "### Tecnología del proyecto\n\nEl harness usa **Rust + Slint**.\n\n| Área | Tecnología | Evidencia |\n| :--- | :---: | --- |\n| Lenguaje | **Rust 2024** | `Cargo.toml` define la edición del proyecto. |\n| Interfaz | **Slint 1.18.1** | `ui/app.slint` contiene los componentes y estilos. |\n| Persistencia | JSON | `src/storage.rs` guarda mensajes localmente. |\n\n### Verificación\n\n- **Tabla interpretada**, con formato dentro de las celdas.\n- Texto posterior conservado.";
        ui.set_messages(ModelRc::new(VecModel::from(vec![MessageRow {
            role: "Codex".into(),
            summary: table_body.lines().next().unwrap_or_default().into(), body: table_body.into(),
            blocks: markdown_ui::blocks("Codex", table_body),
        }])));
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let preview = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-markdown-table-preview.png",
            preview.as_bytes(),
            preview.width(),
            preview.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();

        // Exercise queue completion and token persistence using provider events,
        // without launching a real agent or consuming account quota.
        let queue_project = state
            .borrow_mut()
            .add_project("Cola de tareas".into(), "fixture-queue".into());
        let mut queue_tasks = Vec::new();
        for title in [
            "Implementar navegación",
            "Agregar pruebas",
            "Actualizar documentación",
        ] {
            queue_tasks.push(
                state
                    .borrow_mut()
                    .save_task(
                        queue_project,
                        None,
                        title.into(),
                        "Completar el objetivo de la tarea.".into(),
                        "La verificación debe pasar.".into(),
                        ascurra_harness::domain::TaskSeverity::High,
                    )
                    .unwrap(),
            );
        }
        ui.set_codex_authenticated(false);
        state.borrow_mut().settings.codex.enabled = false;
        ui.set_claude_authenticated(false);
        state.borrow_mut().settings.claude.enabled = false;
        ui.invoke_select_row(format!("p{queue_project}").into());
        ui.set_project_tasks_view(true);
        ui.invoke_run_task_queue();
        assert!(
            state
                .borrow()
                .projects
                .iter()
                .find(|p| p.id == queue_project)
                .unwrap()
                .queue
                .is_none(),
            "Sin agente configurado no se inicia ni se crean chats"
        );
        let queue_config = AgentSettings {
            enabled: true,
            model: "Fixture".into(),
            ..Default::default()
        };
        state
            .borrow_mut()
            .begin_task_queue(queue_project, Provider::Codex, queue_config)
            .unwrap();
        let queued = state
            .borrow_mut()
            .prepare_queued_job(queue_project)
            .unwrap()
            .unwrap();
        store.save(&state.borrow()).unwrap();
        let advances = Rc::new(RefCell::new(Vec::<String>::new()));
        let recorded = advances.clone();
        ui.on_advance_task_queue(move |project| recorded.borrow_mut().push(project.to_string()));
        let reported = ascurra_harness::domain::UsageRecord {
            request_id: "queued-request".into(),
            model: "Fixture".into(),
            provider: Provider::Codex,
            usage: ascurra_harness::domain::TokenUsage {
                input: Some(300),
                output: Some(12),
                cache_read: Some(30),
                ..Default::default()
            },
            context_input: Some(300),
            context_window: None,
        };
        for _ in 0..2 {
            ui.invoke_agent_event(
                serde_json::to_string(&Event::Usage {
                    chat: queued.chat.id,
                    record: reported.clone(),
                })
                .unwrap()
                .into(),
            );
        }
        assert_eq!(
            ui.get_tasks_progress().row_data(0).unwrap().tokens,
            "Entrada 300 · Salida 12 · Caché 30"
        );
        assert!(
            advances.borrow().is_empty(),
            "Un reporte de uso no inicia la siguiente tarea"
        );
        ui.window().set_size(slint::PhysicalSize::new(1280, 820));
        let _ = ui.window().take_snapshot().unwrap();
        ui.set_project_tasks_view(true);
        let queued_view = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-task-queue-preview.png"),
            queued_view.as_bytes(),
            queued_view.width(),
            queued_view.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.invoke_agent_event(
            serde_json::to_string(&Event::Finished {
                chat: queued.chat.id,
                state: RunState::Succeeded,
                message: "Fixture completada".into(),
                history: Vec::new(),
                incomplete_usage: false,
            })
            .unwrap()
            .into(),
        );
        assert_eq!(*advances.borrow(), vec![queue_project.to_string()]);
        assert_eq!(ui.get_tasks_done().row_count(), 1);
        assert_eq!(
            ui.get_tasks_done().row_data(0).unwrap().chat,
            queued.chat.id.to_string()
        );
        let next = state
            .borrow_mut()
            .prepare_queued_job(queue_project)
            .unwrap()
            .unwrap();
        ui.invoke_agent_event(
            serde_json::to_string(&Event::Finished {
                chat: next.chat.id,
                state: RunState::Failed,
                message: "Fixture fallida".into(),
                history: Vec::new(),
                incomplete_usage: true,
            })
            .unwrap()
            .into(),
        );
        assert!(!ui.get_queue_running());
        assert!(
            state
                .borrow_mut()
                .prepare_queued_job(queue_project)
                .unwrap()
                .is_none()
        );
        let persisted = store.load().unwrap();
        let project = persisted
            .projects
            .iter()
            .find(|p| p.id == queue_project)
            .unwrap();
        assert_eq!(project.tasks[0].runs[0].usage.len(), 1);
        assert_eq!(project.tasks[0].token_usage().input, Some(300));
        assert_eq!(
            project.tasks[1].status,
            ascurra_harness::domain::TaskStatus::Todo
        );
        assert!(project.tasks[1].runs[0].usage_incomplete);
        // Failed chats offer recovery and retain the composer when retry cannot
        // start (this UI fixture has no authenticated provider).
        {
            let mut state = state.borrow_mut();
            state.selected_project = None;
            state.selected_chat = Some(next.chat.id);
            store.hydrate(&mut state, next.chat.id).unwrap();
            let chat = state.chat_mut(next.chat.id).unwrap();
            chat.draft = "Borrador que debe conservarse".into();
            refresh(&ui, &state);
        }
        assert!(ui.get_can_retry_message());
        let before_retry = state.borrow().chat(next.chat.id).unwrap().clone();
        ui.invoke_retry_message();
        let after_retry = state.borrow().chat(next.chat.id).unwrap().clone();
        assert_eq!(after_retry.messages.len(), before_retry.messages.len());
        assert_eq!(after_retry.wire_history, before_retry.wire_history);
        assert_eq!(after_retry.draft, before_retry.draft);
        assert_eq!(after_retry.state, RunState::Failed);
        ui.show().unwrap();
        let retry_snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-retry-preview.png",
            retry_snapshot.as_bytes(),
            retry_snapshot.width(),
            retry_snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        {
            let mut state = state.borrow_mut();
            state.chat_mut(next.chat.id).unwrap().state = RunState::Running;
            refresh(&ui, &state);
        }
        assert!(!ui.get_can_retry_message());
        ui.hide().unwrap();
        // Diagrams are computed from schema text only, without an agent or database.
        ui.on_refresh_database(|_| {});
        ui.set_database_open(true);
        ui.set_side_panel_width(620.0);
        ui.set_database_zoom(0.8);
        ui.set_database_schema("prisma/schema.prisma".into());
        let diagram = ascurra_harness::prisma::parse(r#"
            model User { id Int @id email String @unique posts Post[] }
            model Post { id Int @id title String authorId Int author User @relation(fields: [authorId], references: [id]) }
            enum Status { DRAFT PUBLISHED }
        "#).unwrap();
        prisma_ui::display(&ui, diagram);
        assert_eq!(ui.get_database_entities().row_count(), 3);
        assert_eq!(ui.get_database_relations().row_count(), 1);
        assert!(
            ui.get_database_relations()
                .row_data(0)
                .unwrap()
                .detail
                .contains("authorId")
        );
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-prisma-preview.png",
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        assert!(ui.get_database_open());
        assert!(!ui.get_git_open() && !ui.get_files_open() && !ui.get_logcat_open());
        let before = ui.get_workspace_visible_width();
        ui.set_side_panel_width(400.0);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_workspace_visible_width() > before);
        ui.set_database_relations_view(true);
        let _ = ui.window().take_snapshot().unwrap();
        ui.set_database_open(false);
        ui.on_refresh_docker(|| {});
        ui.on_docker_action(|_, _| {});
        ui.set_docker_open(true);
        docker_ui::display(
            &ui,
            ascurra_harness::docker::Snapshot {
                compose: Some("compose.yaml".into()),
                containers: vec![ascurra_harness::docker::Container {
                    id: "a".repeat(64),
                    name: "/project-db-1".into(),
                    image: "postgres:17".into(),
                    state: "running".into(),
                    health: "healthy".into(),
                    service: "db".into(),
                    project: "fixture".into(),
                    directory: "fixture-queue".into(),
                    ports: serde_json::json!({"5432/tcp": [{"HostPort":"5432"}]}),
                }],
            },
        );
        assert_eq!(ui.get_docker_rows().row_count(), 1);
        assert!(ui.get_docker_compose());
        assert!(!ui.get_database_open() && !ui.get_git_open());
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-docker-preview.png",
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_docker_logs_open(true);
        ui.set_docker_logs(slint::ModelRc::new(slint::VecModel::from(vec![
            LogcatRow {
                text: "2026-10-09 Database ready".into(),
                level: "".into(),
            },
        ])));
        let _ = ui.window().take_snapshot().unwrap();
        let width = ui.get_workspace_visible_width();
        ui.set_side_panel_width(500.0);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_workspace_visible_width() < width);
        ui.set_docker_open(false);
        ui.on_detect_lint(|| {});
        ui.set_lint_open(true);
        ui.set_lint_tools(ModelRc::new(VecModel::from(vec!["ESLint".into()])));
        lint_ui::display(
            &ui,
            ascurra_harness::lint::Report {
                status: "ESLint · 1 error · 1 advertencia".into(),
                problems: vec![
                    ascurra_harness::lint::Problem {
                        path: "src/main.ts".into(),
                        line: 8,
                        column: 5,
                        error: true,
                        code: "no-undef".into(),
                        message: "La variable no está definida.".into(),
                    },
                    ascurra_harness::lint::Problem {
                        path: "src/helpers.ts".into(),
                        line: 3,
                        column: 1,
                        error: false,
                        code: "no-unused-vars".into(),
                        message: "Variable sin usar.".into(),
                    },
                ],
            },
        );
        assert_eq!(ui.get_lint_rows().row_count(), 2);
        ui.set_lint_severity(1);
        let _ = ui.window().take_snapshot().unwrap();
        ui.invoke_filter_lint();
        assert_eq!(ui.get_lint_rows().row_count(), 1);
        ui.set_lint_severity(0);
        ui.set_lint_filter("HELPERS".into());
        let _ = ui.window().take_snapshot().unwrap();
        ui.invoke_filter_lint();
        assert_eq!(ui.get_lint_rows().row_count(), 1);
        ui.set_lint_filter("".into());
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-lint-preview.png",
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        assert!(!ui.get_docker_open() && !ui.get_database_open());
        let root = temp
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let text = (1..=150)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(temp.path().join("diagnostic-fixture.txt"), &text).unwrap();
        ui.set_git_project(root.clone().into());
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        ui.set_file_tabs(ModelRc::new(VecModel::from(vec![FileTabRow {
            project: root.into(),
            path: "diagnostic-fixture.txt".into(),
            title: "diagnostic-fixture.txt".into(),
            text: text.into(),
            status: "Solo lectura · texto UTF-8".into(),
        }])));
        ui.invoke_open_problem_file("diagnostic-fixture.txt".into(), 120);
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        assert!(ui.get_file_preview_open());
        assert_eq!(ui.get_file_target_line(), 120);
        ui.set_file_preview_open(false);
        ui.set_lint_open(false);
        // Render a terminal screen without starting any user shell in the UI test.
        ui.on_refresh_terminal(|| {});
        ui.set_terminal_open(true);
        ui.set_terminal_running(true);
        ui.set_terminal_rows(ModelRc::new(VecModel::from(vec![TerminalRow {
            runs: ModelRc::new(VecModel::from(vec![TerminalRun {
                text: "C:\\proyecto> cargo test".into(),
                column: 0,
                columns: 24,
                fg: slint::Color::from_rgb_u8(221, 227, 239),
                bg: slint::Color::from_rgb_u8(16, 24, 34),
                bold: false,
            }])),
        }])));
        for _ in 0..3 {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::testing_backend::mock_elapsed_time(100);
        }
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            "target/ui-terminal-preview.png",
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        assert!(!ui.get_lint_open());
        ui.set_terminal_open(false);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(250);
        let _ = ui.window().take_snapshot().unwrap();
        let workspace_before_usage = ui.get_workspace_visible_width();
        ui.on_refresh_usage(|| {});
        ui.on_show_usage(|| {});
        ui.invoke_open_usage();
        let report = ascurra_harness::usage_metrics::Report {
            groups: vec![
                ascurra_harness::usage_metrics::Group {
                    label: "write_file".into(),
                    calls: 3,
                    estimated_tokens: 12400,
                    argument_bytes: 49000,
                    reply_bytes: 600,
                    execution_ms: 35,
                    ..Default::default()
                },
                ascurra_harness::usage_metrics::Group {
                    label: "inspect_result".into(),
                    calls: 12,
                    estimated_tokens: 4600,
                    argument_bytes: 2400,
                    reply_bytes: 16000,
                    execution_ms: 64,
                    ..Default::default()
                },
            ],
            models: vec![ascurra_harness::usage_metrics::ModelUsage {
                label: "Codex · modelo de prueba".into(),
                requests: 4,
                usage: ascurra_harness::domain::TokenUsage {
                    input: Some(40000),
                    output: Some(9000),
                    cache_read: Some(30000),
                    reasoning: Some(1200),
                    ..Default::default()
                },
                ..Default::default()
            }],
            files: 4,
            ..Default::default()
        };
        usage_ui::display(&ui, &report);
        let snapshot = ui.window().take_snapshot().unwrap();
        assert!(ui.get_usage_open() && ui.get_usage_tab_open());
        assert_eq!(ui.get_usage_rows().row_count(), 2);
        assert_eq!(ui.get_usage_rows().row_data(0).unwrap().ratio, 1.0);
        assert!((ui.get_workspace_visible_width() - workspace_before_usage).abs() < 2.0);
        assert!(ui.get_usage_totals().contains("49000"));
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-usage-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_git_open(true);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(
            ui.get_usage_open(),
            "Los paneles laterales no cierran la pestaña Uso"
        );
        ui.set_git_open(false);
        ui.set_usage_view(2);
        let _ = ui.window().take_snapshot().unwrap();
        usage_ui::display(&ui, &report);
        assert_eq!(
            ui.get_usage_rows().row_data(0).unwrap().metric,
            "49000 tokens"
        );
        let chat = state.borrow().selected_chat.unwrap();
        ui.invoke_select_row(chat.to_string().into());
        assert!(!ui.get_usage_open() && ui.get_usage_tab_open());
        ui.invoke_open_usage();
        assert!(ui.get_usage_open() && ui.get_usage_tab_open());
        ui.set_usage_tab_open(false);
        ui.set_usage_open(false);
        ui.on_load_http(|| {});
        let workspace_before_http = ui.get_workspace_visible_width();
        ui.invoke_open_http();
        i_slint_backend_testing::testing_backend::mock_elapsed_time(220);
        let _ = ui.window().take_snapshot().unwrap();
        assert!(ui.get_http_open());
        assert_eq!(ui.get_active_panel_index(), 8);
        assert!(ui.get_workspace_visible_width() < workspace_before_http - 200.0);
        ui.invoke_select_row(chat.to_string().into());
        assert!(
            ui.get_http_open(),
            "Seleccionar un chat conserva el panel HTTP"
        );
        ui.set_http_open(false);
        i_slint_backend_testing::testing_backend::mock_elapsed_time(220);
        ui.set_approval_description("Directorio: proyecto\nPrograma: python\nTimeout: 30 s\nArgumentos: 2\n\nArgumento 1 (2 bytes):\n-c\n\nArgumento 2:\n# Actualizar documentación\nsave('README.md', 'Índice del proyecto')\nsave('docs/metrics.md', 'Métricas y almacenamiento')\nprint('texto \\n literal')".into());
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-approval-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_approval_description("".into());
    }
}
