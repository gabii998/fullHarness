use crate::{
    domain::{
        AppState, Chat, Id, Message, Provider, RunState, Task, TaskStatus, TokenUsage, UsageRecord,
    },
    settings::AgentSettings,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRun {
    pub chat: Id,
    pub state: RunState,
    pub usage: Vec<UsageRecord>,
    pub usage_incomplete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskQueue {
    pub pending: Vec<Id>,
    pub active_task: Option<Id>,
    pub active_chat: Option<Id>,
    pub running: bool,
    pub provider: Provider,
    pub config: AgentSettings,
    pub message: String,
}
pub struct QueuedJob {
    pub chat: Chat,
    pub project: String,
    pub provider: Provider,
    pub config: AgentSettings,
}
fn active(state: RunState) -> bool {
    matches!(
        state,
        RunState::Running | RunState::WaitingForPermission | RunState::WaitingForInput
    )
}

impl Task {
    pub fn token_usage(&self) -> TokenUsage {
        let reports = self
            .runs
            .iter()
            .flat_map(|run| &run.usage)
            .collect::<Vec<_>>();
        let sum = |field: fn(&TokenUsage) -> Option<u64>| -> Option<u64> {
            if reports.is_empty() || self.runs.iter().any(|run| run.usage.is_empty()) {
                return None;
            }
            reports.iter().try_fold(0u64, |total, record| {
                total.checked_add(field(&record.usage)?)
            })
        };
        TokenUsage {
            input: sum(|u| u.input),
            output: sum(|u| u.output),
            cache_read: sum(|u| u.cache_read),
            cache_write: sum(|u| u.cache_write),
            reasoning: sum(|u| u.reasoning),
        }
    }
}
impl AppState {
    pub fn begin_task_queue(
        &mut self,
        project_id: Id,
        provider: Provider,
        config: AgentSettings,
    ) -> Result<(), String> {
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .ok_or("El proyecto no existe.")?;
        if project
            .queue
            .as_ref()
            .is_some_and(|q| q.running || q.active_chat.is_some())
            || project.chats.iter().any(|chat| active(chat.state))
        {
            return Err("Ya hay una ejecución en este proyecto. Esperá a que termine.".into());
        }
        if !config.enabled {
            return Err("Configurá y habilitá el agente antes de ejecutar la cola.".into());
        }
        let pending = project
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Todo)
            .map(|task| task.id)
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Err("No hay tareas por hacer.".into());
        }
        let mut config = config;
        config.models.clear();
        project.queue = Some(TaskQueue {
            pending,
            active_task: None,
            active_chat: None,
            running: true,
            provider,
            config,
            message: "Cola en ejecución.".into(),
        });
        Ok(())
    }
    pub fn prepare_queued_job(&mut self, project_id: Id) -> Result<Option<QueuedJob>, String> {
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .ok_or("El proyecto no existe.")?;
        let Some(queue) = &mut project.queue else {
            return Ok(None);
        };
        if !queue.running || queue.active_chat.is_some() {
            return Ok(None);
        }
        if project.chats.iter().any(|chat| active(chat.state)) {
            queue.running = false;
            queue.message = "Cola pausada: hay otro chat en ejecución.".into();
            return Err(queue.message.clone());
        }
        // Tasks moved out of Todo after the queue was captured are skipped.
        queue.pending.retain(|id| {
            project
                .tasks
                .iter()
                .any(|t| t.id == *id && t.status == TaskStatus::Todo)
        });
        let next = if queue.pending.is_empty() {
            None
        } else {
            Some(0)
        };
        let Some(next) = next else {
            queue.pending.clear();
            queue.running = false;
            queue.message = "Cola completada.".into();
            return Ok(None);
        };
        let project_context = project
            .profile
            .context(std::path::Path::new(&project.path), queue.provider)?;
        let task_id = queue.pending.remove(next);
        let task = project
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .unwrap();
        let title = task.title.clone();
        let prompt = format!(
            "Ejecutá la siguiente tarea de programación en este repositorio.\n\nTítulo: {}\n\nObjetivo:\n{}\n\nCriterio de aceptación:\n{}\n\nSeveridad: {:?}\n\nAl terminar, describí los cambios y cómo verificaste el criterio de aceptación. Si no podés completarla, indicá qué quedó pendiente.",
            task.title, task.objective, task.acceptance, task.severity
        );
        let provider = queue.provider;
        let config = queue.config.clone();
        let path = project.path.clone();
        let selected = self.selected_chat;
        let overview = self.selected_project;
        let id = self
            .add_chat(project_id)
            .ok_or("No se pudo crear el chat.")?;
        self.selected_chat = selected;
        self.selected_project = overview;
        let chat = self.chat_mut(id).unwrap();
        chat.title = title;
        chat.provider = provider;
        chat.model = config.model.clone();
        chat.reasoning_effort = Some(config.reasoning_effort.clone());
        chat.messages.push(Message {
            role: "Vos".into(),
            text: prompt,
        });
        chat.state = RunState::Running;
        chat.last_status = "Ejecutando tarea de la cola…".into();
        let job = QueuedJob {
            chat: {
                let mut execution = chat.clone();
                execution.project_context = project_context;
                execution
            },
            project: path,
            provider,
            config,
        };
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .unwrap();
        let task = project.tasks.iter_mut().find(|t| t.id == task_id).unwrap();
        task.status = TaskStatus::InProgress;
        task.runs.push(TaskRun {
            chat: id,
            state: RunState::Running,
            usage: Vec::new(),
            usage_incomplete: false,
        });
        let queue = project.queue.as_mut().unwrap();
        queue.active_chat = Some(id);
        queue.active_task = Some(task_id);
        Ok(Some(job))
    }
    pub fn pause_task_queue(&mut self, project_id: Id) {
        if let Some(queue) = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .and_then(|p| p.queue.as_mut())
        {
            queue.running = false;
            queue.message = "Cola pausada. La tarea actual puede terminar.".into();
        }
    }
    pub fn sync_task_run(&mut self, chat_id: Id) {
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|p| p.chats.iter().any(|c| c.id == chat_id))
        else {
            return;
        };
        let chat = project.chats.iter().find(|c| c.id == chat_id).unwrap();
        for run in project
            .tasks
            .iter_mut()
            .flat_map(|task| &mut task.runs)
            .filter(|run| run.chat == chat_id)
        {
            run.state = chat.state;
            run.usage = chat.usage.clone();
            run.usage_incomplete = chat.usage_incomplete;
        }
    }
    pub fn finish_queued_chat(&mut self, chat_id: Id) -> Option<Id> {
        self.sync_task_run(chat_id);
        let project = self.projects.iter_mut().find(|p| {
            p.queue
                .as_ref()
                .is_some_and(|q| q.active_chat == Some(chat_id))
        })?;
        let final_state = project.chats.iter().find(|c| c.id == chat_id)?.state;
        if active(final_state) {
            return None;
        }
        let queue = project.queue.as_mut().unwrap();
        let task_id = queue.active_task.take()?;
        queue.active_chat = None;
        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
            if final_state == RunState::Succeeded {
                task.status = TaskStatus::Done;
            } else {
                task.status = TaskStatus::Todo;
                queue.running = false;
                queue.pending.insert(0, task_id);
                queue.message =
                    "Cola pausada: la tarea no terminó correctamente. Revisá su chat.".into();
            }
        }
        Some(project.id)
    }
    pub fn reconcile_task_queues(&mut self) {
        let chats = self
            .projects
            .iter()
            .flat_map(|p| &p.chats)
            .map(|c| c.id)
            .collect::<Vec<_>>();
        for chat in chats {
            self.sync_task_run(chat);
        }
        for run in self
            .projects
            .iter_mut()
            .flat_map(|p| &mut p.tasks)
            .flat_map(|t| &mut t.runs)
        {
            if run.state == RunState::Interrupted {
                run.usage_incomplete = true;
            }
        }
        for project in &mut self.projects {
            if let Some(queue) = &mut project.queue {
                if queue.running || queue.active_chat.is_some() {
                    queue.running = false;
                    if let Some(id) = queue.active_task.take() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == id) {
                            task.status = TaskStatus::Todo;
                        }
                        if !queue.pending.contains(&id) {
                            queue.pending.insert(0, id);
                        }
                    }
                    queue.active_chat = None;
                    queue.message = "Cola pausada después de reiniciar. Revisá la tarea interrumpida antes de continuar.".into();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (AppState, Id, Vec<Id>) {
        let mut state = AppState::default();
        let project = state.add_project("Fixture".into(), "fixture".into());
        state.select_project(project);
        let ids = (0..3)
            .map(|i| {
                state
                    .save_task(
                        project,
                        None,
                        format!("Task {i}"),
                        "Goal".into(),
                        "Acceptance".into(),
                        crate::domain::TaskSeverity::High,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        (state, project, ids)
    }
    fn config() -> AgentSettings {
        AgentSettings {
            enabled: true,
            model: "fixture".into(),
            ..Default::default()
        }
    }
    fn usage(request: &str, input: u64, output: Option<u64>) -> UsageRecord {
        UsageRecord {
            request_id: request.into(),
            model: "fixture".into(),
            provider: Provider::Codex,
            usage: TokenUsage {
                input: Some(input),
                output,
                cache_read: Some(40),
                ..Default::default()
            },
            context_input: Some(input),
            context_window: None,
        }
    }
    #[test]
    fn queue_is_sequential_preserves_view_and_tracks_exact_deduplicated_tokens() {
        let (mut state, project, ids) = fixture();
        state.projects[0].profile.description = "Queue project configuration".into();
        state
            .begin_task_queue(project, Provider::Codex, config())
            .unwrap();
        let first = state.prepare_queued_job(project).unwrap().unwrap();
        assert!(
            first
                .chat
                .project_context
                .contains("Queue project configuration")
        );
        assert!(
            state
                .chat(first.chat.id)
                .unwrap()
                .project_context
                .is_empty()
        );
        assert_eq!(state.selected_project, Some(project));
        assert!(state.selected_chat.is_none());
        assert!(first.chat.messages[0].text.contains("Acceptance"));
        assert!(state.prepare_queued_job(project).unwrap().is_none());
        assert_eq!(state.projects[0].tasks[0].status, TaskStatus::InProgress);
        state
            .chat_mut(first.chat.id)
            .unwrap()
            .record_usage(usage("request", 100, Some(10)));
        state.sync_task_run(first.chat.id);
        state
            .chat_mut(first.chat.id)
            .unwrap()
            .record_usage(usage("request", 120, Some(20)));
        state.chat_mut(first.chat.id).unwrap().state = RunState::Succeeded;
        assert_eq!(state.finish_queued_chat(first.chat.id), Some(project));
        let task = &state.projects[0].tasks[0];
        assert_eq!(task.id, ids[0]);
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.runs[0].usage.len(), 1);
        assert_eq!(task.token_usage().input, Some(120));
        assert_eq!(task.token_usage().output, Some(20));
        assert_eq!(task.token_usage().cache_read, Some(40));
        let second = state.prepare_queued_job(project).unwrap().unwrap();
        assert_ne!(first.chat.id, second.chat.id);
        state
            .chat_mut(second.chat.id)
            .unwrap()
            .record_usage(usage("second", 50, None));
        state.chat_mut(second.chat.id).unwrap().state = RunState::Succeeded;
        state.finish_queued_chat(second.chat.id);
        assert_eq!(state.projects[0].tasks[1].token_usage().input, Some(50));
        assert_eq!(state.projects[0].tasks[1].token_usage().output, None);
        let third = state.prepare_queued_job(project).unwrap().unwrap();
        state.chat_mut(third.chat.id).unwrap().state = RunState::Succeeded;
        state.finish_queued_chat(third.chat.id);
        assert!(state.prepare_queued_job(project).unwrap().is_none());
        assert!(!state.projects[0].queue.as_ref().unwrap().running);
        let saved: AppState =
            serde_json::from_str(&serde_json::to_string(&state.metadata_snapshot()).unwrap())
                .unwrap();
        assert_eq!(saved.projects[0].tasks[0].token_usage().input, Some(120));
    }
    #[test]
    fn failure_pause_and_restart_never_advance_or_repeat_without_explicit_start() {
        let (mut state, project, _) = fixture();
        state
            .begin_task_queue(project, Provider::Codex, config())
            .unwrap();
        let first = state.prepare_queued_job(project).unwrap().unwrap();
        state.chat_mut(first.chat.id).unwrap().state = RunState::WaitingForPermission;
        assert!(state.finish_queued_chat(first.chat.id).is_none());
        assert!(state.prepare_queued_job(project).unwrap().is_none());
        state
            .chat_mut(first.chat.id)
            .unwrap()
            .record_usage(usage("first", 10, Some(2)));
        state.chat_mut(first.chat.id).unwrap().state = RunState::Failed;
        state.finish_queued_chat(first.chat.id);
        assert_eq!(state.projects[0].tasks[0].status, TaskStatus::Todo);
        assert!(!state.projects[0].queue.as_ref().unwrap().running);
        assert!(state.prepare_queued_job(project).unwrap().is_none());
        state
            .begin_task_queue(project, Provider::Codex, config())
            .unwrap();
        let retry = state.prepare_queued_job(project).unwrap().unwrap();
        assert_ne!(first.chat.id, retry.chat.id);
        state
            .chat_mut(retry.chat.id)
            .unwrap()
            .record_usage(usage("retry", 20, Some(4)));
        state.pause_task_queue(project);
        state.chat_mut(retry.chat.id).unwrap().state = RunState::Succeeded;
        state.finish_queued_chat(retry.chat.id);
        assert_eq!(state.projects[0].tasks[0].runs.len(), 2);
        assert_eq!(state.projects[0].tasks[0].token_usage().input, Some(30));
        assert_eq!(state.projects[0].tasks[0].token_usage().output, Some(6));
        assert!(state.prepare_queued_job(project).unwrap().is_none());
        state
            .begin_task_queue(project, Provider::Codex, config())
            .unwrap();
        let interrupted = state.prepare_queued_job(project).unwrap().unwrap();
        state.reconcile_after_restart();
        assert_eq!(
            state.chat(interrupted.chat.id).unwrap().state,
            RunState::Interrupted
        );
        assert_eq!(
            state.projects[0].tasks[1].runs[0].state,
            RunState::Interrupted
        );
        assert_eq!(state.projects[0].tasks[1].status, TaskStatus::Todo);
        assert!(state.projects[0].tasks[1].runs[0].usage_incomplete);
        assert!(state.prepare_queued_job(project).unwrap().is_none());
    }
    #[test]
    fn active_repository_blocks_a_queue_and_active_tasks_cannot_be_edited() {
        let (mut state, project, ids) = fixture();
        let existing = state.projects[0].chats[0].id;
        state.chat_mut(existing).unwrap().state = RunState::Running;
        assert!(
            state
                .begin_task_queue(project, Provider::Codex, config())
                .is_err()
        );
        state.chat_mut(existing).unwrap().state = RunState::Idle;
        state
            .begin_task_queue(project, Provider::Codex, config())
            .unwrap();
        state.prepare_queued_job(project).unwrap().unwrap();
        assert!(
            state
                .save_task(
                    project,
                    Some(ids[0]),
                    "Edited".into(),
                    "Goal".into(),
                    "Accept".into(),
                    crate::domain::TaskSeverity::Low
                )
                .is_err()
        );
    }
}
