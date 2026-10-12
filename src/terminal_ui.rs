use crate::{AppWindow, TerminalRow, TerminalRun};
use ascurra_harness::{
    runtime::Supervisor,
    terminal::{self, Session},
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::Ordering},
};
type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;
pub fn display(ui: &AppWindow, session: &Session) {
    ui.set_terminal_shell_index(if session.shell.contains("powershell") {
        1
    } else {
        0
    });
    let parser = session.parser.lock().unwrap();
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let mut lines = Vec::new();
    for row in 0..rows {
        let mut runs = Vec::<TerminalRun>::new();
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = terminal::color(cell.fgcolor(), true);
            let mut bg = terminal::color(cell.bgcolor(), false);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let fg = slint::Color::from_rgb_u8(fg.0, fg.1, fg.2);
            let bg = slint::Color::from_rgb_u8(bg.0, bg.1, bg.2);
            let text = if cell.contents().is_empty() {
                " "
            } else {
                cell.contents()
            };
            let width = if cell.is_wide() { 2 } else { 1 };
            if let Some(last) = runs.last_mut().filter(|r| {
                r.fg == fg
                    && r.bg == bg
                    && r.bold == cell.bold()
                    && r.column + r.columns == col as i32
            }) {
                last.text = format!("{}{text}", last.text).into();
                last.columns += width;
            } else {
                runs.push(TerminalRun {
                    text: text.into(),
                    column: col as i32,
                    columns: width,
                    fg,
                    bg,
                    bold: cell.bold(),
                });
            }
        }
        lines.push(TerminalRow {
            runs: ModelRc::new(VecModel::from(runs)),
        });
    }
    let current = ui.get_terminal_rows();
    if let Some(model) = current.as_any().downcast_ref::<VecModel<TerminalRow>>() {
        while model.row_count() > lines.len() {
            model.remove(model.row_count() - 1);
        }
        for (index, line) in lines.into_iter().enumerate() {
            if let Some(old) = model.row_data(index) {
                if !old.runs.iter().eq(line.runs.iter()) {
                    model.set_row_data(index, line);
                }
            } else {
                model.push(line);
            }
        }
    } else {
        ui.set_terminal_rows(ModelRc::new(VecModel::from(lines)));
    }
    let (row, col) = screen.cursor_position();
    ui.set_terminal_cursor_row(row as i32);
    ui.set_terminal_cursor_column(col as i32);
    ui.set_terminal_cursor_visible(
        !screen.hide_cursor() && screen.scrollback() == 0 && !session.ended.load(Ordering::Relaxed),
    );
    ui.set_terminal_running(!session.ended.load(Ordering::Relaxed));
    ui.set_terminal_status(if session.ended.load(Ordering::Relaxed) {
        "La sesión terminó. Podés cerrar e iniciar otra.".into()
    } else if screen.scrollback() > 0 {
        format!("Historial · {} líneas atrás", screen.scrollback()).into()
    } else {
        "Ctrl+C interrumpe · Ctrl+Shift+C copia pantalla · Ctrl+V pega".into()
    });
}
fn selected(ui: &AppWindow, map: &Sessions) -> Option<Arc<Session>> {
    map.lock()
        .unwrap()
        .get(ui.get_git_project().as_str())
        .cloned()
}
fn key(text: &str, control: bool, alt: bool) -> String {
    use slint::platform::Key;
    let keys = [
        (Key::UpArrow, "\x1b[A"),
        (Key::DownArrow, "\x1b[B"),
        (Key::RightArrow, "\x1b[C"),
        (Key::LeftArrow, "\x1b[D"),
        (Key::Home, "\x1b[H"),
        (Key::End, "\x1b[F"),
        (Key::Delete, "\x1b[3~"),
        (Key::PageUp, "\x1b[5~"),
        (Key::PageDown, "\x1b[6~"),
        (Key::Backspace, "\x7f"),
        (Key::Return, "\r"),
        (Key::Tab, "\t"),
        (Key::Escape, "\x1b"),
        (Key::Backtab, "\x1b[Z"),
        (Key::Insert, "\x1b[2~"),
        (Key::F1, "\x1bOP"),
        (Key::F2, "\x1bOQ"),
        (Key::F3, "\x1bOR"),
        (Key::F4, "\x1bOS"),
        (Key::F5, "\x1b[15~"),
        (Key::F6, "\x1b[17~"),
        (Key::F7, "\x1b[18~"),
        (Key::F8, "\x1b[19~"),
        (Key::F9, "\x1b[20~"),
        (Key::F10, "\x1b[21~"),
        (Key::F11, "\x1b[23~"),
        (Key::F12, "\x1b[24~"),
    ];
    if let Some((_, sequence)) = keys.iter().find(|(k, _)| {
        let value: slint::SharedString = (*k).into();
        value.as_str() == text
    }) {
        return sequence.to_string();
    }
    let modifiers = [
        Key::Shift,
        Key::ShiftR,
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
        Key::CapsLock,
    ];
    if modifiers.iter().any(|k| {
        let value: slint::SharedString = (*k).into();
        value.as_str() == text
    }) {
        return String::new();
    }
    let mut text = text.to_string();
    if control && text.len() == 1 {
        let b = text.as_bytes()[0];
        if b.is_ascii_alphabetic() {
            text = String::from_utf8(vec![b.to_ascii_uppercase() & 31]).unwrap();
        }
    }
    if alt { format!("\x1b{text}") } else { text }
}
pub fn configure(ui: &AppWindow, service: Arc<Supervisor>) {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let starting = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
    let weak = ui.as_weak();
    let map = sessions.clone();
    let pending = starting.clone();
    let executor = service.clone();
    ui.on_start_terminal(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let root = ui.get_git_project().to_string();
        if root.is_empty() {
            return;
        }
        if let Some(session) = selected(&ui, &map) {
            display(&ui, &session);
            return;
        }
        if !pending.lock().unwrap().insert(root.clone()) {
            return;
        }
        if map.lock().unwrap().len() + pending.lock().unwrap().len() > 8 {
            pending.lock().unwrap().remove(&root);
            ui.set_terminal_status("Cerrá alguna terminal: el límite es 8 sesiones.".into());
            return;
        }
        ui.set_terminal_busy(true);
        ui.set_terminal_status("Abriendo terminal…".into());
        let shell = ui.get_terminal_shell_index();
        let rows = ui.get_terminal_height_rows().clamp(4, 80) as u16;
        let cols = ui.get_terminal_width_columns().clamp(20, 240) as u16;
        let handle = ui.as_weak();
        let map = map.clone();
        let pending = pending.clone();
        executor.executor.spawn(async move {
            let path = root.clone();
            let result = tokio::task::spawn_blocking(move || {
                #[cfg(windows)]
                let (shell, args) = if shell == 1 {
                    ("powershell.exe", vec!["-NoLogo", "-NoProfile"])
                } else {
                    ("cmd.exe", vec!["/Q"])
                };
                #[cfg(not(windows))]
                let (shell, args) = ("bash", vec!["--noprofile", "--norc"]);
                Session::start(std::path::Path::new(&path), shell, &args, rows, cols)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            pending.lock().unwrap().remove(&root);
            match result {
                Ok(session) => {
                    map.lock().unwrap().insert(root.clone(), session.clone());
                    let _ = handle.upgrade_in_event_loop(move |ui| {
                        if ui.get_git_project().as_str() == root {
                            ui.set_terminal_busy(false);
                            display(&ui, &session);
                        }
                    });
                }
                Err(error) => {
                    let _ = handle.upgrade_in_event_loop(move |ui| {
                        if ui.get_git_project().as_str() == root {
                            ui.set_terminal_busy(false);
                            ui.set_terminal_status(error.into());
                        }
                    });
                }
            }
        });
    });
    let weak = ui.as_weak();
    let map = sessions.clone();
    let executor = service.clone();
    ui.on_close_terminal(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(session) = map.lock().unwrap().remove(ui.get_git_project().as_str()) {
                executor.executor.spawn_blocking(move || {
                    session.stop();
                    drop(session);
                });
            }
            ui.set_terminal_running(false);
            ui.set_terminal_cursor_visible(false);
            ui.set_terminal_rows(ModelRc::default());
            ui.set_terminal_status("Terminal cerrada.".into());
        }
    });
    let weak = ui.as_weak();
    let map = sessions.clone();
    ui.on_terminal_key(move |text, control, alt, shift| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(session) = selected(&ui, &map) else {
            return;
        };
        if control && shift && text.eq_ignore_ascii_case("c") {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(session.text());
            }
            return;
        }
        if control && text.eq_ignore_ascii_case("v") {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    let text = text.replace("\r\n", "\n");
                    let bracket = session.parser.lock().unwrap().screen().bracketed_paste();
                    let text = if bracket {
                        format!("\x1b[200~{text}\x1b[201~")
                    } else {
                        text.replace('\n', "\r")
                    };
                    if let Err(error) = session.write(text.as_bytes()) {
                        ui.set_terminal_status(error.into());
                    }
                }
            }
            return;
        }
        session.scroll(-500);
        let mut bytes = key(&text, control, alt);
        if session.parser.lock().unwrap().screen().application_cursor()
            && ["\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D"].contains(&bytes.as_str())
        {
            bytes = bytes.replace('[', "O");
        }
        if bytes.is_empty() {
            return;
        }
        if let Err(error) = session.write(bytes.as_bytes()) {
            ui.set_terminal_status(error.into());
        }
    });
    let weak = ui.as_weak();
    let map = sessions.clone();
    ui.on_terminal_scroll(move |delta| {
        if let Some(ui) = weak.upgrade() {
            if let Some(session) = selected(&ui, &map) {
                session.scroll(delta);
                display(&ui, &session);
            }
        }
    });
    let weak = ui.as_weak();
    let map = sessions.clone();
    let timer = std::rc::Rc::new(slint::Timer::default());
    let tick = move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_terminal_open() {
            return;
        }
        if let Some(session) = selected(&ui, &map) {
            session.poll_exit();
            let rows = ui.get_terminal_height_rows().clamp(4, 80) as u16;
            let cols = ui.get_terminal_width_columns().clamp(20, 240) as u16;
            if let Err(error) = session.resize(rows, cols) {
                ui.set_terminal_status(error.into());
            }
            if session.dirty.swap(false, Ordering::Relaxed) {
                display(&ui, &session);
            }
        }
    };
    let weak = ui.as_weak();
    let map = sessions.clone();
    let pending = starting.clone();
    let tick = std::rc::Rc::new(tick);
    ui.on_refresh_terminal(move || {
        if let Some(ui) = weak.upgrade() {
            timer.stop();
            if !ui.get_terminal_open() {
                return;
            }
            ui.set_terminal_busy(
                pending
                    .lock()
                    .unwrap()
                    .contains(ui.get_git_project().as_str()),
            );
            if let Some(session) = selected(&ui, &map) {
                display(&ui, &session);
            } else {
                ui.set_terminal_running(false);
                ui.set_terminal_rows(ModelRc::default());
                ui.set_terminal_cursor_visible(false);
                ui.set_terminal_status("Iniciá una terminal en este proyecto.".into());
            }
            let tick = tick.clone();
            timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(50),
                move || tick(),
            );
        }
    });
}
