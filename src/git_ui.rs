use crate::{AppWindow, FileTabRow, GitCommitRow, GitRow};
use ascurra_harness::{
    git::{self, Action, GitFile, LineChanges, Snapshot},
    runtime::Supervisor,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::Cell, path::Path, rc::Rc, sync::Arc};

fn change_rows(files: &[GitFile]) -> Vec<GitRow> {
    let clean = |value: &str| {
        value
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect::<String>()
    };
    let mut rows = Vec::new();
    for staged in [true, false] {
        let group = files
            .iter()
            .filter(|file| if staged { file.staged } else { file.unstaged })
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        rows.push(GitRow {
            header: true,
            prepared: staged,
            name: format!(
                "{} ({})",
                if staged { "Preparados" } else { "Cambios" },
                group.len()
            )
            .into(),
            ..Default::default()
        });
        for file in group {
            let lines = if staged {
                &file.staged_lines
            } else {
                &file.unstaged_lines
            };
            let (added, removed, stats) = match lines {
                Some(LineChanges::Text { added, removed }) => {
                    (format!("+{added}"), format!("−{removed}"), "")
                }
                Some(LineChanges::Binary) => (String::new(), String::new(), "Binario"),
                None => (String::new(), String::new(), "—"),
            };
            let (directory, name) = file.target.rsplit_once('/').unwrap_or(("", &file.target));
            rows.push(GitRow {
                path: file.path.clone().into(),
                target: file.target.clone().into(),
                status: file.status.clone().into(),
                staged: file.staged,
                unstaged: file.unstaged,
                prepared: staged,
                name: clean(name).into(),
                directory: clean(directory).into(),
                code: if staged {
                    file.index_code.clone()
                } else {
                    file.worktree_code.clone()
                }
                .into(),
                added: added.into(),
                removed: removed.into(),
                stats: stats.into(),
                ..Default::default()
            });
        }
    }
    rows
}

fn display(ui: &AppWindow, result: Result<Snapshot, String>) {
    match result {
        Ok(snapshot) => {
            ui.set_git_known(true);
            ui.set_git_repository(snapshot.repository);
            ui.set_git_remote(snapshot.remote);
            ui.set_git_origin_url(snapshot.origin_url.unwrap_or_default().into());
            ui.set_git_author_name(snapshot.user_name.unwrap_or_default().into());
            ui.set_git_author_email(snapshot.user_email.unwrap_or_default().into());
            let mut branches = snapshot.branches;
            let selected = match snapshot.current_branch {
                Some(current) => branches
                    .iter()
                    .position(|name| name == &current)
                    .unwrap_or(0),
                None if snapshot.repository => {
                    branches.insert(0, "HEAD separado".into());
                    0
                }
                None => 0,
            };
            ui.set_git_branches(ModelRc::new(VecModel::from(
                branches
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<slint::SharedString>>(),
            )));
            ui.set_git_branch_index(selected as i32);
            ui.set_git_history(ModelRc::new(VecModel::from(
                snapshot
                    .history
                    .into_iter()
                    .map(|commit| GitCommitRow {
                        title: commit.title.into(),
                        detail: format!(
                            "{} · {} · {}",
                            &commit.id[..commit.id.len().min(8)],
                            commit.author,
                            commit.date
                        )
                        .into(),
                        id: commit.id.into(),
                    })
                    .collect::<Vec<_>>(),
            )));
            if let Some(status) = snapshot.status {
                ui.set_git_dirty(!status.files.is_empty());
                ui.set_git_has_staged(status.files.iter().any(|file| file.staged));
                ui.set_git_status("".into());
                ui.set_git_files(ModelRc::new(VecModel::from(change_rows(&status.files))));
            } else {
                ui.set_git_dirty(false);
                ui.set_git_has_staged(false);
                ui.set_git_files(ModelRc::new(VecModel::from(Vec::<GitRow>::new())));
                ui.set_git_status("Esta carpeta no pertenece a un repositorio Git.".into());
            }
        }
        Err(error) => {
            ui.set_git_known(false);
            ui.set_git_repository(false);
            ui.set_git_status(error.into());
        }
    }
}

pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>) {
    let pending = Rc::new(Cell::new(false));
    let again = pending.clone();
    let weak = ui.as_weak();
    ui.on_git_query_finished(move || {
        if again.replace(false) {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_refresh_git();
            }
        }
    });
    let weak = ui.as_weak();
    let service = supervisor.clone();
    ui.on_refresh_git(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_git_open() {
            return;
        }
        if ui.get_git_busy() {
            pending.set(true);
            return;
        }
        pending.set(false);
        let path = ui.get_git_project().to_string();
        if path.is_empty() {
            ui.set_git_status("Abrí un proyecto para consultar Git.".into());
            return;
        }
        ui.set_git_busy(true);
        ui.set_git_status("Consultando el repositorio…".into());
        let handle = ui.as_weak();
        service.executor.spawn(async move {
            let result = git::snapshot(Path::new(&path)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_git_busy(false);
                if ui.get_git_project().as_str() != path {
                    ui.invoke_refresh_git();
                    return;
                }
                display(&ui, result);
                ui.invoke_git_query_finished();
            });
        });
    });
    let weak = ui.as_weak();
    let service = supervisor.clone();
    ui.on_git_action(move |kind, value| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_git_busy() || ui.get_git_project().is_empty() {
            return;
        }
        let value = value.to_string();
        let action = match kind.as_str() {
            "init" if ui.get_git_known() && !ui.get_git_repository() => Action::Init,
            "stage-all" if ui.get_git_repository() => Action::StageAll,
            "stage" if ui.get_git_repository() => Action::Stage(value),
            "unstage" if ui.get_git_repository() => Action::Unstage(value),
            "commit" if ui.get_git_repository() => Action::Commit(value),
            "remote" if ui.get_git_repository() => Action::Remote(value),
            "settings" if ui.get_git_repository() => Action::Settings {
                name: ui.get_git_user_name().to_string(),
                email: ui.get_git_user_email().to_string(),
                origin: ui.get_git_remote_url().to_string(),
            },
            "switch" if ui.get_git_repository() && !ui.get_git_dirty() => Action::Switch(value),
            "pull" if ui.get_git_repository() && ui.get_git_remote() => Action::Pull,
            "push" if ui.get_git_repository() && ui.get_git_remote() => Action::Push,
            _ => return,
        };
        let commit = matches!(action, Action::Commit(_));
        let remote = matches!(action, Action::Remote(_) | Action::Settings { .. });
        let path = ui.get_git_project().to_string();
        ui.set_git_busy(true);
        ui.set_git_action_message("Ejecutando Git…".into());
        let handle = ui.as_weak();
        service.executor.spawn(async move {
            let result = git::perform(Path::new(&path), action).await;
            let snapshot = git::snapshot(Path::new(&path)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_git_busy(false);
                if ui.get_git_project().as_str() != path {
                    ui.invoke_refresh_git();
                    return;
                }
                let succeeded = result.is_ok();
                ui.set_git_action_message(
                    result
                        .unwrap_or_else(|error| format!("No se completó: {error}"))
                        .into(),
                );
                if remote && succeeded {
                    ui.set_git_settings_open(false);
                }
                if commit && succeeded {
                    ui.set_git_commit_message("".into());
                }
                display(&ui, snapshot);
                if ui.get_files_open() {
                    ui.invoke_refresh_explorer(true);
                }
                ui.invoke_git_query_finished();
            });
        });
    });
    let weak = ui.as_weak();
    let service = supervisor.clone();
    ui.on_git_diff(move |target, commit| {
        if let Some(ui) = weak.upgrade() {
            open_diff(&ui, &service, target.to_string(), commit, None);
        }
    });
    let weak = ui.as_weak();
    ui.on_git_file_diff(move |target, staged| {
        if let Some(ui) = weak.upgrade() {
            open_diff(&ui, &supervisor, target.to_string(), false, Some(staged));
        }
    });
}
pub(crate) fn pending_diff_tab(
    ui: &AppWindow,
    path: &str,
    target: &str,
    commit: bool,
    scope: Option<bool>,
) -> FileTabRow {
    let kind = if commit {
        "commit"
    } else if scope == Some(true) {
        "preparados"
    } else {
        "cambios"
    };
    let title = if commit {
        format!("Commit {}", &target[..target.len().min(8)])
    } else {
        format!(
            "{} · {}",
            if scope == Some(true) {
                "Preparados"
            } else {
                "Cambios"
            },
            target.rsplit('/').next().unwrap_or(target)
        )
    };
    let tab = FileTabRow {
        project: path.into(),
        path: format!("[Git]/{kind}/{target}").into(),
        title: title.into(),
        text: "".into(),
        status: "Leyendo diferencias…".into(),
    };
    let mut tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
    if let Some(existing) = tabs
        .iter_mut()
        .find(|existing| existing.project == tab.project && existing.path == tab.path)
    {
        *existing = tab.clone();
    } else {
        tabs.push(tab.clone());
    }
    ui.set_file_tabs(ModelRc::new(VecModel::from(tabs)));
    ui.invoke_select_file_tab(tab.project.clone(), tab.path.clone());
    tab
}
pub(crate) fn complete_diff_tab(
    ui: &AppWindow,
    pending: &FileTabRow,
    result: Result<String, String>,
) {
    let mut tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
    let Some(tab) = tabs
        .iter_mut()
        .find(|tab| tab.project == pending.project && tab.path == pending.path)
    else {
        return;
    };
    match result {
        Ok(text) => {
            tab.text = text.into();
            tab.status = "Solo lectura · Git · UTF-8".into();
        }
        Err(error) => {
            tab.text = error.into();
            tab.status = "No se pudieron leer las diferencias".into();
        }
    }
    crate::explorer_ui::update_open_file(ui, tab);
    ui.set_file_tabs(ModelRc::new(VecModel::from(tabs)));
}
fn open_diff(
    ui: &AppWindow,
    supervisor: &Arc<Supervisor>,
    target: String,
    commit: bool,
    scope: Option<bool>,
) {
    if ui.get_git_busy() || !ui.get_git_repository() {
        return;
    }
    let path = ui.get_git_project().to_string();
    ui.set_git_busy(true);
    let pending = pending_diff_tab(ui, &path, &target, commit, scope);
    let handle = ui.as_weak();
    supervisor.executor.spawn(async move {
        let result = if let Some(staged) = scope {
            git::file_diff(Path::new(&path), &target, staged).await
        } else {
            git::diff(Path::new(&path), &target, commit).await
        };
        let _ = handle.upgrade_in_event_loop(move |ui| {
            ui.set_git_busy(false);
            complete_diff_tab(&ui, &pending, result);
            if ui.get_git_project().as_str() != path {
                ui.invoke_refresh_git();
            } else {
                ui.invoke_git_query_finished();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partially_staged_files_have_two_rows_with_independent_statistics() {
        let file = GitFile {
            path: "src/main.rs".into(),
            target: "src/main.rs".into(),
            original: None,
            staged: true,
            unstaged: true,
            status: "Preparado (M) · En trabajo (M)".into(),
            index_code: "M".into(),
            worktree_code: "M".into(),
            staged_lines: Some(LineChanges::Text {
                added: 12,
                removed: 4,
            }),
            unstaged_lines: Some(LineChanges::Text {
                added: 3,
                removed: 1,
            }),
        };
        let rows = change_rows(&[file]);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].header && rows[0].name == "Preparados (1)");
        assert!(rows[1].prepared && rows[1].added == "+12" && rows[1].removed == "−4");
        assert!(rows[2].header && rows[2].name == "Cambios (1)");
        assert!(!rows[3].prepared && rows[3].added == "+3" && rows[3].removed == "−1");
        assert_eq!(rows[1].target, rows[3].target);
    }
}
