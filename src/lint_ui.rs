use crate::{AppWindow, ProblemRow};
use ascurra_harness::{
    lint::{self, Kind, Report, Tool},
    runtime::Supervisor,
    storage::Store,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::watch;
pub fn display(ui: &AppWindow, report: Report) {
    ui.set_lint_status(report.status.into());
    ui.set_lint_all_rows(ModelRc::new(VecModel::from(
        report
            .problems
            .into_iter()
            .map(|p| ProblemRow {
                path: p.path.into(),
                line: p.line.min(i32::MAX as u64) as i32,
                column: p.column.min(i32::MAX as u64) as i32,
                error: p.error,
                code: p.code.into(),
                message: p.message.into(),
            })
            .collect::<Vec<_>>(),
    )));
    ui.invoke_filter_lint();
}
pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>, store: Rc<Store>) {
    let weak = ui.as_weak();
    ui.on_filter_lint(move || {
        if let Some(ui) = weak.upgrade() {
            let query = ui.get_lint_filter().to_lowercase();
            let severity = ui.get_lint_severity();
            ui.set_lint_rows(ModelRc::new(VecModel::from(
                ui.get_lint_all_rows()
                    .iter()
                    .filter(|p| {
                        (severity == 0 || (severity == 1 && p.error) || (severity == 2 && !p.error))
                            && format!("{} {} {}", p.path, p.code, p.message)
                                .to_lowercase()
                                .contains(&query)
                    })
                    .collect::<Vec<_>>(),
            )));
        }
    });
    let logs = store.root().to_path_buf();
    let tools = Arc::new(Mutex::new((String::new(), Vec::<Tool>::new())));
    let generation = Arc::new(AtomicU64::new(0));
    let active = Arc::new(Mutex::new(None::<watch::Sender<bool>>));
    let weak = ui.as_weak();
    let version = generation.clone();
    let directory = logs.clone();
    let list = tools.clone();
    let service = supervisor.clone();
    ui.on_detect_lint(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_lint_open() {
            return;
        }
        let project = ui.get_git_project().to_string();
        if project.is_empty() {
            return;
        }
        let request = version.fetch_add(1, Ordering::Relaxed) + 1;
        let version = version.clone();
        let directory = directory.clone();
        let list = list.clone();
        let handle = ui.as_weak();
        ui.set_lint_status("Detectando linters…".into());
        service.executor.spawn(async move {
            let root = PathBuf::from(&project);
            let result = tokio::task::spawn_blocking(move || {
                (lint::detect(&root), lint::cached(&directory, &root))
            })
            .await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if version.load(Ordering::Relaxed) != request
                    || ui.get_git_project().as_str() != project
                {
                    return;
                }
                if let Ok((choices, report)) = result {
                    ui.set_lint_tools(ModelRc::new(VecModel::from(
                        choices
                            .iter()
                            .map(|t| t.name.as_str().into())
                            .collect::<Vec<slint::SharedString>>(),
                    )));
                    ui.set_lint_tool_index(0);
                    *list.lock().unwrap() = (project, choices);
                    display(
                        &ui,
                        report.map(|mut r|{r.status=format!("Última ejecución guardada · {}",r.status);r}).unwrap_or_else(|| Report {
                            problems: Vec::new(),
                            status: if ui.get_lint_tools().row_count() == 0 {
                                "No se detectó un linter. Configurá uno con el botón de ajustes."
                                    .into()
                            } else {
                                "Ejecutá el linter para consultar los problemas del proyecto."
                                    .into()
                            },
                        }),
                    );
                }
            });
        });
    });
    let weak = ui.as_weak();
    let service = supervisor.clone();
    let list = tools.clone();
    let directory = logs.clone();
    let lock = active.clone();
    let version = generation.clone();
    ui.on_run_lint(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_lint_busy() {
            return;
        }
        let root = ui.get_git_project().to_string();
        let list = list.lock().unwrap();
        if list.0 != root {
            return;
        }
        let Some(tool) = list
            .1
            .get(ui.get_lint_tool_index().max(0) as usize)
            .cloned()
        else {
            return;
        };
        drop(list);
        version.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = watch::channel(false);
        *lock.lock().unwrap() = Some(sender);
        ui.set_lint_busy(true);
        ui.set_lint_status(format!("Ejecutando {}…", tool.name).into());
        ui.set_lint_rows(ModelRc::default());
        ui.set_lint_all_rows(ModelRc::default());
        let handle = ui.as_weak();
        let directory = directory.clone();
        let lock = lock.clone();
        service.executor.spawn(async move {
            let report = lint::run(std::path::Path::new(&root), &tool, &directory, receiver).await;
            *lock.lock().unwrap() = None;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_lint_busy(false);
                if ui.get_git_project().as_str() == root {
                    display(&ui, report);
                } else {
                    ui.invoke_detect_lint();
                }
            });
        });
    });
    ui.on_stop_lint(move || {
        if let Some(sender) = active.lock().unwrap().as_ref() {
            let _ = sender.send(true);
        }
    });
    let weak = ui.as_weak();
    let directory = logs.clone();
    ui.on_open_lint_logs(move || {
        if let Some(ui) = weak.upgrade() {
            let folder = lint::folder(
                &directory,
                std::path::Path::new(ui.get_git_project().as_str()),
            );
            let _ = std::process::Command::new("explorer.exe")
                .arg(folder)
                .spawn();
        }
    });
    let weak = ui.as_weak();
    ui.on_save_lint_config(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let root = PathBuf::from(ui.get_git_project().as_str());
        let kinds = [
            Kind::Text,
            Kind::Json,
            Kind::Sarif,
            Kind::Checkstyle,
            Kind::Eslint,
            Kind::Clippy,
            Kind::Ruff,
            Kind::Android,
            Kind::Phpstan,
            Kind::Rubocop,
            Kind::Swiftlint,
            Kind::Dart,
            Kind::Shellcheck,
        ];
        let result = (|| -> Result<(), String> {
            let program = ui.get_lint_program().to_string();
            if program.trim().is_empty() {
                return Err("Indicá el ejecutable del linter.".into());
            }
            let args: Vec<String> = serde_json::from_str(&ui.get_lint_args())
                .map_err(|_| "Los argumentos deben ser un array JSON de strings.")?;
            if args.len() > 512 || args.iter().any(|s| s.len() > 4096) {
                return Err("Demasiados argumentos.".into());
            }
            let path = root.join(".harness/lint.json");
            let mut list: Vec<Tool> = if path.exists() {
                serde_json::from_slice(&lint::config_bytes(&path, 64 * 1024)?)
                    .map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            let name = ui.get_lint_name().to_string();
            let name = if name.trim().is_empty() {
                "Personalizado".into()
            } else {
                name
            };
            list.retain(|t| t.name != name);
            list.push(Tool {
                kind: *kinds
                    .get(ui.get_lint_format_index().max(0) as usize)
                    .unwrap_or(&Kind::Text),
                program: program.into(),
                args,
                name,
                reports: ui.get_lint_report_files(),
            });
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let base = root.canonicalize().map_err(|e| e.to_string())?;
            let parent = path
                .parent()
                .unwrap()
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !parent.starts_with(&base)
                || (path.exists()
                    && !path
                        .canonicalize()
                        .map_err(|e| e.to_string())?
                        .starts_with(&base))
            {
                return Err("La configuración debe quedar dentro del proyecto.".into());
            }
            let bytes = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
            if bytes.len() > 64 * 1024 {
                return Err("La configuración supera 64 KiB.".into());
            }
            std::fs::write(path, bytes).map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                ui.set_lint_config_open(false);
                ui.invoke_detect_lint();
            }
            Err(error) => ui.set_lint_config_error(error.into()),
        }
    });
}
