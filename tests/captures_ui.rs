use ascurra_harness::{captures, domain::AppState, runtime::Supervisor, storage::Store};
use slint::{ComponentHandle, Model};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
slint::include_modules!();
#[path = "../src/captures_ui.rs"]
mod captures_ui;

#[test]
fn gallery_loads_real_images_filters_chat_and_excludes_other_projects() {
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
    let mut state = AppState::default();
    let p = state.add_project("Proyecto".into(), temp.path().to_string_lossy().into());
    let first = state.add_chat(p).unwrap();
    let second = state.add_chat(p).unwrap();
    let other = state.add_project("Otro".into(), "other".into());
    let excluded = state.add_chat(other).unwrap();
    let screenshot = temp.path().join("pantalla.png");
    image::RgbaImage::from_fn(640, 360, |x, y| {
        image::Rgba([((x / 3) % 255) as u8, ((y / 2) % 255) as u8, 120, 255])
    })
    .save(&screenshot)
    .unwrap();
    for chat in [first, second, excluded] {
        captures::import(
            &screenshot,
            &store
                .root()
                .join("artifacts")
                .join(chat.to_string())
                .join("run"),
        )
        .unwrap();
    }
    state.select_chat(first);
    let state = Rc::new(RefCell::new(state));
    let ui = AppWindow::new().unwrap();
    captures_ui::configure(
        &ui,
        state.clone(),
        store.clone(),
        Supervisor::new(store.root()).unwrap(),
    );
    ui.set_has_project(true);
    ui.set_has_chat(true);
    ui.set_git_project(temp.path().to_string_lossy().into_owned().into());
    ui.set_usage_selected_chat(first.to_string().into());
    let _ = ui.window().take_snapshot().unwrap();
    ui.invoke_select_panel(9);
    let done = Rc::new(Cell::new(false));
    let completed = done.clone();
    let step = Rc::new(Cell::new(0));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    let start = std::time::Instant::now();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(20),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if start.elapsed() > std::time::Duration::from_secs(10) {
                let _ = ui.hide();
                let _ = slint::quit_event_loop();
                return;
            }
            match step.get() {
                0 if ui.get_captures_rows().row_count() == 2 => {
                    assert_eq!(ui.get_active_panel_index(), 9);
                    ui.set_captures_scope(1);
                    step.set(1);
                }
                1 if ui.get_captures_rows().row_count() == 1 => {
                    assert_eq!(
                        ui.get_captures_rows().row_data(0).unwrap().detail,
                        format!("Chat {first}")
                    );
                    ui.invoke_pick_capture(0);
                    step.set(2);
                }
                2 if ui.get_captures_image().size().width > 0
                    && start.elapsed() > std::time::Duration::from_millis(400) =>
                {
                    assert_eq!(ui.get_captures_image().size().width, 640);
                    let snapshot = ui.window().take_snapshot().unwrap();
                    image::save_buffer(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("target/ui-captures-preview.png"),
                        snapshot.as_bytes(),
                        snapshot.width(),
                        snapshot.height(),
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    ui.set_capture_preview_open(true);
                    let _ = ui.window().take_snapshot().unwrap();
                    state.borrow_mut().select_chat(second);
                    ui.set_usage_selected_chat(second.to_string().into());
                    step.set(3);
                }
                3 if ui.get_captures_rows().row_count() == 1
                    && ui.get_captures_rows().row_data(0).unwrap().detail
                        == format!("Chat {second}") =>
                {
                    ui.set_captures_open(false);
                    assert!(!ui.get_capture_preview_open());
                    completed.set(true);
                    let _ = ui.hide();
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    ui.run().unwrap();
    assert!(
        done.get(),
        "La galería no completó la carga/filtro: {}",
        ui.get_captures_notice()
    );
}
