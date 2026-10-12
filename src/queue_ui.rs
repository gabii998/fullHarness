use crate::{AppWindow, available_providers, refresh};
use ascurra_harness::{
    domain::{AppState, Provider, RunState},
    runtime::{Event, Supervisor},
    storage::Store,
};
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn configure(
    ui: &AppWindow,
    state: Rc<RefCell<AppState>>,
    store: Rc<Store>,
    supervisor: Arc<Supervisor>,
) {
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_run_task_queue(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        let Some(project) = state.selected_project else {
            return;
        };
        let Some(provider) = available_providers(&ui, &state)
            .get(ui.get_queue_provider_index() as usize)
            .copied()
        else {
            ui.set_task_error("Configurá un agente antes de ejecutar la cola.".into());
            return;
        };
        let config = state.settings.agent(provider).clone();
        if let Err(error) = state.begin_task_queue(project, provider, config) {
            ui.set_task_error(error.into());
            return;
        }
        if let Err(error) = disk.save(&state) {
            state.pause_task_queue(project);
            ui.set_task_error(format!("No se pudo guardar: {error}").into());
            refresh(&ui, &state);
            return;
        }
        ui.set_task_error("".into());
        refresh(&ui, &state);
        drop(state);
        ui.invoke_advance_task_queue(project.to_string().into());
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_pause_task_queue(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        let Some(project) = state.selected_project else {
            return;
        };
        state.pause_task_queue(project);
        if let Err(error) = disk.save(&state) {
            ui.set_task_error(format!("No se pudo guardar: {error}").into());
        }
        refresh(&ui, &state);
    });
    let weak = ui.as_weak();
    ui.on_advance_task_queue(move |project| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(project) = project.parse::<u64>() else {
            return;
        };
        let mut state = state.borrow_mut();
        let provider = state
            .projects
            .iter()
            .find(|p| p.id == project)
            .and_then(|p| p.queue.as_ref())
            .filter(|q| q.running && q.active_chat.is_none())
            .map(|q| q.provider);
        let Some(provider) = provider else {
            return;
        };
        if !available_providers(&ui, &state).contains(&provider) {
            state.pause_task_queue(project);
            if let Some(queue) = state
                .projects
                .iter_mut()
                .find(|p| p.id == project)
                .and_then(|p| p.queue.as_mut())
            {
                queue.message = "Cola pausada: verificá la configuración del agente.".into();
            }
            let _ = store.save(&state);
            refresh(&ui, &state);
            return;
        }
        let job = match state.prepare_queued_job(project) {
            Ok(Some(job)) => job,
            Ok(None) => {
                if let Err(error) = store.save(&state) {
                    ui.set_task_error(format!("No se pudo guardar: {error}").into());
                }
                refresh(&ui, &state);
                return;
            }
            Err(error) => {
                state.pause_task_queue(project);
                let _ = store.save(&state);
                ui.set_task_error(error.into());
                refresh(&ui, &state);
                return;
            }
        };
        let id = job.chat.id;
        let start = store
            .save(&state)
            .map_err(|e| format!("No se inició la tarea: no se pudo guardar ({e})."))
            .and_then(|_| {
                let handle = ui.as_weak();
                let emit = Arc::new(move |event: Event| {
                    if let Ok(event) = serde_json::to_string(&event) {
                        let _ = handle
                            .upgrade_in_event_loop(move |ui| ui.invoke_agent_event(event.into()));
                    }
                });
                if job.provider == Provider::Claude {
                    supervisor.start_claude(
                        job.chat,
                        job.project,
                        store.root().into(),
                        job.config,
                        emit,
                    )
                } else {
                    supervisor.start(
                        job.chat,
                        job.project,
                        store.root().into(),
                        job.config.model,
                        job.config.reasoning_effort,
                        job.config.concurrency as usize,
                        emit,
                    )
                }
            });
        if let Err(error) = start {
            if let Some(chat) = state.chat_mut(id) {
                chat.state = RunState::Failed;
                chat.last_status = error.clone();
            }
            state.finish_queued_chat(id);
            let _ = store.save(&state);
            ui.set_task_error(error.into());
        }
        refresh(&ui, &state);
    });
}
