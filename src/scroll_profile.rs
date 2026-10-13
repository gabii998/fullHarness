//! Opt-in scroll timing. No input is injected and no content or filenames are logged.
use crate::AppWindow;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, winit_030::{WinitWindowAccessor, winit}};
use std::{cell::{Cell, RefCell}, rc::Rc, path::Path, time::Instant, sync::mpsc::{SyncSender, TrySendError}};

#[derive(Clone)]
pub struct Handle {
    enabled: Rc<Cell<bool>>,
    state: Rc<RefCell<State>>,
}
#[derive(Default)]
struct State {
    sender: Option<SyncSender<Value>>,
    cursor_x: f64,
    cursor_y: f64,
    pane: &'static str,
    pending: Option<Instant>,
    last_wheel: Option<Instant>,
    before: Option<Instant>,
    previous_frame: Option<Instant>,
    started: Option<Instant>,
    wheel_events: u32,
    rows: [u32; 4],
    dropped: u32,
    sequence: u64,
}
impl Handle {
    pub fn enabled(&self) -> bool { self.enabled.get() }
    pub fn event(&self, ui: &AppWindow, window: &slint::Window, event: &winit::event::WindowEvent) {
        if !self.enabled() { return; }
        let mut state = self.state.borrow_mut();
        match event {
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                state.cursor_x = position.x;
                state.cursor_y = position.y;
            }
            winit::event::WindowEvent::MouseWheel { .. } => {
                let pane = window.with_winit_window(|native| {
                    let x = state.cursor_x / native.scale_factor();
                    let y = state.cursor_y / native.scale_factor();
                    let width = native.inner_size().width as f64 / native.scale_factor();
                    let sidebar = x >= width - ui.get_side_panel_visible_width() as f64 - 14.0;
                    if y < 58.0 || ui.get_settings_open() { "other" }
                    else if sidebar && ui.get_git_open() {
                        if ui.get_git_history_view() { "git_history" } else { "git_changes" }
                    } else if sidebar && ui.get_http_open() { "http" }
                    else if sidebar { "other" }
                    else if ui.get_file_preview_open() { "file" }
                    else { "chat" }
                }).unwrap_or("other");
                let now = Instant::now();
                if state.pane != pane {
                    state.pane = pane;
                    state.pending = None;
                    state.previous_frame = None;
                    state.wheel_events = 0;
                    state.rows = [0; 4];
                }
                state.pending.get_or_insert(now);
                state.last_wheel = Some(now);
                state.wheel_events += 1;
            }
            _ => {}
        }
    }
}

fn writer(path: &Path) -> Result<SyncSender<Value>, String> {
    use std::io::Write;
    let file = std::fs::OpenOptions::new().create_new(true).write(true).open(path)
        .map_err(|error| format!("No se pudo crear el perfil de scroll: {error}"))?;
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Value>(128);
    std::thread::spawn(move || {
        let mut file = std::io::BufWriter::new(file);
        let mut bytes = 0usize;
        let mut buffered = 0usize;
        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(value) => {
                    let line = value.to_string() + "\n";
                    if bytes + line.len() > 4 * 1024 * 1024 { break; }
                    if file.write_all(line.as_bytes()).is_err() { break; }
                    bytes += line.len();
                    buffered += 1;
                    if buffered >= 30 { if file.flush().is_err() { break; } buffered = 0; }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if file.flush().is_err() { break; } buffered = 0;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = file.flush();
    });
    Ok(sender)
}

pub fn configure(ui: &AppWindow, data: &Path) -> Handle {
    let handle = Handle { enabled: Rc::new(Cell::new(false)), state: Rc::new(RefCell::new(State::default())) };
    let render = handle.clone();
    let weak = ui.as_weak();
    let supported = ui.window().set_rendering_notifier(move |phase, api| {
        if !render.enabled() { return; }
        let now = Instant::now();
        let mut state = render.state.borrow_mut();
        if matches!(phase, slint::RenderingState::BeforeRendering) {
            state.before = Some(now);
        } else if matches!(phase, slint::RenderingState::AfterRendering) {
            // Capture consecutive frames during a wheel burst, not idle/background activity.
            let active = state.last_wheel.is_some_and(|last| last.elapsed().as_millis() < 250);
            if !active { state.previous_frame = None; state.rows = [0; 4]; return; }
            let Some(before) = state.before.take() else { return; };
            let input = state.pending.take();
            let interval = state.previous_frame.map(|last| now.duration_since(last).as_secs_f64() * 1000.0);
            state.previous_frame = Some(now);
            state.sequence += 1;
            let Some(ui) = weak.upgrade() else { return; };
            let record = json!({
                "event":"scroll_frame", "pane":state.pane, "sequence":state.sequence,
                "elapsed_ms":state.started.map(|start| start.elapsed().as_millis()),
                "wheel_to_before_render_ms":input.map(|start| before.saturating_duration_since(start).as_secs_f64()*1000.0),
                "wheel_to_render_complete_ms":input.map(|start| now.saturating_duration_since(start).as_secs_f64()*1000.0),
                "render_cpu_ms":now.duration_since(before).as_secs_f64()*1000.0,
                "frame_interval_ms":interval, "wheel_events":state.wheel_events,
                "rows_created":{"file":state.rows[0],"git_changes":state.rows[1],"git_history":state.rows[2],"chat":state.rows[3]},
                "file_lines":ui.get_file_preview_lines().row_count(),
                "git_rows":ui.get_git_files().row_count(), "git_commits":ui.get_git_history().row_count(),
                "chat_messages":ui.get_messages().row_count(),
                "file_view_open":ui.get_file_preview_open(), "git_panel_open":ui.get_git_open(),
                "window_width_px":ui.window().size().width, "window_height_px":ui.window().size().height,
                "renderer":if matches!(api, slint::GraphicsAPI::NativeOpenGL { .. }) { "opengl" } else { "other" },
                "dropped_records":state.dropped,
            });
            state.wheel_events = 0;
            state.rows = [0; 4];
            if let Some(sender) = &state.sender {
                if let Err(error) = sender.try_send(record) {
                    match error {
                        TrySendError::Full(_) => state.dropped += 1,
                                                TrySendError::Disconnected(_) => {
                            render.enabled.set(false);
                            ui.set_scroll_profiling(false);
                            ui.set_scroll_profile_status("La captura finalizó por el límite de tamaño o un error de escritura. Los datos disponibles quedan en logs/performance.".into());
                        }
                    }
                }
            }
        }
    }).is_ok();
    let rows = handle.clone();
    ui.on_profile_row_created(move |pane| {
        if !rows.enabled() { return; }
        let index = match pane.as_str() { "file" => 0, "git_changes" => 1, "git_history" => 2, "chat" => 3, _ => return };
        rows.state.borrow_mut().rows[index] += 1;
    });
    let directory = data.join("logs").join("performance");
    let toggle = handle.clone();
    let weak = ui.as_weak();
    ui.on_toggle_scroll_profile(move || {
        let Some(ui) = weak.upgrade() else { return; };
        if toggle.enabled() {
            toggle.enabled.set(false);
            toggle.state.borrow_mut().sender.take();
            ui.set_scroll_profiling(false);
            ui.set_scroll_profile_status(ui.get_scroll_profile_status().to_string().replace("Medición activa.", "Medición detenida.").into());
            return;
        }
        if !supported {
            ui.set_scroll_profile_status("El renderizador actual no admite la medición interna.".into()); return;
        }
        let path = directory.join(format!("scroll-{}.jsonl", uuid::Uuid::new_v4().simple()));
        let result = std::fs::create_dir_all(&directory).map_err(|e| e.to_string()).and_then(|_| writer(&path));
        match result {
            Ok(sender) => {
                let refresh = ui.window().with_winit_window(|window| window.current_monitor().and_then(|m| m.refresh_rate_millihertz()));
                let _ = sender.try_send(json!({"event":"scroll_profile_started","debug_build":cfg!(debug_assertions),"opt_level":option_env!("HARNESS_BUILD_OPT_LEVEL").unwrap_or("unknown"),"build_profile":option_env!("HARNESS_BUILD_PROFILE").unwrap_or("unknown"),"refresh_millihertz":refresh.flatten(),"note":"CPU submission timings, not GPU presentation timings; intervals reflect input cadence too"}));
                *toggle.state.borrow_mut() = State { sender: Some(sender), started: Some(Instant::now()), ..Default::default() };
                toggle.enabled.set(true);
                ui.set_scroll_profiling(true);
                ui.set_scroll_profile_status(format!("Medición activa. Archivo: {}", path.display()).into());
                ui.set_settings_open(false);
            }
            Err(error) => ui.set_scroll_profile_status(error.into()),
        }
    });
    handle
}
