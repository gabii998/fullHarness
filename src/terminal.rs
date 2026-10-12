//! Native PTY sessions. Bounded terminal state stays separate from agent context.
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Replies(std::sync::mpsc::SyncSender<Vec<u8>>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if i1.is_none() && i2.is_none() && c == 'n' {
            let value = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
            let reply = match value {
                6 => {
                    let (r, c) = screen.cursor_position();
                    format!("\x1b[{};{}R", r + 1, c + 1)
                }
                5 => "\x1b[0n".into(),
                _ => return,
            };
            let _ = self.0.try_send(reply.into_bytes());
        }
    }
}
pub struct Session {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    pub parser: Mutex<vt100::Parser<Replies>>,
    pub shell: String,
    pub dirty: AtomicBool,
    pub ended: AtomicBool,
    #[cfg(windows)]
    job: Option<crate::tool_runtime::ProcessJob>,
}
impl Session {
    pub fn start(
        root: &Path,
        shell: &str,
        args: &[&str],
        rows: u16,
        cols: u16,
    ) -> Result<Arc<Self>, String> {
        let size = size(rows, cols);
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(shell);
        command.args(args);
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let job = match crate::tool_runtime::ProcessJob::attach(
            child.process_id().ok_or("Terminal sin PID.")?,
        ) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (input, queue) = std::sync::mpsc::sync_channel::<Vec<u8>>(128);
        std::thread::Builder::new()
            .name("terminal-writer".into())
            .spawn(move || {
                while let Ok(bytes) = queue.recv() {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let parser =
            vt100::Parser::new_with_callbacks(size.rows, size.cols, 500, Replies(input.clone()));
        let session = Arc::new(Self {
            master: Mutex::new(pair.master),
            input,
            child: Mutex::new(child),
            parser: Mutex::new(parser),
            shell: shell.into(),
            dirty: AtomicBool::new(true),
            ended: AtomicBool::new(false),
            #[cfg(windows)]
            job: Some(job),
        });
        let weak = Arc::downgrade(&session);
        std::thread::Builder::new()
            .name("terminal-reader".into())
            .spawn(move || {
                let mut reader = reader;
                let mut bytes = [0u8; 8192];
                loop {
                    match reader.read(&mut bytes) {
                        Ok(0) | Err(_) => {
                            if let Some(session) = weak.upgrade() {
                                session.ended.store(true, Ordering::Relaxed);
                                session.dirty.store(true, Ordering::Relaxed);
                            }
                            break;
                        }
                        Ok(n) => {
                            let Some(session) = weak.upgrade() else {
                                break;
                            };
                            session.parser.lock().unwrap().process(&bytes[..n]);
                            session.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(session)
    }
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 64 * 1024 {
            return Err("El texto pegado supera 64 KiB.".into());
        }
        if self.ended.load(Ordering::Relaxed) {
            return Err("La terminal terminó. Abrí una nueva sesión.".into());
        }
        self.input
            .try_send(bytes.to_vec())
            .map_err(|_| "El buffer de entrada de la terminal está lleno o cerrado.".into())
    }
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        let size = size(rows, cols);
        if self
            .master
            .lock()
            .unwrap()
            .get_size()
            .map_err(|e| e.to_string())?
            == size
        {
            return Ok(());
        }
        self.master
            .lock()
            .unwrap()
            .resize(size)
            .map_err(|e| e.to_string())?;
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(size.rows, size.cols);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }
    pub fn stop(&self) {
        let _ = self.child.lock().unwrap().kill();
        self.ended.store(true, Ordering::Relaxed);
    }
    pub fn poll_exit(&self) {
        if self
            .child
            .lock()
            .unwrap()
            .try_wait()
            .ok()
            .flatten()
            .is_some()
            && !self.ended.swap(true, Ordering::Relaxed)
        {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
    pub fn scroll(&self, delta: i32) {
        let mut parser = self.parser.lock().unwrap();
        let screen = parser.screen_mut();
        let offset = (screen.scrollback() as i32 + delta).clamp(0, 500);
        screen.set_scrollback(offset as usize);
        self.dirty.store(true, Ordering::Relaxed);
    }
    pub fn text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.get_mut().unwrap().kill();
        #[cfg(windows)]
        drop(self.job.take());
    }
}
fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(4, 80),
        cols: cols.clamp(20, 240),
        pixel_width: 0,
        pixel_height: 0,
    }
}
pub fn color(color: vt100::Color, foreground: bool) -> (u8, u8, u8) {
    match color {
        vt100::Color::Default => {
            if foreground {
                (221, 227, 239)
            } else {
                (16, 24, 34)
            }
        }
        vt100::Color::Rgb(r, g, b) => (r, g, b),
        vt100::Color::Idx(i) => {
            const COLORS: [(u8, u8, u8); 16] = [
                (16, 24, 34),
                (213, 98, 109),
                (105, 188, 133),
                (220, 178, 104),
                (111, 145, 218),
                (168, 131, 219),
                (100, 180, 197),
                (205, 211, 224),
                (95, 106, 130),
                (240, 143, 154),
                (147, 217, 165),
                (239, 207, 151),
                (153, 183, 243),
                (199, 168, 246),
                (148, 217, 230),
                (239, 242, 250),
            ];
            if i < 16 {
                COLORS[i as usize]
            } else if i >= 232 {
                let c = 8 + (i - 232) * 10;
                (c, c, c)
            } else {
                let i = i - 16;
                let value = |n| if n == 0 { 0 } else { 55 + n * 40 };
                (value(i / 36), value(i % 36 / 6), value(i % 6))
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ansi_unicode_redraw_and_bounded_scrollback() {
        let mut parser = vt100::Parser::new(4, 30, 500);
        parser.process("hola á\r\n\u{1b}[31merror\u{1b}[0m".as_bytes());
        assert!(parser.screen().contents().contains("hola á"));
        assert_ne!(
            color(parser.screen().cell(1, 0).unwrap().fgcolor(), true),
            color(vt100::Color::Default, true)
        );
        parser.process(b"\r\x1b[2Kcorrected");
        assert!(parser.screen().contents().contains("corrected"));
        assert!(!parser.screen().contents().contains("error"));
        for _ in 0..1000 {
            parser.process(b"\r\nline");
        }
        parser.screen_mut().set_scrollback(1000);
        assert!(parser.screen().scrollback() <= 500);
    }
}
