use crate::{AppWindow, HttpRequestRow, explorer_ui};
use ascurra_harness::{
    http_client::{self, Collection, Config, Request, Response},
    runtime::Supervisor,
    storage::Store,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::watch;
const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
fn show(ui: &AppWindow, config: &Config, index: usize, selected: &str) {
    ui.set_http_collections(ModelRc::new(VecModel::from(
        config
            .collections
            .iter()
            .map(|c| c.name.clone().into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index = index.min(config.collections.len().saturating_sub(1));
    ui.set_http_collection_index(index as i32);
    let c = config.collections.get(index);
    ui.set_http_collection_name(c.map(|c| c.name.as_str()).unwrap_or("").into());
    ui.set_http_exposed(c.is_some_and(|c| c.exposed));
    let request = c.and_then(|c| {
        c.requests
            .iter()
            .find(|r| r.id == selected)
            .or_else(|| c.requests.first())
    });
    ui.set_http_request_names(ModelRc::new(VecModel::from(
        c.map(|c| {
            c.requests
                .iter()
                .map(|r| r.name.clone().into())
                .collect::<Vec<slint::SharedString>>()
        })
        .unwrap_or_default()
        .into_iter()
        .chain(std::iter::once("＋ Nueva request".into()))
        .collect::<Vec<slint::SharedString>>(),
    )));
    ui.set_http_request_index(
        c.and_then(|c| {
            c.requests
                .iter()
                .position(|r| Some(r.id.as_str()) == request.map(|r| r.id.as_str()))
        })
        .unwrap_or(0) as i32,
    );
    ui.set_http_requests(ModelRc::new(VecModel::from(
        c.map(|c| {
            c.requests
                .iter()
                .map(|r| HttpRequestRow {
                    id: r.id.clone().into(),
                    title: r.name.clone().into(),
                    method: r.method.clone().into(),
                    selected: request.is_some_and(|s| s.id == r.id),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default(),
    )));
    ui.set_http_request_id(request.map(|r| r.id.as_str()).unwrap_or("").into());
    ui.set_http_request_name(request.map(|r| r.name.as_str()).unwrap_or("").into());
    ui.set_http_method_index(
        request
            .and_then(|r| METHODS.iter().position(|m| *m == r.method))
            .unwrap_or(0) as i32,
    );
    ui.set_http_url(request.map(|r| r.url.as_str()).unwrap_or("").into());
    ui.set_http_headers(request.map(|r| r.headers.as_str()).unwrap_or("").into());
    ui.set_http_body(request.map(|r| r.body.as_str()).unwrap_or("").into());
    ui.set_http_saved_method_index(ui.get_http_method_index());
    ui.set_http_saved_url(ui.get_http_url());
    ui.set_http_saved_headers(ui.get_http_headers());
    ui.set_http_saved_body(ui.get_http_body());
    ui.set_http_saved_exposed(ui.get_http_exposed());
    ui.set_http_timeout(config.timeout_seconds as i32);
}
fn draft(ui: &AppWindow, config: &Config) -> Result<Config, String> {
    let mut config = config.clone();
    let c = config
        .collections
        .get_mut(ui.get_http_collection_index().max(0) as usize)
        .ok_or("Creá una colección primero.")?;
    c.name = ui.get_http_collection_name().to_string();
    c.exposed = ui.get_http_exposed();
    if let Some(r) = c
        .requests
        .iter_mut()
        .find(|r| r.id == ui.get_http_request_id().as_str())
    {
        r.name = ui.get_http_request_name().to_string();
        r.method = METHODS[ui.get_http_method_index().clamp(0, 6) as usize].into();
        r.url = ui.get_http_url().to_string();
        r.headers = ui.get_http_headers().to_string();
        r.body = ui.get_http_body().to_string();
    }
    http_client::validate(&config)?;
    Ok(config)
}
fn response(ui: &AppWindow, result: &Response) {
    let text = if ui.get_http_response_view() == 1 {
        result.headers.clone()
    } else {
        let prefix = &result.body[..result.body.len().min(64 * 1024)];
        if prefix.contains(&0) {
            "[Respuesta binaria: consultá el archivo guardado]".into()
        } else {
            String::from_utf8_lossy(prefix).into_owned()
        }
    };
    let (lines, columns) = explorer_ui::bounded_lines(text.into());
    ui.set_http_response_lines(lines);
    ui.set_http_response_columns(columns);
    ui.set_http_response_status(
        format!(
            "HTTP {} · {} ms · {} bytes{}",
            result.status,
            result.duration_ms,
            result.body.len(),
            if result.truncated || result.body.len() > 64 * 1024 {
                " · vista acotada"
            } else {
                ""
            }
        )
        .into(),
    );
}
pub fn configure(ui: &AppWindow, store: Rc<Store>, supervisor: Arc<Supervisor>) {
    let data = store.root().to_path_buf();
    let config = Arc::new(Mutex::new((String::new(), Config::default())));
    let version = Arc::new(AtomicU64::new(0));
    let active = Arc::new(Mutex::new(None::<watch::Sender<bool>>));
    let last = Arc::new(Mutex::new(None::<Response>));
    let weak = ui.as_weak();
    let cfg = config.clone();
    let root = data.clone();
    let sequence = version.clone();
    let service = supervisor.clone();
    let result = last.clone();
    ui.on_load_http(move || {
        let Some(ui)=weak.upgrade() else{return;};let project=ui.get_git_project().to_string();if project.is_empty(){return;}
        ui.set_http_ready(false); show(&ui,&Config::default(),0,""); ui.set_http_notice("Leyendo colecciones…".into());ui.set_http_response_lines(ModelRc::new(VecModel::from(Vec::new())));ui.set_http_response_status("".into());*result.lock().unwrap()=None;
        let generation=sequence.fetch_add(1,Ordering::Relaxed)+1;let sequence=sequence.clone();let root=root.clone();let cfg=cfg.clone();let handle=ui.as_weak();
        service.executor.spawn(async move {let source=project.clone();let loaded=tokio::task::spawn_blocking(move ||http_client::load(&root,std::path::Path::new(&source))).await;
            let _=handle.upgrade_in_event_loop(move |ui| {if sequence.load(Ordering::Relaxed)!=generation || ui.get_git_project().as_str()!=project{return;}match loaded {Ok(Ok(loaded))=>{ui.set_http_ready(true);ui.set_http_loaded_project(project.clone().into());show(&ui,&loaded,0,"");*cfg.lock().unwrap()=(project,loaded);ui.set_http_notice("Colecciones del proyecto. Las variables nunca se envían al contexto del agente.".into());},_=>{ui.set_http_loaded_project("".into());*cfg.lock().unwrap()=(String::new(),Config::default());show(&ui,&Config::default(),0,"");ui.set_http_notice("No se pudieron leer las colecciones; no se reemplazó el archivo.".into());}}});
        });
    });
    let weak = ui.as_weak();
    let cfg = config.clone();
    let result = last.clone();
    ui.on_pick_http_collection(move |index| {
        if let Some(ui) = weak.upgrade() {
            *result.lock().unwrap() = None;
            ui.set_http_response_lines(ModelRc::new(VecModel::from(Vec::new())));
            ui.set_http_response_status("".into());
            let cfg = cfg.lock().unwrap();
            show(&ui, &cfg.1, index.max(0) as usize, "");
        }
    });
    let weak = ui.as_weak();
    let cfg = config.clone();
    let result = last.clone();
    ui.on_pick_http_request(move |id| {
        if let Some(ui) = weak.upgrade() {
            *result.lock().unwrap() = None;
            let cfg = cfg.lock().unwrap();
            show(
                &ui,
                &cfg.1,
                ui.get_http_collection_index().max(0) as usize,
                &id,
            );
            ui.set_http_response_lines(ModelRc::new(VecModel::from(Vec::new())));
            ui.set_http_response_status("".into());
        }
    });
    let weak = ui.as_weak();
    let cfg = config.clone();
    let root = data.clone();
    let result = last.clone();
    ui.on_http_action(move |kind| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_http_busy() || !ui.get_http_ready() {
            return;
        }
        let mut current = cfg.lock().unwrap();
        if current.0 != ui.get_git_project().as_str() {
            return;
        }
        let mut next = current.1.clone();
        let index = ui.get_http_collection_index().max(0) as usize;
        let mut selected = ui.get_http_request_id().to_string();
        let mut target = index;
        let action = (|| -> Result<(), String> {
            match kind.as_str() {
                "collection" => {
                    next.collections.push(Collection {
                        id: uuid::Uuid::new_v4().simple().to_string(),
                        name: "Nueva colección".into(),
                        exposed: false,
                        requests: Vec::new(),
                    });
                    target = next.collections.len() - 1;
                    selected.clear();
                }
                "request" => {
                    let request = Request::default();
                    selected = request.id.clone();
                    next.collections
                        .get_mut(index)
                        .ok_or("Creá una colección primero.")?
                        .requests
                        .push(request);
                }
                "rename-collection" | "rename-request" => {
                    let name = ui.get_http_rename_name().trim().to_string();
                    let collection = next
                        .collections
                        .get_mut(index)
                        .ok_or("Seleccioná una colección.")?;
                    if kind == "rename-collection" {
                        collection.name = name;
                    } else {
                        collection
                            .requests
                            .iter_mut()
                            .find(|r| r.id == selected)
                            .ok_or("Seleccioná una request.")?
                            .name = name;
                    }
                }
                "save" => {
                    next = draft(&ui, &next)?;
                }
                "delete" => {
                    next.collections
                        .get_mut(index)
                        .ok_or("Seleccioná una colección.")?
                        .requests
                        .retain(|r| r.id != selected);
                    selected.clear();
                }
                "delete-collection" => {
                    if index < next.collections.len() {
                        next.collections.remove(index);
                    }
                    target = 0;
                    selected.clear();
                }
                "variables" => {
                    ui.set_http_timeout(next.timeout_seconds as i32);
                    ui.set_http_variables_json(
                        serde_json::to_string_pretty(&next.variables)
                            .unwrap_or_default()
                            .into(),
                    );
                    ui.set_http_variables_open(true);
                    return Ok(());
                }
                "save-variables" => {
                    next.timeout_seconds = ui.get_http_timeout() as u64;
                    next.variables = serde_json::from_str(ui.get_http_variables_json().as_str())
                        .map_err(
                            |_| "Las variables deben ser un objeto JSON con valores de texto.",
                        )?;
                }
                _ => return Err("Acción HTTP desconocida.".into()),
            }
            if kind == "variables" {
                return Ok(());
            }
            http_client::save(&root, std::path::Path::new(&current.0), &next)?;
            current.1 = next;
            if kind == "rename-collection" || kind == "rename-request" {
                // Refresh labels only: renaming must not discard the request draft.
                ui.set_http_collections(ModelRc::new(VecModel::from(
                    current
                        .1
                        .collections
                        .iter()
                        .map(|c| c.name.as_str().into())
                        .collect::<Vec<slint::SharedString>>(),
                )));
                if let Some(c) = current.1.collections.get(index) {
                    ui.set_http_collection_name(c.name.as_str().into());
                    ui.set_http_request_names(ModelRc::new(VecModel::from(
                        c.requests
                            .iter()
                            .map(|r| r.name.as_str().into())
                            .chain(std::iter::once("＋ Nueva request".into()))
                            .collect::<Vec<slint::SharedString>>(),
                    )));
                    ui.set_http_requests(ModelRc::new(VecModel::from(
                        c.requests
                            .iter()
                            .map(|r| HttpRequestRow {
                                id: r.id.as_str().into(),
                                title: r.name.as_str().into(),
                                method: r.method.as_str().into(),
                                selected: r.id == selected,
                            })
                            .collect::<Vec<_>>(),
                    )));
                    if let Some(r) = c.requests.iter().find(|r| r.id == selected) {
                        ui.set_http_request_name(r.name.as_str().into());
                    }
                }
                ui.set_http_rename_open(false);
                ui.set_http_notice("Nombre guardado en este proyecto.".into());
                return Ok(());
            }
            *result.lock().unwrap() = None;
            ui.set_http_response_lines(ModelRc::new(VecModel::from(Vec::new())));
            ui.set_http_response_status("".into());
            if kind != "save-variables" {
                show(&ui, &current.1, target, &selected);
            }
            if kind == "delete" {
                ui.set_http_rename_open(false);
            }
            ui.set_http_variables_open(false);
            ui.set_http_notice("Guardado en este proyecto.".into());
            Ok(())
        })();
        if let Err(error) = action {
            ui.set_http_notice(error.into());
        }
    });
    let weak = ui.as_weak();
    let cfg = config.clone();
    let root = data.clone();
    let running = active.clone();
    let result = last.clone();
    let service = supervisor.clone();
    ui.on_send_http(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_http_busy() {
            return;
        }
        let current = cfg.lock().unwrap();
        let project = ui.get_git_project().to_string();
        if current.0 != project {
            return;
        }
        let collection = ui.get_http_collection_index().max(0) as usize;
        let request = ui.get_http_request_id().to_string();
        let prepared = draft(&ui, &current.1).and_then(|c| {
            http_client::prepare(
                &c,
                c.collections
                    .get(collection)
                    .ok_or("Seleccioná una colección.")?
                    .id
                    .as_str(),
                &request,
                false,
            )
        });
        drop(current);
        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                ui.set_http_notice(e.into());
                return;
            }
        };
        let (tx, rx) = watch::channel(false);
        *running.lock().unwrap() = Some(tx);
        *result.lock().unwrap() = None;
        ui.set_http_response_lines(ModelRc::new(VecModel::from(Vec::new())));
        ui.set_http_response_status("".into());
        ui.set_http_busy(true);
        ui.set_http_notice("Enviando request…".into());
        let root = root.clone();
        let result = result.clone();
        let running = running.clone();
        let handle = ui.as_weak();
        service.executor.spawn(async move {
            let outcome = http_client::send(prepared, rx).await;
            let folder = http_client::folder(&root, &PathBuf::from(&project))
                .join("responses")
                .join(&request);
            let outcome = outcome.and_then(|r| http_client::retain(&folder, &r).map(|_| r));
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_http_busy(false);
                *running.lock().unwrap() = None;
                if ui.get_git_project().as_str() != project
                    || ui.get_http_request_id().as_str() != request
                {
                    return;
                }
                match outcome {
                    Ok(r) => {
                        response(&ui, &r);
                        *result.lock().unwrap() = Some(r);
                        ui.set_http_notice("Respuesta guardada. El envío manual no usa IA.".into());
                    }
                    Err(e) => ui.set_http_notice(e.into()),
                }
            });
        });
    });
    let running = active.clone();
    ui.on_cancel_http(move || {
        if let Some(cancel) = running.lock().unwrap().as_ref() {
            let _ = cancel.send(true);
        }
    });
    let weak = ui.as_weak();
    let result = last.clone();
    ui.on_show_http_response(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(r) = result.lock().unwrap().as_ref() {
                response(&ui, r);
            }
        }
    });
    let weak = ui.as_weak();
    let root = data;
    ui.on_open_http_results(move || {
        if let Some(ui) = weak.upgrade() {
            let path = http_client::folder(&root, &PathBuf::from(ui.get_git_project().as_str()))
                .join("responses");
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
