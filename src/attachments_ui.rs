use crate::{AppWindow, AttachmentRow};
use ascurra_harness::{
    attachments::{self, Attachment},
    domain::{AppState, RunState},
    runtime::Supervisor,
    storage::Store,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
};
type Inbox = Arc<Mutex<HashMap<u64, Vec<Result<Attachment, String>>>>>;
pub struct Inputs {
    pub files: Arc<dyn Fn() -> Vec<std::path::PathBuf> + Send + Sync>,
    pub clipboard:
        Arc<dyn Fn() -> Result<arboard::ImageData<'static>, arboard::Error> + Send + Sync>,
}
fn preserve(ui: &AppWindow, state: &mut AppState) {
    if let Some(chat) = state.selected_chat.and_then(|id| state.chat_mut(id)) {
        chat.draft = ui.get_draft().to_string();
    }
}

pub fn display(ui: &AppWindow, state: &AppState) {
    if ui.get_attachment_data_root().is_empty() {
        return;
    }
    let chat = state.selected_chat.and_then(|id| state.chat(id));
    let images = chat.map(|c| c.draft_images.as_slice()).unwrap_or(&[]);
    let previous = ui.get_attachment_rows().iter().collect::<Vec<_>>();
    if previous
        .iter()
        .map(|r| r.id.as_str())
        .eq(images.iter().map(|i| i.id.as_str()))
    {
        return;
    }
    let rows = images
        .iter()
        .map(|image| {
            let thumbnail = previous
                .iter()
                .find(|r| r.id.as_str() == image.id)
                .map(|r| r.image.clone())
                .unwrap_or_else(|| {
                    chat.and_then(|c| {
                        attachments::thumbnail_path(
                            Path::new(ui.get_attachment_data_root().as_str()),
                            c.id,
                            &image.id,
                        )
                        .ok()
                    })
                    .and_then(|p| slint::Image::load_from_path(&p).ok())
                    .unwrap_or_default()
                });
            AttachmentRow {
                id: image.id.clone().into(),
                name: image.name.clone().into(),
                image: thumbnail,
            }
        })
        .collect::<Vec<_>>();
    ui.set_attachment_rows(ModelRc::new(VecModel::from(rows)));
}

pub fn configure(
    ui: &AppWindow,
    state: Rc<RefCell<AppState>>,
    store: Rc<Store>,
    service: Arc<Supervisor>,
) {
    configure_with_inputs(
        ui,
        state,
        store,
        service,
        Inputs {
            files: Arc::new(|| {
                rfd::FileDialog::new()
                    .set_title("Adjuntar imágenes al chat")
                    .add_filter("Imágenes", &["png", "jpg", "jpeg"])
                    .pick_files()
                    .unwrap_or_default()
            }),
            clipboard: Arc::new(|| arboard::Clipboard::new().and_then(|mut c| c.get_image())),
        },
    );
}
pub fn configure_with_inputs(
    ui: &AppWindow,
    state: Rc<RefCell<AppState>>,
    store: Rc<Store>,
    service: Arc<Supervisor>,
    inputs: Inputs,
) {
    ui.set_attachment_data_root(store.root().to_string_lossy().into_owned().into());
    display(ui, &state.borrow());
    let inbox: Inbox = Arc::new(Mutex::new(HashMap::new()));
    let pending = Arc::new(Mutex::new(HashSet::<u64>::new()));
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    let results = inbox.clone();
    let busy = pending.clone();
    ui.on_apply_attachments(move |chat_key| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(id) = chat_key.parse::<u64>() else {
            return;
        };
        busy.lock().unwrap().remove(&id);
        let items = results.lock().unwrap().remove(&id).unwrap_or_default();
        let mut state = data.borrow_mut();
        preserve(&ui, &mut state);
        let mut notice = String::new();
        if let Some(chat) = state.chat_mut(id) {
            if matches!(
                chat.state,
                RunState::Running | RunState::WaitingForPermission
            ) {
                notice = "El chat ya está en ejecución; no se añadieron imágenes.".into();
            } else {
                let before = chat.draft_images.clone();
                for item in items {
                    match item {
                        Ok(image) if chat.draft_images.len() < attachments::MAX_IMAGES => {
                            chat.draft_images.push(image)
                        }
                        Ok(_) => notice = "Se admiten hasta cuatro imágenes por mensaje.".into(),
                        Err(error) => notice = error,
                    }
                }
                if let Err(error) = disk.save(&state) {
                    state.chat_mut(id).unwrap().draft_images = before;
                    notice = format!("No se pudieron guardar los adjuntos: {error}");
                }
            }
        }
        display(&ui, &state);
        if !notice.is_empty() {
            ui.set_notice(notice.into());
        }
        ui.set_attachment_busy(false);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let disk = store.clone();
    ui.on_remove_attachment(move |key| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = data.borrow_mut();
        preserve(&ui, &mut state);
        if let Some(chat) = state.selected_chat.and_then(|id| state.chat_mut(id)) {
            let before = chat.draft_images.clone();
            let id = chat.id;
            chat.draft_images.retain(|i| i.id != key.as_str());
            if let Err(error) = disk.save(&state) {
                state.chat_mut(id).unwrap().draft_images = before;
                ui.set_notice(format!("No se pudo quitar el adjunto: {error}").into());
            }
        }
        display(&ui, &state);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    let root = store.root().to_path_buf();
    let results = inbox.clone();
    let busy = pending.clone();
    let executor = service.clone();
    let files = inputs.files;
    ui.on_attach_images(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(id) = data.borrow().selected_chat else {
            return;
        };
        if ui.get_chat_running() || ui.get_attachment_busy() || !busy.lock().unwrap().insert(id) {
            return;
        }
        ui.set_attachment_busy(true);
        let handle = ui.as_weak();
        let root = root.clone();
        let results = results.clone();
        let files = files.clone();
        executor.executor.spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || {
                let paths = files();
                let too_many = paths.len() > attachments::MAX_IMAGES;
                let mut result = paths
                    .into_iter()
                    .take(attachments::MAX_IMAGES)
                    .map(|p| attachments::from_file(&root, id, &p))
                    .collect::<Vec<_>>();
                if too_many {
                    result.push(Err("Se admiten hasta cuatro imágenes por mensaje.".into()));
                }
                result
            })
            .await
            .unwrap_or_else(|_| vec![Err("No se pudieron adjuntar las imágenes.".into())]);
            results.lock().unwrap().insert(id, loaded);
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.invoke_apply_attachments(id.to_string().into())
            });
        });
    });
    let weak = ui.as_weak();
    let data = state;
    let root = store.root().to_path_buf();
    let results = inbox;
    let busy = pending;
    ui.on_paste_image(move || {
        let Some(ui) = weak.upgrade() else {
            return false;
        };
        let Some(id) = data.borrow().selected_chat else {
            return false;
        };
        if ui.get_chat_running() {
            return false;
        }
        let image = match (inputs.clipboard)() {
            Ok(image) => image,
            Err(_) => return false,
        };
        if ui.get_attachment_busy() {
            return true;
        }
        if ui.get_attachment_rows().row_count() >= attachments::MAX_IMAGES {
            ui.set_notice("Se admiten hasta cuatro imágenes por mensaje.".into());
            return true;
        }
        if !busy.lock().unwrap().insert(id) {
            return true;
        }
        ui.set_attachment_busy(true);
        let handle = ui.as_weak();
        let root = root.clone();
        let results = results.clone();
        let (width, height, pixels) = (image.width, image.height, image.bytes.into_owned());
        service.executor.spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || {
                attachments::from_clipboard(&root, id, width, height, &pixels)
            })
            .await
            .unwrap_or_else(|_| Err("No se pudo pegar la imagen.".into()));
            results.lock().unwrap().insert(id, vec![loaded]);
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.invoke_apply_attachments(id.to_string().into())
            });
        });
        true
    });
}
