use crate::domain::Provider;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub enabled: bool,
    pub executable: String,
    /// Empty means the official client's default. Not an entitlement claim.
    pub model: String,
    pub models: Vec<crate::codex::ModelChoice>,
    /// Empty preserves the model default without sending reasoning.effort.
    pub reasoning_effort: String,
    pub concurrency: u8,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: String::new(),
            model: String::new(),
            models: Vec::new(),
            reasoning_effort: String::new(),
            concurrency: 1,
        }
    }
}

pub const REASONING_EFFORTS: &[&str] = &[
    "", "none", "minimal", "low", "medium", "high", "xhigh", "max",
];

pub fn codex_efforts(model: &str) -> &'static [&'static str] {
    if model.starts_with("gpt-6.1-sol") || model.starts_with("gpt-6-astra") {
        &["", "low", "medium", "high", "xhigh", "max"]
    } else if model.starts_with("gpt-6-sol") || model.starts_with("gpt-6-luna") {
        &["", "none", "low", "medium", "high", "xhigh", "max"]
    } else {
        REASONING_EFFORTS
    }
}

/// Migrate saved legacy efforts before sending the first request; never retry it.
pub fn codex_effort<'a>(model: &str, effort: &'a str) -> &'a str {
    if !codex_efforts(model).contains(&effort) && matches!(effort, "minimal" | "none") {
        "low"
    } else {
        effort
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub codex: AgentSettings,
    pub claude: AgentSettings,
}

impl Settings {
    pub fn agent(&self, provider: Provider) -> &AgentSettings {
        match provider {
            Provider::Codex => &self.codex,
            Provider::Claude => &self.claude,
        }
    }
    pub fn agent_mut(&mut self, provider: Provider) -> &mut AgentSettings {
        match provider {
            Provider::Codex => &mut self.codex,
            Provider::Claude => &mut self.claude,
        }
    }
}

pub fn detect_client(provider: Provider) -> Option<PathBuf> {
    let name = match provider {
        Provider::Codex => "codex",
        Provider::Claude => "claude",
    };
    let mut directories = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&paths));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        directories.push(PathBuf::from(appdata).join("npm"));
    }
    for directory in directories {
        #[cfg(windows)]
        if provider == Provider::Claude {
            let native = directory.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
            if native.is_file() {
                return Some(native);
            }
        }
        #[cfg(windows)]
        let candidates = [
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.ps1"),
        ];
        #[cfg(not(windows))]
        let candidates = [name.to_string()];
        for candidate in candidates {
            let path = directory.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn validate_settings(config: &AgentSettings) -> Result<(), String> {
    if !(1..=4).contains(&config.concurrency) {
        return Err("La concurrencia debe estar entre 1 y 4.".into());
    }
    if config.enabled || !config.executable.is_empty() {
        let path = Path::new(&config.executable);
        if !path.is_absolute() || !path.is_file() {
            return Err("Seleccioná la ruta absoluta de un cliente instalado.".into());
        }
        #[cfg(windows)]
        if !matches!(
            path.extension()
                .and_then(OsStr::to_str)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("exe" | "cmd" | "ps1")
        ) {
            return Err("El cliente debe ser un archivo .exe, .cmd o .ps1.".into());
        }
    }
    Ok(())
}

pub(crate) fn official_command(executable: &Path) -> Command {
    #[cfg(windows)]
    let mut command = if executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
    {
        // Pass a script path as a distinct argument; never interpolate shell code.
        let shell = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(shell);
        command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(executable);
        command
    } else {
        Command::new(executable)
    };
    #[cfg(not(windows))]
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    // Subscription mode never injects API credentials or silently selects API billing.
    for key in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_SIMPLE",
        "CLAUDE_CODE_SKIP_PROMPT_HISTORY",
    ] {
        command.env_remove(key);
    }
    command.stdin(Stdio::null());
    command
}

pub(crate) fn run_official(
    executable: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<(bool, String), String> {
    let mut stdout =
        tempfile::tempfile().map_err(|_| "No se pudo crear la captura temporal.".to_string())?;
    let mut stderr =
        tempfile::tempfile().map_err(|_| "No se pudo crear la captura temporal.".to_string())?;
    let mut command = official_command(executable);
    command
        .args(arguments)
        .stdout(Stdio::from(
            stdout
                .try_clone()
                .map_err(|_| "No se pudo capturar la salida.".to_string())?,
        ))
        .stderr(Stdio::from(
            stderr
                .try_clone()
                .map_err(|_| "No se pudo capturar la salida.".to_string())?,
        ));
    let mut child = command
        .spawn()
        .map_err(|_| "No se pudo iniciar el cliente. Revisá su instalación y ruta.".to_string())?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(100)),
            _ => {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    let _ = Command::new("taskkill.exe")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .creation_flags(0x08000000)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("El cliente no respondió dentro del tiempo permitido.".into());
            }
        }
    };
    let mut output = String::new();
    for file in [&mut stdout, &mut stderr] {
        file.seek(SeekFrom::Start(0))
            .map_err(|_| "No se pudo leer el resultado.".to_string())?;
        let mut bytes = Vec::new();
        file.take(32 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|_| "No se pudo leer el resultado.".to_string())?;
        output.push_str(&String::from_utf8_lossy(&bytes));
        output.push('\n');
    }
    // Raw account output is transient; only sanitized classifications reach the UI.
    Ok((status.success(), output))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    Subscription,
    Api,
    SignedOut,
    Unknown,
}

pub fn classify_auth(provider: Provider, succeeded: bool, output: &str) -> AuthStatus {
    match provider {
        Provider::Codex => {
            let output = output.to_ascii_lowercase();
            if succeeded && output.contains("logged in using chatgpt") {
                AuthStatus::Subscription
            } else if output.contains("api key") {
                AuthStatus::Api
            } else if output.contains("not logged in") {
                AuthStatus::SignedOut
            } else {
                AuthStatus::Unknown
            }
        }
        Provider::Claude => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(output.trim()) else {
                return AuthStatus::Unknown;
            };
            match (
                value.get("loggedIn").and_then(|v| v.as_bool()),
                value.get("authMethod").and_then(|v| v.as_str()),
            ) {
                (Some(false), _) => AuthStatus::SignedOut,
                (Some(true), Some("claude.ai")) if succeeded => AuthStatus::Subscription,
                (_, Some("api_key" | "api_key_helper")) => AuthStatus::Api,
                _ => AuthStatus::Unknown,
            }
        }
    }
}

pub fn check_client(provider: Provider, config: &AgentSettings) -> Result<String, String> {
    validate_settings(config)?;
    let executable = Path::new(&config.executable);
    let (ok, output) = run_official(executable, &["--version"], Duration::from_secs(15))?;
    if !ok {
        return Err("El cliente no pudo informar su versión.".into());
    }
    let expected = match provider {
        Provider::Codex => "codex",
        Provider::Claude => "Claude Code",
    };
    if !output
        .to_ascii_lowercase()
        .contains(&expected.to_ascii_lowercase())
    {
        return Err("El archivo seleccionado no corresponde al proveedor elegido.".into());
    }
    let version = output
        .lines()
        .find(|line| {
            line.to_ascii_lowercase()
                .contains(&expected.to_ascii_lowercase())
        })
        .unwrap_or("Versión detectada")
        .chars()
        .take(100)
        .collect::<String>();
    let args: &[&str] = match provider {
        Provider::Codex => &["login", "status"],
        Provider::Claude => &["auth", "status"],
    };
    let (ok, output) = run_official(executable, args, Duration::from_secs(15))?;
    let auth = match classify_auth(provider, ok, &output) {
        AuthStatus::Subscription => {
            "Autenticado con cuenta de suscripción; acceso a modelos aún no verificado."
        }
        AuthStatus::Api => {
            "Autenticación por API detectada. No se habilitará ejecución en modo suscripción."
        }
        AuthStatus::SignedOut => "Sin sesión iniciada. Usá «Iniciar sesión».",
        AuthStatus::Unknown => "El cliente no confirmó autenticación por suscripción.",
    };
    Ok(format!("{version}\n{auth}"))
}

pub fn login_client(provider: Provider, config: &AgentSettings) -> Result<String, String> {
    // Validate the client identity before allowing it to open an authentication flow.
    check_client(provider, config)?;
    let args: &[&str] = match provider {
        Provider::Codex => &["login"],
        Provider::Claude => &["auth", "login"],
    };
    let (ok, _) = run_official(
        Path::new(&config.executable),
        args,
        Duration::from_secs(180),
    )?;
    if !ok {
        return Err("El inicio de sesión no se completó. Podés hacerlo desde el cliente oficial y verificar nuevamente.".into());
    }
    check_client(provider, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_state_without_settings_remains_readable() {
        let state: crate::domain::AppState = serde_json::from_str(
            r#"{"projects":[],"selected_chat":null,"open_tabs":[],"next_id":0}"#,
        )
        .unwrap();
        assert!(!state.settings.codex.enabled);
        assert!(!state.settings.claude.enabled);
        assert_eq!(state.settings.codex.concurrency, 1);
    }
    #[test]
    fn unknown_or_api_auth_is_never_classified_as_subscription() {
        assert_eq!(
            classify_auth(
                Provider::Claude,
                true,
                r#"{"loggedIn":true,"authMethod":"api_key"}"#
            ),
            AuthStatus::Api
        );
        assert_eq!(
            classify_auth(
                Provider::Claude,
                true,
                r#"{"loggedIn":true,"authMethod":"oauth_token"}"#
            ),
            AuthStatus::Unknown
        );
        assert_eq!(
            classify_auth(
                Provider::Claude,
                false,
                r#"{"loggedIn":false,"authMethod":"none"}"#
            ),
            AuthStatus::SignedOut
        );
        assert_eq!(
            classify_auth(Provider::Codex, true, "Logged in using ChatGPT"),
            AuthStatus::Subscription
        );
        assert_eq!(
            classify_auth(Provider::Codex, false, "Logged in using ChatGPT"),
            AuthStatus::Unknown
        );
        assert_eq!(
            classify_auth(
                Provider::Claude,
                true,
                r#"{"loggedIn":true,"authMethod":"claude.ai","email":"private@example.com"}"#
            ),
            AuthStatus::Subscription
        );
    }
}
