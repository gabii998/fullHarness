use ascurra_harness::{attachments, domain::AppState, runtime::Supervisor, storage::Store};
use slint::{ComponentHandle, Model};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
slint::include_modules!();
#[allow(dead_code)]
#[path = "../src/attachments_ui.rs"]
mod attachments_ui;
fn click(ui: &AppWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
}
fn ctrl_v(ui: &AppWindow) {
    let control: char = slint::platform::Key::Control.into();
    for c in [control, 'v'] {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: c.into() });
    }
    for c in ['v', control] {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: c.into() });
    }
}
#[test]
fn image_button_and_ctrl_v_preserve_chat_drafts_and_removable_thumbnails() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            threading: true,
            mock_time: false,
            ..Default::default()
        },
    )))
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let store = Rc::new(Store::new(temp.path().join("data")).unwrap());
    let source = temp.path().join("diagrama.png");
    image::RgbaImage::from_pixel(64, 48, image::Rgba([100, 80, 180, 255]))
        .save(&source)
        .unwrap();
    let mut state = AppState::default();
    state.add_project("Uno".into(), "one".into());
    let first = state.selected_chat.unwrap();
    state.chat_mut(first).unwrap().draft = "Compará estas imágenes".into();
    state.add_project("Dos".into(), "two".into());
    let second = state.selected_chat.unwrap();
    state.select_chat(first);
    let state = Rc::new(RefCell::new(state));
    let ui = AppWindow::new().unwrap();
    let clipboard = Arc::new(std::sync::Mutex::new(Some(arboard::ImageData {
        width: 8,
        height: 6,
        bytes: std::borrow::Cow::Owned([5, 10, 15, 255].repeat(48)),
    })));
    let image_clipboard = clipboard.clone();
    attachments_ui::configure_with_inputs(
        &ui,
        state.clone(),
        store.clone(),
        Supervisor::new(store.root()).unwrap(),
        attachments_ui::Inputs {
            files: Arc::new(move || vec![source.clone()]),
            clipboard: Arc::new(move || {
                image_clipboard
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or(arboard::Error::ContentNotAvailable)
            }),
        },
    );
    ui.set_has_chat(true);
    ui.set_has_project(true);
    ui.set_selected_agent_configured(true);
    ui.set_draft("Compará estas imágenes".into());
    ui.show().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 820));
    let snapshot = ui.window().take_snapshot().unwrap();
    image::save_buffer(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ui-attachments-before.png"),
        snapshot.as_bytes(),
        snapshot.width(),
        snapshot.height(),
        image::ColorType::Rgba8,
    )
    .unwrap();
    // Click the attachment icon in the composer, using a fixture file picker.
    click(&ui, 348.0, 770.0);
    assert!(
        ui.get_attachment_busy(),
        "El botón debe iniciar la carga de imágenes"
    );
    state.borrow_mut().select_chat(second);
    attachments_ui::display(&ui, &state.borrow());
    ui.set_draft("Segundo chat".into());
    let completed = Rc::new(Cell::new(false));
    let done = completed.clone();
    let step = Rc::new(Cell::new(0));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    let started = std::time::Instant::now();
    let disk = store.clone();
    let data = state.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(20),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if started.elapsed() > std::time::Duration::from_secs(10) {
                let _ = ui.hide();
                let _ = slint::quit_event_loop();
                return;
            }
            match step.get() {
                0 if !ui.get_attachment_busy() => {
                    assert!(data.borrow().chat(second).unwrap().draft_images.is_empty());
                    assert_eq!(data.borrow().chat(first).unwrap().draft_images.len(), 1);
                    assert_eq!(ui.get_attachment_rows().row_count(), 0);
                    data.borrow_mut().select_chat(first);
                    ui.set_draft("Compará estas imágenes".into());
                    attachments_ui::display(&ui, &data.borrow());
                    assert_eq!(ui.get_attachment_rows().row_count(), 1);
                    let _ = ui.window().take_snapshot().unwrap();
                    // Focus the text editor, then send the actual Ctrl+V key combination.
                    click(&ui, 500.0, 710.0);
                    assert_eq!(
                        data.borrow().chat(first).unwrap().draft_images.len(),
                        1,
                        "El click debe enfocar el editor"
                    );
                    ctrl_v(&ui);
                    assert!(ui.get_attachment_busy(), "Ctrl+V debe consumir una imagen");
                    step.set(1);
                }
                1 if !ui.get_attachment_busy() => {
                    step.set(2);
                    assert_eq!(
                        ui.get_attachment_rows().row_count(),
                        2,
                        "{}; selected {:?}; first {}; second {}",
                        ui.get_notice(),
                        data.borrow().selected_chat,
                        data.borrow().chat(first).unwrap().draft_images.len(),
                        data.borrow().chat(second).unwrap().draft_images.len()
                    );
                    assert!(
                        !ui.invoke_paste_image(),
                        "Sin imagen debe conservar el pegado de texto"
                    );
                    let snapshot = ui.window().take_snapshot().unwrap();
                    image::save_buffer(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("target/ui-attachments-preview.png"),
                        snapshot.as_bytes(),
                        snapshot.width(),
                        snapshot.height(),
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    let saved = disk.load().unwrap();
                    assert_eq!(saved.chat(first).unwrap().draft_images.len(), 2);
                    let first_id = ui.get_attachment_rows().row_data(0).unwrap().id;
                    ui.invoke_remove_attachment(first_id);
                    assert_eq!(ui.get_attachment_rows().row_count(), 1);
                    assert_eq!(
                        disk.load().unwrap().chat(first).unwrap().draft_images.len(),
                        1
                    );
                    assert_eq!(
                        data.borrow().chat(first).unwrap().draft,
                        "Compará estas imágenes"
                    );
                    assert!(
                        attachments::codex_message(
                            "",
                            &data.borrow().chat(first).unwrap().draft_images
                        )["content"]
                            .is_array()
                    );
                    done.set(true);
                    let _ = ui.hide();
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    ui.run().unwrap();
    assert!(completed.get(), "{}", ui.get_notice());
}
