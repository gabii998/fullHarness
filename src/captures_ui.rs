use crate::{AppWindow, CaptureRow};
use ascurra_harness::{
    captures::{self, Capture},
    domain::AppState,
    runtime::Supervisor,
    storage::Store,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub fn configure(
    ui: &AppWindow,
    state: Rc<RefCell<AppState>>,
    store: Rc<Store>,
    service: Arc<Supervisor>,
) {
    let rows = Arc::new(Mutex::new(Vec::<Capture>::new()));
    let generation = Arc::new(AtomicU64::new(0));
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    let list = rows.clone();
    let serial = generation.clone();
    let executor = service.clone();
    ui.on_refresh_captures(move || {
        let Some(ui) = weak.upgrade() else { return; };
        if !ui.get_captures_open() { return; }
        let Ok(state) = data.try_borrow() else {
            let handle = ui.as_weak();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = handle.upgrade() { ui.invoke_refresh_captures(); }
            });
            return;
        };
        let chats = state.projects.iter().find(|p| Some(p.id) == state.selected_project || p.chats.iter().any(|c|Some(c.id)==state.selected_chat)).map(|p| p.chats.iter().filter(|c|
            ui.get_captures_scope() == 0 || Some(c.id) == state.selected_chat).map(|c|c.id).collect::<Vec<_>>()).unwrap_or_default();
        drop(state);
        let root = disk.root().to_path_buf(); let handle = ui.as_weak();
        let sequence = serial.clone(); let id = sequence.fetch_add(1,Ordering::Relaxed)+1; let list = list.clone();
        ui.set_captures_image(slint::Image::default()); ui.set_captures_title("".into());
        ui.set_capture_preview_open(false);
        ui.set_captures_rows(ModelRc::new(VecModel::from(Vec::new())));
        ui.set_captures_notice("Buscando imágenes…".into());
        executor.executor.spawn(async move {
            let loaded = tokio::task::spawn_blocking(move ||captures::list(&root,&chats)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if sequence.load(Ordering::Relaxed)!=id || !ui.get_captures_open() {return;}
                match loaded {
                    Ok((items,partial)) => {
                        ui.set_captures_rows(ModelRc::new(VecModel::from(items.iter().enumerate().map(|(index,c)|CaptureRow {
                            index: index as i32, name:c.name.clone().into(), detail:format!("Chat {}",c.chat).into()
                        }).collect::<Vec<_>>())));
                        ui.set_captures_notice(if items.is_empty() {"Todavía no hay imágenes devueltas por el agente en este proyecto o chat.".into()}
                            else {format!("{} imágenes{}",items.len(),if partial {" · listado parcial"} else {""}).into()});
                        *list.lock().unwrap() = items;
                    }, Err(_) => ui.set_captures_notice("No se pudieron consultar las imágenes.".into())
                }
            });
        });
    });
    let weak = ui.as_weak();
    let list = rows.clone();
    let serial = generation.clone();
    let executor = service.clone();
    let preview_slot = Arc::new(tokio::sync::Semaphore::new(1));
    ui.on_pick_capture(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(item) = list.lock().unwrap().get(index.max(0) as usize).cloned() else {
            return;
        };
        let sequence = serial.clone();
        let id = sequence.fetch_add(1, Ordering::Relaxed) + 1;
        ui.set_captures_image(slint::Image::default());
        ui.set_captures_title(item.name.clone().into());
        ui.set_captures_notice("Cargando imagen…".into());
        let handle = ui.as_weak();
        let preview_slot = preview_slot.clone();
        executor.executor.spawn(async move {
            let Ok(_permit) = preview_slot.acquire().await else {
                return;
            };
            if sequence.load(Ordering::Relaxed) != id {
                return;
            }
            let loaded = tokio::task::spawn_blocking(move || captures::preview(&item.path)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if sequence.load(Ordering::Relaxed) != id || !ui.get_captures_open() {
                    return;
                }
                match loaded {
                    Ok(Ok((pixels, w, h))) => {
                        let buffer =
                            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                &pixels, w, h,
                            );
                        ui.set_captures_image(slint::Image::from_rgba8(buffer));
                        ui.set_captures_notice(
                            "Vista previa. La imagen permanece fuera del contexto del agente."
                                .into(),
                        );
                    }
                    Ok(Err(error)) => ui.set_captures_notice(error.into()),
                    Err(_) => ui.set_captures_notice("No se pudo cargar la imagen.".into()),
                }
            });
        });
    });
}
