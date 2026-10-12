//! Bounded, live ADB output for the UI. Never inserted into agent context.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, watch},
};

pub const MAX_LINES: usize = 1000;
pub const MAX_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub serial: String,
    pub label: String,
    pub available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub text: String,
    pub level: char,
}
pub enum StreamEvent {
    Lines(Vec<Line>),
    End(Result<(), String>),
}

pub fn detect_adb() -> Option<PathBuf> {
    let name = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut candidates = Vec::new();
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(root) = std::env::var_os(key) {
            candidates.push(PathBuf::from(root).join("platform-tools").join(name));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(root)
                .join("Android/SDK/platform-tools")
                .join(name),
        );
    }
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|p| p.join(name)));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn command(adb: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(adb);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            if serial == "List"
                || serial.starts_with('*')
                || !["device", "offline", "unauthorized", "no"].contains(&state)
            {
                return None;
            }
            let model = parts
                .find_map(|p| p.strip_prefix("model:"))
                .unwrap_or(serial)
                .replace('_', " ");
            Some(Device {
                serial: serial.into(),
                label: format!("{model} · {serial} · {state}"),
                available: state == "device",
            })
        })
        .collect()
}

pub async fn devices(adb: &Path) -> Result<Vec<Device>, String> {
    let mut child = command(adb)
        .args(["devices", "-l"])
        .spawn()
        .map_err(|e| format!("No se pudo ejecutar ADB: {e}"))?;
    let out = child.stdout.take().ok_or("ADB no abrió stdout")?;
    let err = child.stderr.take().ok_or("ADB no abrió stderr")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut bytes = Vec::new();
        let mut errors = Vec::new();
        let mut out = out.take(64 * 1024 + 1);
        let mut err = err.take(4096);
        tokio::try_join!(out.read_to_end(&mut bytes), err.read_to_end(&mut errors))
            .map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 {
            return Err("La lista de dispositivos excede 64 KiB".into());
        }
        if !child.wait().await.map_err(|e| e.to_string())?.success() {
            return Err(format!("ADB: {}", String::from_utf8_lossy(&errors).trim()));
        }
        Ok(parse_devices(&String::from_utf8_lossy(&bytes)))
    })
    .await
    .map_err(|_| "ADB no respondió en 10 segundos".to_string())?
}

#[derive(Default)]
struct Framer {
    bytes: Vec<u8>,
    truncated: bool,
    level: Option<char>,
}
impl Framer {
    fn feed(&mut self, bytes: &[u8], lines: &mut Vec<Line>) {
        for byte in bytes {
            if *byte == b'\n' {
                self.finish(lines);
            } else if self.bytes.len() < MAX_LINE_BYTES {
                self.bytes.push(*byte);
            } else {
                self.truncated = true;
            }
        }
    }
    fn finish(&mut self, lines: &mut Vec<Line>) {
        if self.bytes.is_empty() && !self.truncated {
            return;
        }
        let mut text = String::from_utf8_lossy(&self.bytes)
            .trim_end_matches('\r')
            .to_string();
        if self.truncated {
            text.push_str(" … [línea recortada]");
        }
        // Brief format includes priority/tag/pid. Stacktrace continuations inherit
        // their parent's priority so an Error filter retains the useful frames.
        if text.as_bytes().get(1) == Some(&b'/') {
            if let Some(level) = text.chars().next().filter(|c| "VDIWEF".contains(*c)) {
                self.level = Some(level);
            }
        } else if text.starts_with("---------") {
            self.level = None;
        }
        lines.push(Line {
            text,
            level: self.level.unwrap_or('I'),
        });
        self.bytes.clear();
        self.truncated = false;
    }
}

async fn deliver(
    tx: &mpsc::Sender<StreamEvent>,
    stop: &mut watch::Receiver<bool>,
    lines: &mut Vec<Line>,
) -> bool {
    if lines.is_empty() {
        return true;
    }
    let event = StreamEvent::Lines(std::mem::take(lines));
    tokio::select! {
        _ = stop.changed() => false,
        result = tx.send(event) => result.is_ok(),
    }
}

pub async fn stream(
    adb: PathBuf,
    serial: String,
    mut stop: watch::Receiver<bool>,
    tx: mpsc::Sender<StreamEvent>,
) {
    let result = run(adb, serial, &mut stop, &tx).await;
    if !*stop.borrow() {
        let _ = tx.send(StreamEvent::End(result)).await;
    }
}
async fn run(
    adb: PathBuf,
    serial: String,
    stop: &mut watch::Receiver<bool>,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<(), String> {
    if *stop.borrow() {
        return Ok(());
    }
    let mut child = command(&adb)
        .args(["-s", &serial, "logcat", "-v", "brief", "-T", "1", "*:V"])
        .spawn()
        .map_err(|e| format!("No se pudo iniciar Logcat: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("ADB no abrió stdout")?;
    let stderr = child.stderr.take().ok_or("ADB no abrió stderr")?;
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.take(4096).read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).trim().to_string()
    });
    let mut framer = Framer::default();
    let mut lines = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut flush = tokio::time::interval(Duration::from_millis(100));
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            _ = stop.changed() => break Ok(()),
            _ = flush.tick() => { if !deliver(tx, stop, &mut lines).await { break Ok(()); } }
            read = stdout.read(&mut buffer) => match read {
                Ok(0) => { framer.finish(&mut lines); let _ = deliver(tx, stop, &mut lines).await; break Ok(()); }
                Ok(count) => {
                    framer.feed(&buffer[..count], &mut lines);
                    if lines.len() >= 128 && !deliver(tx, stop, &mut lines).await { break Ok(()); }
                }
                Err(error) => break Err(format!("No se pudo leer Logcat: {error}")),
            }
        }
    };
    if *stop.borrow() || tx.is_closed() || result.is_err() {
        let _ = child.kill().await;
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let detail = errors.await.unwrap_or_default();
    result?;
    if !*stop.borrow() && !status.success() {
        return Err(format!("Logcat se desconectó: {detail}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn devices_and_fragmented_bounded_lines_preserve_errors_and_unicode() {
        let list = parse_devices(
            "List of devices attached\nemulator-5554 device product:sdk model:Pixel_8\nusb-1 unauthorized\nusb-2 offline\n",
        );
        assert_eq!(list.len(), 3);
        assert!(list[0].available);
        assert!(!list[1].available);
        let mut f = Framer::default();
        let mut lines = Vec::new();
        let text = "E/App( 1): fallo α\r\n    at Main.run()\nI/App( 1): listo\n";
        for chunk in text.as_bytes().chunks(1) {
            f.feed(chunk, &mut lines);
        }
        assert!(lines[0].text.ends_with('α'));
        assert_eq!(lines[1].level, 'E');
        assert_eq!(lines[2].level, 'I');
        f.feed(&vec![b'x'; 100_000], &mut lines);
        assert!(f.bytes.len() <= MAX_LINE_BYTES);
        f.feed(b"\n", &mut lines);
        assert!(lines.last().unwrap().text.ends_with("[línea recortada]"));
    }
}
