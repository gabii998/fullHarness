use serde::{Deserialize, Serialize};

pub type Id = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    #[default]
    Idle,
    Running,
    WaitingForInput,
    WaitingForPermission,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}

/// Unknown is None, never zero. Reasoning/cache breakdowns are subsets when
/// the provider defines them that way; they must not be added to totals again.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    pub request_id: String,
    pub model: String,
    pub provider: Provider,
    pub usage: TokenUsage,
    /// Explicit measurement of one inference input, not a turn aggregate.
    #[serde(default)]
    pub context_input: Option<u64>,
    #[serde(default)]
    pub context_window: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    /// Per-execution project instructions, never duplicated in stored histories.
    #[serde(skip)]
    pub project_context: String,
    pub id: Id,
    pub title: String,
    pub provider: Provider,
    pub state: RunState,
    pub provider_session_id: Option<String>,
    pub messages: Vec<Message>,
    /// Relative immutable payload referenced by the lightweight state index.
    #[serde(default)]
    pub history_file: Option<String>,
    #[serde(skip)]
    pub history_loaded: bool,
    pub usage: Vec<UsageRecord>,
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub draft_images: Vec<crate::attachments::Attachment>,
    #[serde(default)]
    pub message_images: std::collections::BTreeMap<usize, Vec<crate::attachments::Attachment>>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Provider wire history includes function calls and encrypted reasoning.
    /// Tool artifacts themselves are never inserted here automatically.
    #[serde(default)]
    pub wire_history: Vec<serde_json::Value>,
    #[serde(default)]
    pub usage_incomplete: bool,
    #[serde(default)]
    pub last_status: String,
    #[serde(skip)]
    pub approval: String,
}

impl Chat {
    /// Replace an updated report for the same request instead of counting it twice.
    pub fn record_usage(&mut self, record: UsageRecord) {
        if let Some(existing) = self
            .usage
            .iter_mut()
            .find(|item| item.request_id == record.request_id && item.provider == record.provider)
        {
            *existing = record;
        } else {
            self.usage.push(record);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    #[default]
    Todo,
    InProgress,
    Done,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskSeverity {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub title: String,
    pub objective: String,
    pub acceptance: String,
    pub severity: TaskSeverity,
    pub status: TaskStatus,
    #[serde(default)]
    pub runs: Vec<crate::task_queue::TaskRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub profile: crate::project_profile::ProjectProfile,
    pub id: Id,
    pub name: String,
    pub path: String,
    pub chats: Vec<Chat>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub queue: Option<crate::task_queue::TaskQueue>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub settings: crate::settings::Settings,
    pub projects: Vec<Project>,
    pub selected_chat: Option<Id>,
    #[serde(default)]
    pub selected_project: Option<Id>,
    pub open_tabs: Vec<Id>,
    #[serde(default)]
    pub open_project_tabs: Vec<Id>,
    next_id: Id,
}

impl AppState {
    /// Clone metadata without cloning potentially large transcripts and wire JSON.
    pub fn metadata_snapshot(&self) -> Self {
        Self {
            settings: self.settings.clone(),
            selected_chat: self.selected_chat,
            selected_project: self.selected_project,
            open_tabs: self.open_tabs.clone(),
            open_project_tabs: self.open_project_tabs.clone(),
            next_id: self.next_id,
            projects: self
                .projects
                .iter()
                .map(|project| Project {
                    profile: project.profile.clone(),
                    id: project.id,
                    name: project.name.clone(),
                    path: project.path.clone(),
                    tasks: project.tasks.clone(),
                    queue: project.queue.clone(),
                    chats: project
                        .chats
                        .iter()
                        .map(|chat| Chat {
                            project_context: String::new(),
                            id: chat.id,
                            title: chat.title.clone(),
                            provider: chat.provider,
                            state: chat.state,
                            provider_session_id: chat.provider_session_id.clone(),
                            messages: Vec::new(),
                            wire_history: Vec::new(),
                            history_file: chat.history_file.clone(),
                            history_loaded: false,
                            usage: chat.usage.clone(),
                            draft: chat.draft.clone(),
                            draft_images: chat.draft_images.clone(),
                            message_images: chat.message_images.clone(),
                            model: chat.model.clone(),
                            reasoning_effort: chat.reasoning_effort.clone(),
                            usage_incomplete: chat.usage_incomplete,
                            last_status: chat.last_status.clone(),
                            approval: String::new(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn allocate_id(&mut self) -> Id {
        self.next_id += 1;
        self.next_id
    }

    pub fn add_project(&mut self, name: String, path: String) -> Id {
        let id = self.allocate_id();
        self.projects.push(Project {
            profile: Default::default(),
            id,
            name,
            path,
            chats: Vec::new(),
            tasks: Vec::new(),
            queue: None,
        });
        self.add_chat(id);
        id
    }

    pub fn save_task(
        &mut self,
        project_id: Id,
        task_id: Option<Id>,
        title: String,
        objective: String,
        acceptance: String,
        severity: TaskSeverity,
    ) -> Result<Id, String> {
        let title = title.trim().to_string();
        let objective = objective.trim().to_string();
        let acceptance = acceptance.trim().to_string();
        if title.is_empty() || objective.is_empty() || acceptance.is_empty() {
            return Err("Completá el título, objetivo y criterio de aceptación.".into());
        }
        if title.chars().count() > 200
            || objective.len() > 16 * 1024
            || acceptance.len() > 16 * 1024
        {
            return Err("El título admite 200 caracteres y cada descripción hasta 16 KiB.".into());
        }
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or("El proyecto ya no está disponible.")?;
        if task_id.is_some()
            && project
                .queue
                .as_ref()
                .is_some_and(|q| q.active_task == task_id && q.active_chat.is_some())
        {
            return Err("La tarea está ejecutándose. Esperá a que termine para editarla.".into());
        }
        if let Some(id) = task_id {
            if !project.tasks.iter().any(|task| task.id == id) {
                return Err("La tarea ya no está disponible.".into());
            }
        }
        let id = task_id.unwrap_or_else(|| self.allocate_id());
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .unwrap();
        if let Some(task) = project.tasks.iter_mut().find(|task| task.id == id) {
            task.title = title;
            task.objective = objective;
            task.acceptance = acceptance;
            task.severity = severity;
        } else {
            project.tasks.push(Task {
                id,
                title,
                objective,
                acceptance,
                severity,
                status: TaskStatus::Todo,
                runs: Vec::new(),
            });
        }
        Ok(id)
    }

    pub fn move_task(
        &mut self,
        project_id: Id,
        task_id: Id,
        status: TaskStatus,
    ) -> Result<(), String> {
        let task = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .and_then(|p| p.tasks.iter_mut().find(|task| task.id == task_id))
            .ok_or("La tarea no pertenece a este proyecto.")?;
        task.status = status;
        Ok(())
    }

    pub fn add_chat(&mut self, project_id: Id) -> Option<Id> {
        if !self.projects.iter().any(|p| p.id == project_id) {
            return None;
        }
        let id = self.allocate_id();
        let project = self.projects.iter_mut().find(|p| p.id == project_id)?;
        project.chats.push(Chat {
            project_context: String::new(),
            id,
            title: "Nuevo chat".into(),
            provider: Provider::Codex,
            state: RunState::Idle,
            provider_session_id: None,
            messages: Vec::new(),
            history_file: None,
            history_loaded: true,
            usage: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            message_images: Default::default(),
            model: String::new(),
            reasoning_effort: None,
            wire_history: Vec::new(),
            usage_incomplete: false,
            last_status: String::new(),
            approval: String::new(),
        });
        self.select_chat(id);
        Some(id)
    }

    pub fn chat(&self, id: Id) -> Option<&Chat> {
        self.projects
            .iter()
            .flat_map(|p| &p.chats)
            .find(|c| c.id == id)
    }

    pub fn chat_mut(&mut self, id: Id) -> Option<&mut Chat> {
        self.projects
            .iter_mut()
            .flat_map(|p| &mut p.chats)
            .find(|c| c.id == id)
    }

    pub fn select_project(&mut self, id: Id) {
        if !self.projects.iter().any(|project| project.id == id) {
            return;
        }
        if !self.open_project_tabs.contains(&id) {
            self.open_project_tabs.push(id);
        }
        self.selected_project = Some(id);
        self.selected_chat = None;
    }

    pub fn select_chat(&mut self, id: Id) {
        if self.chat(id).is_none() {
            return;
        }
        self.selected_project = None;
        self.selected_chat = Some(id);
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id);
        }
    }

    pub fn close_project_tab(&mut self, id: Id) {
        self.open_project_tabs.retain(|tab| *tab != id);
        if self.selected_project == Some(id) {
            self.selected_project = self.open_project_tabs.last().copied();
            if self.selected_project.is_none() {
                self.selected_chat = self.open_tabs.last().copied();
            }
        }
    }

    /// Closing a view does not alter or remove the session or its run state.
    pub fn close_tab(&mut self, id: Id) {
        self.open_tabs.retain(|tab| *tab != id);
        if self.selected_chat == Some(id) {
            self.selected_chat = self.open_tabs.last().copied();
        }
    }

    /// Remove a stopped chat and its navigation; task run summaries remain historical.
    pub fn delete_chat(&mut self, id: Id) -> Result<(), String> {
        let chat = self.chat(id).ok_or("El chat ya no está disponible.")?;
        if matches!(
            chat.state,
            RunState::Running | RunState::WaitingForInput | RunState::WaitingForPermission
        ) || self
            .projects
            .iter()
            .any(|p| p.queue.as_ref().is_some_and(|q| q.active_chat == Some(id)))
        {
            return Err("Detené la ejecución del chat antes de eliminarlo.".into());
        }
        self.close_tab(id);
        for project in &mut self.projects {
            project.chats.retain(|chat| chat.id != id);
        }
        Ok(())
    }

    /// A fresh process cannot assume a persisted execution is still alive.
    pub fn reconcile_after_restart(&mut self) {
        for chat in self.projects.iter_mut().flat_map(|p| &mut p.chats) {
            if matches!(
                chat.state,
                RunState::Running | RunState::WaitingForInput | RunState::WaitingForPermission
            ) {
                chat.state = RunState::Interrupted;
            }
        }
        self.reconcile_task_queues();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_tab_closure_preserve_running_chats() {
        let mut app = AppState::default();
        app.add_project("A".into(), "a".into());
        let first = app.selected_chat.unwrap();
        app.chat_mut(first).unwrap().state = RunState::Running;
        app.add_project("B".into(), "b".into());
        app.close_tab(first);
        assert_eq!(app.chat(first).unwrap().state, RunState::Running);
        app.select_chat(first);
        assert_eq!(app.chat(first).unwrap().state, RunState::Running);
    }

    #[test]
    fn project_overview_preserves_sessions_and_persists_selection() {
        let mut app = AppState::default();
        let project = app.add_project("A".into(), "a".into());
        let first = app.selected_chat.unwrap();
        app.chat_mut(first).unwrap().state = RunState::Running;
        app.chat_mut(first).unwrap().draft = "draft".into();
        let tabs = app.open_tabs.clone();
        app.select_project(project);
        assert_eq!(app.selected_chat, None);
        assert_eq!(app.open_tabs, tabs);
        app.select_project(project);
        assert_eq!(app.open_project_tabs, vec![project]);
        assert_eq!(app.chat(first).unwrap().state, RunState::Running);
        assert_eq!(app.chat(first).unwrap().draft, "draft");
        assert_eq!(app.metadata_snapshot().selected_project, Some(project));
        let loaded: AppState = serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
        assert_eq!(loaded.selected_project, Some(project));
        assert_eq!(loaded.open_project_tabs, vec![project]);
        app.select_chat(first);
        assert_eq!(app.selected_project, None);
        assert_eq!(app.selected_chat, Some(first));
        app.select_project(project);
        app.close_project_tab(project);
        assert!(app.open_project_tabs.is_empty());
        assert_eq!(app.selected_chat, Some(first));
    }

    #[test]
    fn tasks_validate_isolate_projects_and_survive_metadata_and_reload() {
        let mut app = AppState::default();
        let first = app.add_project("A".into(), "a".into());
        let second = app.add_project("B".into(), "b".into());
        assert!(
            app.save_task(
                first,
                None,
                " ".into(),
                "Goal".into(),
                "Accept".into(),
                TaskSeverity::High
            )
            .is_err()
        );
        let id = app
            .save_task(
                first,
                None,
                " Title ".into(),
                "Goal".into(),
                "Accept".into(),
                TaskSeverity::High,
            )
            .unwrap();
        assert!(app.move_task(second, id, TaskStatus::Done).is_err());
        app.move_task(first, id, TaskStatus::InProgress).unwrap();
        app.save_task(
            first,
            Some(id),
            "Updated".into(),
            "Goal 2".into(),
            "Accept 2".into(),
            TaskSeverity::Critical,
        )
        .unwrap();
        let project = &app.projects[0];
        assert_eq!(project.tasks.len(), 1);
        assert_eq!(project.tasks[0].status, TaskStatus::InProgress);
        assert_eq!(project.tasks[0].severity, TaskSeverity::Critical);
        app.move_task(first, id, TaskStatus::Done).unwrap();
        let loaded: AppState =
            serde_json::from_str(&serde_json::to_string(&app.metadata_snapshot()).unwrap())
                .unwrap();
        assert_eq!(loaded.projects[0].tasks[0].status, TaskStatus::Done);
        assert_eq!(loaded.projects[0].tasks[0].acceptance, "Accept 2");
        assert!(loaded.projects[1].tasks.is_empty());
        let mut legacy = serde_json::to_value(&app).unwrap();
        legacy["projects"][0]
            .as_object_mut()
            .unwrap()
            .remove("tasks");
        let loaded: AppState = serde_json::from_value(legacy).unwrap();
        assert!(loaded.projects[0].tasks.is_empty());
    }
    #[test]
    fn repeated_usage_reports_do_not_duplicate_consumption() {
        let mut app = AppState::default();
        app.add_project("A".into(), "a".into());
        let id = app.selected_chat.unwrap();
        let chat = app.chat_mut(id).unwrap();
        for output in [10, 20] {
            chat.record_usage(UsageRecord {
                context_input: None,
                context_window: None,
                request_id: "request-1".into(),
                model: "reported-model".into(),
                provider: Provider::Codex,
                usage: TokenUsage {
                    output: Some(output),
                    ..Default::default()
                },
            });
        }
        assert_eq!(chat.usage.len(), 1);
        assert_eq!(chat.usage[0].usage.output, Some(20));
        assert_eq!(chat.usage[0].usage.input, None);
    }
}
