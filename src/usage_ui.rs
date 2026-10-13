use crate::{AppWindow, UsageRow};
use ascurra_harness::{
    domain::AppState,
    runtime::Supervisor,
    storage::Store,
    usage_metrics::{self, Report},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

fn number(n: Option<u64>) -> String {
    n.map(|n| n.to_string())
        .unwrap_or_else(|| "sin dato".into())
}
fn partial_number(n: Option<u64>, partial: bool) -> String {
    format!(
        "{}{}",
        if partial && n.is_some() { "≥ " } else { "" },
        number(n)
    )
}
pub fn display(ui: &AppWindow, report: &Report) {
    let view = ui.get_usage_view();
    let groups = if view == 3 {
        &report.processes
    } else {
        &report.groups
    };
    let maximum = if view == 2 {
        report
            .models
            .iter()
            .map(|m| {
                m.usage
                    .input
                    .unwrap_or(0)
                    .saturating_add(m.usage.output.unwrap_or(0))
            })
            .max()
    } else if view == 1 {
        report.calls.iter().map(|c| c.estimated_tokens).max()
    } else {
        groups.iter().map(|g| g.estimated_tokens).max()
    }
    .unwrap_or(0)
    .max(1) as f32;
    let rows = if view == 2 {
        report
            .models
            .iter()
            .map(|m| {
                let total = m
                    .usage
                    .input
                    .unwrap_or(0)
                    .saturating_add(m.usage.output.unwrap_or(0));
                UsageRow {
                    title: m.label.clone().into(),
                    metric: if m.usage.input.is_none() && m.usage.output.is_none() {
                        "Sin datos de tokens".into()
                    } else {
                        format!("{}{} tokens", if m.incomplete { "≥ " } else { "" }, total).into()
                    },
                    detail: format!(
                        "Entrada {} · salida {}\nCaché leída {} · razonamiento {}",
                        partial_number(m.usage.input, m.incomplete),
                        partial_number(m.usage.output, m.incomplete),
                        partial_number(m.usage.cache_read, m.cache_incomplete),
                        partial_number(m.usage.reasoning, m.reasoning_incomplete)
                    )
                    .into(),
                    hint: format!(
                        "{} registros del proveedor{}",
                        m.requests,
                        if m.incomplete {
                            " · medición parcial"
                        } else {
                            ""
                        }
                    )
                    .into(),
                    ratio: total as f32 / maximum,
                }
            })
            .collect::<Vec<_>>()
    } else if view == 1 {
        report
            .calls
            .iter()
            .take(500)
            .map(|c| UsageRow {
                title: format!(
                    "{}{}",
                    c.tool,
                    if c.target.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", c.target)
                    }
                )
                .into(),
                metric: format!("≈ {} tokens", c.estimated_tokens).into(),
                detail: format!(
                    "Argumentos {} B · respuesta {} B · {} ms{}",
                    number(c.argument_bytes),
                    c.reply_bytes,
                    c.execution_ms,
                    match (c.offset, c.limit) {
                        (Some(offset), Some(limit)) =>
                            format!("\nOffset {offset}, limite {limit}, stream {}", c.stream),
                        _ => String::new(),
                    }
                )
                .into(),
                hint: format!(
                    "{} · llamada {} · {}{}",
                    c.chat_title,
                    c.index,
                    c.run.chars().take(8).collect::<String>(),
                    if c.legacy {
                        " · histórico parcial"
                    } else {
                        ""
                    }
                )
                .into(),
                ratio: c.estimated_tokens as f32 / maximum,
            })
            .collect()
    } else {
        groups
            .iter()
            .map(|g| UsageRow {
                title: g.label.clone().into(),
                metric: format!("≈ {} tokens", g.estimated_tokens).into(),
                detail: format!(
                    "{} llamadas · {} ms\nArgumentos {} · respuestas {} B",
                    g.calls,
                    g.execution_ms,
                    if g.legacy_calls == g.calls {
                        "sin datos".into()
                    } else {
                        format!(
                            "{}{} B",
                            if g.legacy_calls > 0 { "≥ " } else { "" },
                            g.argument_bytes
                        )
                    },
                    g.reply_bytes
                )
                .into(),
                hint: if g.legacy_calls > 0 {
                    format!(
                        "{} llamadas históricas sin tamaño de argumentos",
                        g.legacy_calls
                    )
                } else {
                    "Argumentos + respuestas, una exposición".into()
                }
                .into(),
                ratio: g.estimated_tokens as f32 / maximum,
            })
            .collect()
    };
    ui.set_usage_rows(ModelRc::new(VecModel::from(rows)));
    let total = report.models.iter().fold(0u64, |sum, m| {
        sum.saturating_add(m.usage.input.unwrap_or(0))
            .saturating_add(m.usage.output.unwrap_or(0))
    });
    let incomplete = report.models.iter().any(|m| m.incomplete);
    let count = report
        .groups
        .iter()
        .fold(0u64, |sum, g| sum.saturating_add(g.calls));
    ui.set_usage_totals(
        if report.models.is_empty()
            || report
                .models
                .iter()
                .all(|m| m.usage.input.is_none() && m.usage.output.is_none())
        {
            format!("Tokens reportados: sin datos · {count} llamadas registradas").into()
        } else {
            format!(
                "{}{} tokens reportados · {} llamadas registradas",
                if incomplete { "≥ " } else { "" },
                total,
                count
            )
            .into()
        },
    );
    ui.set_usage_status(
        format!(
            "{} logs{}{}{} \u{b7} {} relecturas exactas \u{b7} {} inspecciones \u{b7} {} B en respuestas de lectura{}",
            report.files,
            if report.partial {
                " · lectura parcial por límites o ejecución en curso"
            } else {
                ""
            },
            if report.invalid_lines > 0 {
                format!(" · {} líneas inválidas", report.invalid_lines)
            } else {
                String::new()
            },
            if report.missing_chats > 0 {
                format!("; {} chats sin logs", report.missing_chats)
            } else {
                String::new()
            },
            report.repeated_reads,
            report.inspected_calls,
            report.read_reply_bytes,
            if report.truncation_reasons.is_empty() { String::new() } else { format!("; limites: {}", report.truncation_reasons.join(", ")) }
        )
        .into(),
    );
}
pub fn configure(
    ui: &AppWindow,
    state: Rc<RefCell<AppState>>,
    store: Rc<Store>,
    supervisor: Arc<Supervisor>,
) {
    let cached = Arc::new(std::sync::Mutex::new(Report::default()));
    let weak = ui.as_weak();
    let report = cached.clone();
    ui.on_show_usage(move || {
        if let Some(ui) = weak.upgrade() {
            display(&ui, &report.lock().unwrap());
        }
    });
    let generation = Arc::new(AtomicU64::new(0));
    let scan_gate = Arc::new(std::sync::Mutex::new(()));
    let weak = ui.as_weak();
    let data = store.root().to_path_buf();
    let report = cached.clone();
    ui.on_refresh_usage(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_usage_open() {
            return;
        }
        let project = ui.get_git_project().to_string();
        let scope = ui.get_usage_scope();
        let selected = ui.get_usage_selected_chat().to_string();
        let state = state.borrow();
        let Some(project_state) = state.projects.iter().find(|p| p.path == project) else {
            generation.fetch_add(1, Ordering::Relaxed);
            ui.set_usage_busy(false);
            *report.lock().unwrap() = Report::default();
            display(&ui, &report.lock().unwrap());
            ui.set_usage_status("Abrí un proyecto para consultar su uso.".into());
            return;
        };
        let chats = project_state
            .chats
            .iter()
            .filter(|c| scope == 0 || Some(c.id) == selected.parse().ok())
            .map(|c| (c.id, c.title.clone()))
            .collect::<Vec<_>>();
        let samples = project_state
            .chats
            .iter()
            .filter(|c| scope == 0 || Some(c.id) == selected.parse().ok())
            .flat_map(|c| c.usage.iter().cloned().map(move |u| (c.id, u)))
            .collect::<Vec<_>>();
        drop(state);
        *report.lock().unwrap() = Report::default();
        display(&ui, &report.lock().unwrap());
        let version = generation.fetch_add(1, Ordering::Relaxed) + 1;
        let current = generation.clone();
        let data = data.clone();
        let handle = ui.as_weak();
        let report = report.clone();
        ui.set_usage_busy(true);
        ui.set_usage_status("Leyendo métricas locales…".into());
        let scan_gate = scan_gate.clone();
        let worker_generation = current.clone();
        supervisor.executor.spawn(async move {
            let export_project = project.clone();
            let result = tokio::task::spawn_blocking(move || {
                // Serialize disk work. Superseded queued refreshes never scan or export.
                let _guard = scan_gate.lock().unwrap();
                if worker_generation.load(Ordering::Relaxed) != version {
                    return None;
                }
                let result = usage_metrics::scan(&data, &chats, &samples);
                if worker_generation.load(Ordering::Relaxed) != version {
                    return None;
                }
                let saved = usage_metrics::save(
                    &data,
                    &export_project,
                    if scope == 0 { "project" } else { "chat" },
                    &result,
                )
                .is_ok();
                Some((result, saved))
            })
            .await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if current.load(Ordering::Relaxed) != version
                    || ui.get_git_project().as_str() != project
                    || ui.get_usage_scope() != scope
                    || (scope == 1 && ui.get_usage_selected_chat().as_str() != selected)
                {
                    return;
                }
                ui.set_usage_busy(false);
                if let Ok(Some((result, saved))) = result {
                    *report.lock().unwrap() = result;
                    display(&ui, &report.lock().unwrap());
                    if !saved {
                        ui.set_usage_status(
                            format!("{} · no se pudo guardar el informe", ui.get_usage_status())
                                .into(),
                        );
                    }
                } else {
                    ui.set_usage_status("No se pudieron leer las métricas.".into());
                }
            });
        });
    });
    let weak = ui.as_weak();
    let data = store.root().to_path_buf();
    ui.on_open_usage_report(move || {
        if let Some(ui) = weak.upgrade() {
            let path = usage_metrics::report_folder(&data, ui.get_git_project().as_str());
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
            }
            #[cfg(not(windows))]
            {
                let _ = path;
            }
        }
    });
}
