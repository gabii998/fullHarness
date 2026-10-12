use ascurra_harness::{
    http_client::{self, Collection, Config, Request},
    runtime::Supervisor,
    storage::Store,
};
use slint::{ComponentHandle, Model};
use std::{cell::Cell, rc::Rc};
slint::include_modules!();
#[allow(dead_code)]
#[path = "../src/explorer_ui.rs"]
mod explorer_ui;
#[path = "../src/http_ui.rs"]
mod http_ui;
#[test]
fn asynchronous_http_editor_loads_sends_and_persists_with_a_real_event_loop() {
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
    let project = temp.path().join("repo");
    std::fs::create_dir(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let store = Rc::new(Store::new(temp.path().join("data")).unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            socket.read_exact(&mut b).unwrap();
            header.push(b[0]);
        }
        let header = String::from_utf8(header).unwrap();
        assert!(header.starts_with("GET /health HTTP/1.1"));
        assert!(header.to_lowercase().contains("x-check: fixture-local"));
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            )
            .unwrap();
    });
    let mut config = Config::default();
    config
        .variables
        .insert("BASE_URL".into(), format!("http://{address}"));
    config
        .variables
        .insert("TOKEN".into(), "fixture-local".into());
    config.collections.push(Collection {
        id: "c".into(),
        name: "API local".into(),
        exposed: true,
        requests: vec![Request {
            id: "r".into(),
            name: "Estado del servicio".into(),
            url: "{{BASE_URL}}/health".into(),
            headers: "X-Check: {{TOKEN}}".into(),
            ..Default::default()
        }],
    });
    http_client::save(store.root(), &project, &config).unwrap();
    let ui = AppWindow::new().unwrap();
    let service = Supervisor::new(store.root()).unwrap();
    http_ui::configure(&ui, store.clone(), service);
    ui.set_has_project(true);
    ui.set_git_project(project.to_string_lossy().into_owned().into());
    ui.set_overview_name("Proyecto HTTP de prueba".into());
    let _ = ui.window().take_snapshot().unwrap();
    ui.invoke_open_http();
    let done = Rc::new(Cell::new(false));
    let completed = done.clone();
    let step = Rc::new(Cell::new(0));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    let started = std::time::Instant::now();
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
            if step.get() == 0 && ui.get_http_ready() {
                ui.set_http_request_name("Estado guardado".into());
                ui.invoke_http_action("save".into());
                ui.invoke_send_http();
                step.set(1);
            } else if step.get() == 1
                && !ui.get_http_busy()
                && started.elapsed() > std::time::Duration::from_millis(350)
            {
                assert_eq!(ui.get_active_panel_index(), 8);
                assert!(ui.get_workspace_visible_width() < ui.window().size().width as f32);
                completed.set(ui.get_http_response_status().contains("HTTP 200"));
                let snapshot = ui.window().take_snapshot().unwrap();
                let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("target/ui-http-preview.png");
                image::save_buffer(
                    path,
                    snapshot.as_bytes(),
                    snapshot.width(),
                    snapshot.height(),
                    image::ColorType::Rgba8,
                )
                .unwrap();
                let _ = ui.hide();
                let _ = slint::quit_event_loop();
            }
        },
    );
    ui.run().unwrap();
    assert!(
        done.get(),
        "{} / {}",
        ui.get_http_notice(),
        ui.get_http_response_status()
    );
    assert!(
        ui.get_http_response_lines()
            .row_data(0)
            .unwrap()
            .text
            .contains("ok")
    );
    server.join().unwrap();
    let saved = http_client::load(store.root(), &project).unwrap();
    assert_eq!(saved.collections[0].requests[0].name, "Estado guardado");
    assert!(saved.collections[0].exposed);
    assert_eq!(saved.variables["TOKEN"], "fixture-local");
    ui.set_http_open(false);
    ui.invoke_open_http();
    assert!(ui.get_http_open());
}
