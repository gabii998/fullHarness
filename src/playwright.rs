use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::watch,
};

pub struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    #[cfg(windows)]
    _job: super::ProcessJob,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub fn validate(a: &Value) -> Result<(), String> {
    let action = super::field(a, "action")?;
    let target = super::field(a, "target")?;
    let value = super::field(a, "value")?;
    if target.len() > 1024 || value.len() > 8192 {
        return Err("Argumentos del navegador demasiado grandes.".into());
    }
    match action {
        "navigate" if value.starts_with("http://") || value.starts_with("https://") => {}
        "click" | "fill" | "press" | "assert_visible" if !target.is_empty() => {}
        "snapshot" | "screenshot" | "close" | "assert_text" | "assert_url" => {}
        _ => return Err("Acción inválida, URL no HTTP/HTTPS o selector vacío.".into()),
    }
    Ok(())
}

impl Session {
    fn start(root: &Path) -> Result<Self, String> {
        let mut command = tokio::process::Command::new("node");
        command
            .args(["-e", include_str!("playwright_worker.cjs")])
            .current_dir(root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|_| "No se pudo iniciar Node.js para Playwright.")?;
        #[cfg(windows)]
        let job = super::ProcessJob::attach(child.id().ok_or("Proceso sin PID.")?)?;
        Ok(Self {
            input: child.stdin.take().ok_or("Sin stdin.")?,
            output: BufReader::new(child.stdout.take().ok_or("Sin stdout.")?),
            child,
            #[cfg(windows)]
            _job: job,
        })
    }
    async fn request(&mut self, a: &Value) -> Result<Value, String> {
        let mut line = serde_json::to_vec(a).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.input
            .write_all(&line)
            .await
            .map_err(|_| "Worker Playwright desconectado.")?;
        self.input.flush().await.map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        (&mut self.output)
            .take(32769)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > 32768 || bytes.last() != Some(&b'\n') {
            return Err("Respuesta Playwright inválida o demasiado grande.".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "Respuesta Playwright inválida.".into())
    }
}

pub async fn execute(
    slot: &tokio::sync::Mutex<Option<Session>>,
    root: &Path,
    a: &Value,
    mut cancel: watch::Receiver<bool>,
) -> Result<Value, String> {
    validate(a)?;
    let mut state = tokio::select! {
        _ = crate::codex::cancelled(&mut cancel) => return Err("Navegador cancelado.".into()),
        state = slot.lock() => state,
    };
    if a["action"] == "close" && state.is_none() {
        return Ok(json!({"ok":true}));
    }
    if state.is_none() {
        *state = Some(Session::start(root)?);
    }
    let result = tokio::select! {
        _ = crate::codex::cancelled(&mut cancel) => Err("Navegador cancelado.".into()),
        result = tokio::time::timeout(Duration::from_secs(30), state.as_mut().unwrap().request(a)) => result.unwrap_or_else(|_| Err("Timeout del navegador.".into())),
    };
    if result.is_err() || a["action"] == "close" {
        *state = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_actions_and_limits() {
        for url in [
            "file:///secret",
            "javascript:alert(1)",
            "data:text/html,x",
            "",
        ] {
            assert!(validate(&json!({"action":"navigate","target":"","value":url})).is_err());
        }
        assert!(
            validate(&json!({"action":"navigate","target":"","value":"http://localhost:3000"}))
                .is_ok()
        );
        assert!(validate(&json!({"action":"evaluate","target":"","value":"x"})).is_err());
        assert!(validate(&json!({"action":"click","target":"","value":""})).is_err());
        assert!(
            validate(&json!({"action":"fill","target":"@e1","value":"x".repeat(8193)})).is_err()
        );
    }
}
