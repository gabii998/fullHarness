//! Bounded local Docker operations. Ownership is checked again before each action.
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub health: String,
    pub service: String,
    pub project: String,
    pub directory: String,
    pub ports: serde_json::Value,
}
#[derive(Debug)]
pub struct Snapshot {
    pub containers: Vec<Container>,
    pub compose: Option<PathBuf>,
}
#[derive(Clone)]
pub struct Client {
    binary: PathBuf,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            binary: "docker".into(),
        }
    }
}
impl Client {
    pub fn with_binary(binary: PathBuf) -> Self {
        Self { binary }
    }
    async fn command(&self, root: &Path, args: &[&str], seconds: u64) -> Result<String, String> {
        let mut cmd = tokio::process::Command::new(&self.binary);
        cmd.args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Compose must use this project's file, regardless of inherited shell settings.
        for key in [
            "COMPOSE_FILE",
            "COMPOSE_PROJECT_NAME",
            "COMPOSE_PROFILES",
            "COMPOSE_ENV_FILES",
        ] {
            cmd.env_remove(key);
        }
        cmd.env("NO_COLOR", "1");
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn().map_err(|_| {
            "No se pudo ejecutar Docker. Instalá Docker y verificá que esté en PATH.".to_string()
        })?;
        let out = child.stdout.take().ok_or("Docker no abrió stdout.")?;
        let err = child.stderr.take().ok_or("Docker no abrió stderr.")?;
        let result = tokio::time::timeout(Duration::from_secs(seconds), async {
            let (out, err) = tokio::try_join!(bounded(out), bounded(err))?;
            let status = child.wait().await.map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(if err.is_empty() {
                    format!("Docker falló: {out}")
                } else {
                    err
                });
            }
            Ok(if args.first() == Some(&"logs") && !err.is_empty() {
                format!("{out}\n{err}")
            } else {
                out
            })
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => Err(format!(
                "Docker superó {seconds} segundos. Actualizá el estado antes de reintentar."
            )),
        }
    }
    async fn inspect(&self, root: &Path, ids: &[&str]) -> Result<Vec<Container>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        for id in ids {
            validate_id(id)?;
        }
        // Do not request environment variables or mount contents in container metadata.
        let template = r#"{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Config.Image}},"state":{{json .State.Status}},"health":{{if .State.Health}}{{json .State.Health.Status}}{{else}}""{{end}},"project":{{json (index .Config.Labels "com.docker.compose.project")}},"service":{{json (index .Config.Labels "com.docker.compose.service")}},"directory":{{json (index .Config.Labels "com.docker.compose.project.working_dir")}},"ports":{{json .NetworkSettings.Ports}}}"#;
        let mut args = vec!["container", "inspect", "--format", template];
        args.extend_from_slice(ids);
        self.command(root, &args, 15)
            .await?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|e| format!("Metadatos Docker inválidos: {e}"))
            })
            .collect()
    }
    pub async fn snapshot(&self, root: &Path) -> Result<Snapshot, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let output = self
            .command(
                &root,
                &[
                    "ps",
                    "-aq",
                    "--no-trunc",
                    "--filter",
                    "label=com.docker.compose.project.working_dir",
                ],
                15,
            )
            .await?;
        let ids: Vec<_> = output.lines().filter(|s| !s.is_empty()).collect();
        if ids.len() > 200 {
            return Err("Hay más de 200 contenedores Compose. Esta vista limita la consulta para conservar recursos.".into());
        }
        let mut containers: Vec<_> = self
            .inspect(&root, &ids)
            .await?
            .into_iter()
            .filter(|c| owned(&root, c))
            .collect();
        containers.sort_by(|a, b| (&a.service, &a.name).cmp(&(&b.service, &b.name)));
        Ok(Snapshot {
            containers,
            compose: compose_file(&root),
        })
    }
    pub async fn action(&self, root: &Path, id: &str, action: &str) -> Result<String, String> {
        validate_id(id)?;
        if !matches!(action, "start" | "stop" | "restart" | "logs") {
            return Err("Operación Docker desconocida.".into());
        }
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let containers = self.inspect(&root, &[id]).await?;
        let c = containers
            .first()
            .filter(|c| c.id == id && owned(&root, c))
            .ok_or("El contenedor no pertenece al proyecto actual.")?;
        if action == "logs" {
            return self
                .command(&root, &["logs", "--tail", "200", "--timestamps", &c.id], 15)
                .await;
        }
        let args = if action == "start" {
            vec!["container", action, &c.id]
        } else {
            vec!["container", action, "--time", "10", &c.id]
        };
        self.command(&root, &args, 45).await?;
        Ok(format!(
            "{}: {}",
            c.name.trim_start_matches('/'),
            match action {
                "start" => "iniciado",
                "stop" => "detenido",
                _ => "reiniciado",
            }
        ))
    }
    pub async fn up(&self, root: &Path) -> Result<String, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let file = compose_file(&root).ok_or(
            "No hay compose.yaml, compose.yml o docker-compose.yml en la raíz del proyecto.",
        )?;
        let base = cli_path(&root);
        let mut args = vec![
            "compose".to_string(),
            "--project-directory".into(),
            base,
            "-f".into(),
            cli_path(&file),
        ];
        let legacy = file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("docker-compose"));
        let overrides = if legacy {
            [
                "docker-compose.override.yaml",
                "docker-compose.override.yml",
            ]
        } else {
            ["compose.override.yaml", "compose.override.yml"]
        };
        for name in overrides {
            if let Ok(path) = root.join(name).canonicalize() {
                if path.is_file() && path.starts_with(&root) {
                    args.extend(["-f".into(), cli_path(&path)]);
                    break;
                }
            }
        }
        args.extend(["up".into(), "-d".into()]);
        self.command(
            &root,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            180,
        )
        .await?;
        Ok("Servicios Compose iniciados.".into())
    }
}
async fn bounded(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut block = [0u8; 4096];
    loop {
        let n = stream.read(&mut block).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > 256 * 1024 {
            return Err(
                "La salida de Docker supera 256 KiB. Reducí los logs o consultá el CLI.".into(),
            );
        }
        bytes.extend_from_slice(&block[..n]);
    }
    // Exclude terminal control sequences from the native UI.
    let text = String::from_utf8_lossy(&bytes);
    let mut result = String::new();
    let mut escape = false;
    for ch in text.chars() {
        if escape {
            if ch.is_ascii_alphabetic() {
                escape = false;
            }
            continue;
        }
        if ch == '\u{1b}' {
            escape = true;
            continue;
        }
        if !ch.is_control() || matches!(ch, '\n' | '\t') {
            result.push(ch);
        }
    }
    Ok(result)
}
fn validate_id(id: &str) -> Result<(), String> {
    if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Identificador de contenedor inválido.".into())
    }
}
fn normalize(path: &str) -> String {
    let path = path
        .strip_prefix("\\\\?\\")
        .unwrap_or(path)
        .replace('\\', "/");
    let path = path.trim_end_matches('/');
    #[cfg(windows)]
    let path = path.to_lowercase();
    path.to_string()
}
fn cli_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        path.strip_prefix("\\\\?\\").unwrap_or(&path).to_string()
    }
}
fn owned(root: &Path, container: &Container) -> bool {
    if container.project.is_empty()
        || container.service.is_empty()
        || container.directory.is_empty()
    {
        return false;
    }
    let directory = Path::new(&container.directory)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| container.directory.clone());
    normalize(&root.to_string_lossy()) == normalize(&directory)
}
fn compose_file(root: &Path) -> Option<PathBuf> {
    [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]
    .iter()
    .find_map(|name| {
        let path = root.join(name).canonicalize().ok()?;
        (path.is_file() && path.starts_with(root)).then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_requires_exact_directory_and_compose_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut c: Container = serde_json::from_value(serde_json::json!({"id":"a".repeat(64),"name":"/db","image":"postgres","state":"running","health":"healthy","service":"db","project":"same-name","directory":root.to_string_lossy(),"ports":null})).unwrap();
        assert!(owned(&root, &c));
        c.directory.push_str("-other");
        assert!(!owned(&root, &c));
        c.directory = root.to_string_lossy().into();
        c.project.clear();
        assert!(!owned(&root, &c));
        assert!(validate_id("--help").is_err());
        assert!(validate_id(&"a".repeat(64)).is_ok());
        std::fs::write(root.join("compose.yaml"), "services: {}").unwrap();
        assert!(compose_file(&root).is_some());
    }
    #[tokio::test]
    async fn output_is_bounded_and_terminal_codes_removed() {
        assert_eq!(
            bounded(&b"\x1b[31mhello\x1b[0m\n"[..]).await.unwrap(),
            "hello\n"
        );
        assert!(bounded(&vec![b'x'; 262145][..]).await.is_err());
    }
}
