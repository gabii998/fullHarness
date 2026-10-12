use crate::{AppWindow, TaskRow, refresh};
use ascurra_harness::{
    domain::{AppState, TaskSeverity, TaskStatus},
    storage::Store,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

pub fn task_tokens(task: &ascurra_harness::domain::Task) -> String {
    let usage = task.token_usage();
    let number = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
    if task.runs.is_empty() {
        return "Tokens: sin ejecución".into();
    }
    if usage.input.is_none() && usage.output.is_none() {
        return if task.runs.iter().any(|run| !run.usage.is_empty()) {
            "Tokens: total desconocido (datos parciales)".into()
        } else {
            "Tokens: sin datos reportados".into()
        };
    }
    let mut text = format!(
        "Entrada {} · Salida {}",
        number(usage.input),
        number(usage.output)
    );
    if let Some(cache) = usage.cache_read {
        text.push_str(&format!(" · Caché {cache}"));
    }
    if let Some(cache) = usage.cache_write {
        text.push_str(&format!(" · Escritura caché {cache}"));
    }
    if task.runs.iter().any(|run| run.usage_incomplete) {
        text.push_str(" · Parcial");
    }
    text
}

pub fn display(ui: &AppWindow, state: &AppState) {
    ui.set_queue_configured(!crate::available_providers(ui, state).is_empty());
    let queue = state
        .selected_project
        .and_then(|id| state.projects.iter().find(|p| p.id == id))
        .and_then(|p| p.queue.as_ref());
    ui.set_queue_running(queue.is_some_and(|q| q.running));
    ui.set_queue_active(queue.is_some_and(|q| q.active_chat.is_some()));
    ui.set_queue_chat(
        queue
            .and_then(|q| q.active_chat)
            .map(|id| id.to_string())
            .unwrap_or_default()
            .into(),
    );
    ui.set_queue_message(
        queue
            .map(|q| {
                format!(
                    "{:?} · {} · {} · {} pendientes",
                    q.provider,
                    q.config.model,
                    q.message,
                    q.pending.len()
                )
            })
            .unwrap_or_default()
            .into(),
    );
    let tasks = state
        .selected_project
        .and_then(|id| state.projects.iter().find(|p| p.id == id))
        .map(|p| p.tasks.as_slice())
        .unwrap_or(&[]);
    for (status, set) in [
        (
            TaskStatus::Todo,
            AppWindow::set_tasks_todo as fn(&AppWindow, ModelRc<TaskRow>),
        ),
        (TaskStatus::InProgress, AppWindow::set_tasks_progress),
        (TaskStatus::Done, AppWindow::set_tasks_done),
    ] {
        set(
            ui,
            ModelRc::new(VecModel::from(
                tasks
                    .iter()
                    .filter(|task| task.status == status)
                    .map(|task| TaskRow {
                        id: task.id.to_string().into(),
                        tokens: task_tokens(task).into(),
                        chat: task
                            .runs
                            .last()
                            .map(|run| run.chat.to_string())
                            .unwrap_or_default()
                            .into(),
                        running: task.runs.last().is_some_and(|run| {
                            matches!(
                                run.state,
                                ascurra_harness::domain::RunState::Running
                                    | ascurra_harness::domain::RunState::WaitingForPermission
                                    | ascurra_harness::domain::RunState::WaitingForInput
                            )
                        }),
                        title: task.title.clone().into(),
                        objective: task.objective.clone().into(),
                        acceptance: task.acceptance.clone().into(),
                        severity: match task.severity {
                            TaskSeverity::Low => 0,
                            TaskSeverity::Medium => 1,
                            TaskSeverity::High => 2,
                            TaskSeverity::Critical => 3,
                        },
                        status: match task.status {
                            TaskStatus::Todo => 0,
                            TaskStatus::InProgress => 1,
                            TaskStatus::Done => 2,
                        },
                    })
                    .collect::<Vec<_>>(),
            )),
        );
    }
}

fn persist(ui: &AppWindow, state: &mut AppState, store: &Store) -> bool {
    let result = store.save(state);
    if result.is_ok() {
        store.release_inactive(state);
    }
    refresh(ui, state);
    match result {
        Ok(()) => true,
        Err(error) => {
            ui.set_task_error(format!("No se pudo guardar: {error}").into());
            false
        }
    }
}

pub fn configure(ui: &AppWindow, state: Rc<RefCell<AppState>>, store: Rc<Store>) {
    let weak = ui.as_weak();
    let data = state.clone();
    ui.on_edit_task(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let state = data.borrow();
        let Some(project) = state
            .selected_project
            .and_then(|id| state.projects.iter().find(|p| p.id == id))
        else {
            return;
        };
        let task = if id.is_empty() {
            None
        } else {
            let Some(task) = id
                .parse::<u64>()
                .ok()
                .and_then(|id| project.tasks.iter().find(|t| t.id == id))
            else {
                return;
            };
            Some(task)
        };
        ui.set_task_token_detail(
            task.map(|task| {
                let mut detail = task_tokens(task);
                for run in &task.runs {
                    let attempt = ascurra_harness::domain::Task {
                        id: task.id,
                        title: String::new(),
                        objective: String::new(),
                        acceptance: String::new(),
                        severity: task.severity,
                        status: task.status,
                        runs: vec![run.clone()],
                    };
                    detail.push_str(&format!(
                        "\nChat #{} · {:?} · {}",
                        run.chat,
                        run.state,
                        task_tokens(&attempt)
                    ));
                }
                detail
            })
            .unwrap_or_default()
            .into(),
        );
        ui.set_task_edit_id(id);
        ui.set_task_edit_project(project.id.to_string().into());
        ui.set_task_title(task.map(|t| t.title.as_str()).unwrap_or("").into());
        ui.set_task_objective(task.map(|t| t.objective.as_str()).unwrap_or("").into());
        ui.set_task_acceptance(task.map(|t| t.acceptance.as_str()).unwrap_or("").into());
        ui.set_task_severity(
            task.map(|t| match t.severity {
                TaskSeverity::Low => 0,
                TaskSeverity::Medium => 1,
                TaskSeverity::High => 2,
                TaskSeverity::Critical => 3,
            })
            .unwrap_or(1),
        );
        ui.set_task_status(
            task.map(|t| match t.status {
                TaskStatus::Todo => 0,
                TaskStatus::InProgress => 1,
                TaskStatus::Done => 2,
            })
            .unwrap_or(0),
        );
        ui.set_task_error("".into());
        ui.set_task_dialog_open(true);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_save_task(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        let Some(project) = state
            .selected_project
            .filter(|id| id.to_string() == ui.get_task_edit_project().as_str())
        else {
            return;
        };
        let status = match ui.get_task_status() {
            0 => TaskStatus::Todo,
            1 => TaskStatus::InProgress,
            2 => TaskStatus::Done,
            _ => return,
        };
        let severity = match ui.get_task_severity() {
            0 => TaskSeverity::Low,
            1 => TaskSeverity::Medium,
            2 => TaskSeverity::High,
            3 => TaskSeverity::Critical,
            _ => return,
        };
        let task_id = if ui.get_task_edit_id().is_empty() {
            None
        } else {
            let Ok(id) = ui.get_task_edit_id().parse::<u64>() else {
                return;
            };
            Some(id)
        };
        match state.save_task(
            project,
            task_id,
            ui.get_task_title().to_string(),
            ui.get_task_objective().to_string(),
            ui.get_task_acceptance().to_string(),
            severity,
        ) {
            Ok(id) => {
                if let Err(error) = state.move_task(project, id, status) {
                    ui.set_task_error(error.into());
                    return;
                }
                ui.set_task_edit_id(id.to_string().into());
                if persist(&ui, &mut state, &disk) {
                    ui.set_task_dialog_open(false);
                    ui.set_task_error("".into());
                }
            }
            Err(error) => ui.set_task_error(error.into()),
        }
    });
}
