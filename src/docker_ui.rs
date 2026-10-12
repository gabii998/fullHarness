use crate::{AppWindow, DockerRow, LogcatRow};
use ascurra_harness::{
    docker::{Client, Snapshot},
    runtime::Supervisor,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub fn display(ui: &AppWindow, snapshot: Snapshot) {
    ui.set_docker_compose(snapshot.compose.is_some());
    let rows = snapshot
        .containers
        .into_iter()
        .map(|c| {
            let mut ports = Vec::new();
            if let Some(map) = c.ports.as_object() {
                for (target, bindings) in map {
                    if let Some(bindings) = bindings.as_array() {
                        for binding in bindings {
                            if let Some(port) = binding["HostPort"].as_str() {
                                ports.push(format!("{port} → {target}"));
                            }
                        }
                    }
                }
            }
            DockerRow {
                id: c.id.into(),
                name: c.name.trim_start_matches('/').into(),
                detail: format!(
                    "{} · {}{}",
                    c.service,
                    c.image,
                    if ports.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", ports.join(" · "))
                    }
                )
                .into(),
                status: format!(
                    "{}{}",
                    c.state,
                    if c.health.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", c.health)
                    }
                )
                .into(),
                running: c.state == "running",
                paused: c.state == "paused",
            }
        })
        .collect::<Vec<_>>();
    ui.set_docker_status(if rows.is_empty() {
        "No hay contenedores Compose asociados a este proyecto.".into()
    } else {
        format!("{} contenedores del proyecto", rows.len()).into()
    });
    ui.set_docker_rows(ModelRc::new(VecModel::from(rows)));
}
pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>) {
    let generation = Arc::new(AtomicU64::new(0));
    let operation = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let weak = ui.as_weak();
    let executor = supervisor.clone();
    let version = generation.clone();
    let lock = operation.clone();
    ui.on_refresh_docker(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_docker_open() || ui.get_git_project().is_empty() || lock.load(Ordering::Relaxed)
        {
            return;
        }
        let root = ui.get_git_project().to_string();
        let request = version.fetch_add(1, Ordering::Relaxed) + 1;
        let version = version.clone();
        let handle = ui.as_weak();
        ui.set_docker_busy(true);
        ui.set_docker_status("Consultando Docker…".into());
        executor.executor.spawn(async move {
            let result = Client::default().snapshot(Path::new(&root)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if version.load(Ordering::Relaxed) != request {
                    return;
                }
                ui.set_docker_busy(false);
                if ui.get_git_project().as_str() != root {
                    ui.invoke_refresh_docker();
                    return;
                }
                match result {
                    Ok(snapshot) => display(&ui, snapshot),
                    Err(error) => {
                        ui.set_docker_rows(ModelRc::default());
                        ui.set_docker_compose(false);
                        ui.set_docker_status(error.chars().take(1200).collect::<String>().into());
                    }
                }
            });
        });
    });
    let weak = ui.as_weak();
    ui.on_docker_action(move |id, action| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_git_project().is_empty()
            || ui.get_docker_busy()
            || operation.swap(true, Ordering::Relaxed)
        {
            return;
        }
        let root = ui.get_git_project().to_string();
        let id = id.to_string();
        let action = action.to_string();
        let handle = ui.as_weak();
        let lock = operation.clone();
        generation.fetch_add(1, Ordering::Relaxed);
        ui.set_docker_busy(true);
        ui.set_docker_status("Ejecutando operación…".into());
        if action == "logs" {
            ui.set_docker_logs_open(true);
            ui.set_docker_log_title("Leyendo logs…".into());
            ui.set_docker_logs(ModelRc::default());
        }
        supervisor.executor.spawn(async move {
            let client = Client::default();
            let result = if action == "up" {
                client.up(Path::new(&root)).await
            } else {
                client.action(Path::new(&root), &id, &action).await
            };
            let _ = handle.upgrade_in_event_loop(move |ui| {
                lock.store(false, Ordering::Relaxed);
                ui.set_docker_busy(false);
                if ui.get_git_project().as_str() != root {
                    ui.invoke_refresh_docker();
                    return;
                }
                match result {
                    Ok(text) if action == "logs" => {
                        ui.set_docker_log_title("Logs · últimas 200 líneas".into());
                        let lines = text
                            .lines()
                            .rev()
                            .take(200)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .map(|line| LogcatRow {
                                text: line.chars().take(4096).collect::<String>().into(),
                                level: "".into(),
                            })
                            .collect::<Vec<_>>();
                        ui.set_docker_logs(ModelRc::new(VecModel::from(lines)));
                        ui.set_docker_status("Logs consultados.".into());
                    }
                    Ok(text) => {
                        ui.invoke_refresh_docker();
                        ui.set_docker_status(text.into());
                    }
                    Err(error) => {
                        ui.set_docker_status(error.chars().take(1200).collect::<String>().into());
                        if action == "logs" {
                            ui.set_docker_log_title("No se pudieron leer los logs".into());
                        }
                    }
                }
            });
        });
    });
}
