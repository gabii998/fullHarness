//! Bounded Git operations for the desktop UI, outside agent context.
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LineChanges {
    Text { added: u64, removed: u64 },
    Binary,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitFile {
    pub path: String,
    pub target: String,
    pub original: Option<String>,
    pub staged: bool,
    pub unstaged: bool,
    pub status: String,
    pub index_code: String,
    pub worktree_code: String,
    pub staged_lines: Option<LineChanges>,
    pub unstaged_lines: Option<LineChanges>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub files: Vec<GitFile>,
}
#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub title: String,
    pub author: String,
    pub date: String,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub repository: bool,
    pub status: Option<GitStatus>,
    pub history: Vec<Commit>,
    pub remote: bool,
    pub origin_url: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub branches: Vec<String>,
    pub current_branch: Option<String>,
}
struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}

async fn command(path: &Path, args: &[&str], seconds: u64, limit: u64) -> Result<Output, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["--no-optional-locks", "--literal-pathspecs"])
        .args(args)
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| "No se pudo ejecutar Git. Revisá que esté instalado.".to_string())?;
    let stdout = child.stdout.take().ok_or("Git no abrió su salida.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Git no abrió su salida de errores.")?;
    let result = tokio::time::timeout(Duration::from_secs(seconds), async {
        let read = async {
            let mut data = Vec::new();
            stdout
                .take(limit + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|_| "No se pudo leer Git.")?;
            if data.len() as u64 > limit {
                return Err("La salida de Git supera el límite de esta vista.".to_string());
            }
            Ok(data)
        };
        // Keep draining stderr so hooks/transports cannot block on a full pipe.
        let errors = async {
            let mut stream = stderr;
            let mut retained = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream
                    .read(&mut chunk)
                    .await
                    .map_err(|_| "No se pudieron leer los errores de Git.")?;
                if n == 0 {
                    break;
                }
                let keep = n.min(4096usize.saturating_sub(retained.len()));
                retained.extend_from_slice(&chunk[..keep]);
            }
            Ok::<_, String>(clean(&retained))
        };
        let (stdout, stderr) = tokio::try_join!(read, errors)?;
        let success = child
            .wait()
            .await
            .map_err(|_| "No se pudo esperar a Git.")?
            .success();
        Ok(Output {
            success,
            stdout,
            stderr,
        })
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Git superó {seconds} segundos. Revisá el estado antes de reintentar."
        )),
    }
}
fn checked(output: Output) -> Result<Vec<u8>, String> {
    if output.success {
        Ok(output.stdout)
    } else if output.stderr.is_empty() {
        Err("Git no pudo completar la operación.".into())
    } else {
        Err(output.stderr)
    }
}
async fn root(path: &Path) -> Result<PathBuf, String> {
    let output = checked(command(path, &["rev-parse", "--show-toplevel"], 10, 32 * 1024).await?)?;
    let value = std::str::from_utf8(&output)
        .map_err(|_| "La ruta del repositorio no es UTF-8.")?
        .trim_end_matches(['\r', '\n']);
    Ok(PathBuf::from(value))
}
pub async fn status(path: &Path) -> Result<GitStatus, String> {
    parse_status(&checked(
        command(
            path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
            10,
            256 * 1024,
        )
        .await?,
    )?)
}
pub async fn snapshot(path: &Path) -> Result<Snapshot, String> {
    let probe = command(path, &["rev-parse", "--show-toplevel"], 10, 32 * 1024).await?;
    if !probe.success {
        if probe.stderr.contains("not a git repository") {
            return Ok(Snapshot {
                repository: false,
                status: None,
                history: Vec::new(),
                remote: false,
                origin_url: None,
                user_name: None,
                user_email: None,
                branches: Vec::new(),
                current_branch: None,
            });
        }
        return Err(probe.stderr);
    }
    let repo = root(path).await?;
    let mut status = status(&repo).await?;
    fill_line_changes(&repo, &mut status.files).await?;
    let has_head = command(&repo, &["rev-parse", "--verify", "HEAD"], 10, 4096)
        .await?
        .success;
    let history = if has_head {
        let bytes = checked(
            command(
                &repo,
                &[
                    "log",
                    "-50",
                    "--format=%H%x00%s%x00%an%x00%as%x00",
                    "--no-show-signature",
                ],
                10,
                256 * 1024,
            )
            .await?,
        )?;
        let parts = bytes.split(|b| *b == 0).collect::<Vec<_>>();
        parts
            .chunks_exact(4)
            .map(|p| Commit {
                id: clean(p[0]).trim().to_string(),
                title: clean(p[1]),
                author: clean(p[2]),
                date: clean(p[3]),
            })
            .collect()
    } else {
        Vec::new()
    };
    let remote = !checked(command(&repo, &["remote"], 10, 32 * 1024).await?)?.is_empty();
    let origin_url = origin_url(&repo).await?;
    let (user_name, user_email) = tokio::try_join!(
        identity_value(&repo, "user.name"),
        identity_value(&repo, "user.email")
    )?;
    let (branches, current_branch) = branches(&repo).await?;
    Ok(Snapshot {
        repository: true,
        status: Some(status),
        history,
        remote,
        origin_url,
        user_name,
        user_email,
        branches,
        current_branch,
    })
}
async fn identity_value(repo: &Path, key: &str) -> Result<Option<String>, String> {
    // Read the effective value, including inherited configuration; never write globally.
    let output = command(repo, &["config", "--get", key], 10, 16 * 1024).await?;
    if output.success {
        Ok(Some(
            String::from_utf8(output.stdout)
                .map_err(|_| "La identidad de Git no es UTF-8.")?
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        ))
    } else if output.stderr.trim().is_empty() {
        Ok(None)
    } else {
        Err(output.stderr)
    }
}
async fn origin_url(repo: &Path) -> Result<Option<String>, String> {
    let output = command(repo, &["remote", "get-url", "origin"], 10, 32 * 1024).await?;
    if output.success {
        Ok(Some(
            String::from_utf8(output.stdout)
                .map_err(|_| "La URL no es UTF-8.")?
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        ))
    } else if output.stderr.contains("No such remote") {
        Ok(None)
    } else {
        Err(output.stderr)
    }
}
// Local branches only: selecting a remote must not implicitly create a branch.
async fn branches(repo: &Path) -> Result<(Vec<String>, Option<String>), String> {
    let bytes = checked(
        command(
            repo,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            10,
            64 * 1024,
        )
        .await?,
    )?;
    let mut names = std::str::from_utf8(&bytes)
        .map_err(|_| "Las ramas no son UTF-8.")?
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let head = command(
        repo,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        10,
        4096,
    )
    .await?;
    let current = if head.success {
        Some(
            std::str::from_utf8(&head.stdout)
                .map_err(|_| "La rama no es UTF-8.")?
                .trim()
                .to_string(),
        )
    } else {
        None
    };
    // Unborn branches have a symbolic HEAD but no refs/heads entry yet.
    if let Some(current) = &current {
        if !names.contains(current) {
            names.push(current.clone());
            names.sort();
        }
    }
    Ok((names, current))
}
// NUL records preserve paths containing tabs and newlines, including rename pairs.
fn numstat(bytes: &[u8]) -> Result<std::collections::HashMap<String, LineChanges>, String> {
    let mut records = bytes.split(|byte| *byte == 0);
    let mut result = std::collections::HashMap::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let added = fields.next().ok_or("Git no informó líneas agregadas.")?;
        let removed = fields.next().ok_or("Git no informó líneas eliminadas.")?;
        let mut path = fields.next().ok_or("Git no informó la ruta del cambio.")?;
        if path.is_empty() {
            let _original = records.next().ok_or("Git no informó la ruta anterior.")?;
            path = records.next().ok_or("Git no informó la ruta renombrada.")?;
        }
        if path.is_empty() {
            return Err("Git informó una ruta vacía en las estadísticas.".into());
        }
        let stats = if added == b"-" && removed == b"-" {
            LineChanges::Binary
        } else {
            let number = |bytes: &[u8]| {
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or("Git informó un contador de líneas inválido.")
            };
            LineChanges::Text {
                added: number(added)?,
                removed: number(removed)?,
            }
        };
        let path =
            std::str::from_utf8(path).map_err(|_| "Git informó una ruta que no es UTF-8.")?;
        result.insert(path.into(), stats);
    }
    Ok(result)
}
async fn fill_line_changes(repo: &Path, files: &mut [GitFile]) -> Result<(), String> {
    let cached = command(
        repo,
        &[
            "diff",
            "--cached",
            "--numstat",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
        ],
        10,
        256 * 1024,
    );
    let working = command(
        repo,
        &["diff", "--numstat", "-z", "--no-ext-diff", "--no-textconv"],
        10,
        256 * 1024,
    );
    let (cached, working) = tokio::try_join!(cached, working)?;
    let cached = numstat(&checked(cached)?)?;
    let working = numstat(&checked(working)?)?;
    for file in files.iter_mut() {
        file.staged_lines = cached.get(&file.target).cloned();
        file.unstaged_lines = working.get(&file.target).cloned();
    }
    // No temporary index and no staging as a side effect. Git computes each new
    // file's statistics. Bound concurrent children, file size, count and latency.
    use futures_util::StreamExt;
    let targets = files
        .iter()
        .filter(|file| file.index_code == "?")
        .take(100)
        .map(|file| file.target.clone())
        .collect::<Vec<_>>();
    let mut pending = futures_util::stream::iter(targets.into_iter().map(|target| async move {
        let path = repo.join(&target);
        let stats =
            if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.len() <= 8 * 1024 * 1024) {
                let absolute = path.to_str();
                if let Some(absolute) = absolute {
                    match command(
                        repo,
                        &[
                            "diff",
                            "--no-index",
                            "--numstat",
                            "-z",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--",
                            "/dev/null",
                            absolute,
                        ],
                        5,
                        32 * 1024,
                    )
                    .await
                    {
                        Ok(output) => numstat(&output.stdout)
                            .ok()
                            .and_then(|stats| stats.into_values().next()),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            };
        (target, stats)
    }))
    .buffer_unordered(2);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Ok(Some((target, stats))) = tokio::time::timeout_at(deadline, pending.next()).await {
        if let Some(file) = files.iter_mut().find(|file| file.target == target) {
            file.unstaged_lines = stats;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum Action {
    Init,
    Stage(String),
    StageAll,
    Unstage(String),
    Commit(String),
    Pull,
    Push,
    Remote(String),
    Settings {
        name: String,
        email: String,
        origin: String,
    },
    Switch(String),
}
pub async fn perform(path: &Path, action: Action) -> Result<String, String> {
    if matches!(action, Action::Init) {
        if snapshot(path).await?.repository {
            return Err("La carpeta ya pertenece a un repositorio.".into());
        }
        checked(command(path, &["init"], 30, 128 * 1024).await?)?;
        return Ok("Repositorio inicializado.".into());
    }
    let repo = root(path).await?;
    if let Action::Settings {
        name,
        email,
        origin,
    } = &action
    {
        for (label, value) in [("nombre", name), ("email", email)] {
            if value.len() > 1024 || value.chars().any(char::is_control) {
                return Err(format!("Ingresá un {label} válido para los commits."));
            }
        }
        if origin.len() > 4096 || origin.chars().any(char::is_control) {
            return Err("Ingresá una URL o ruta de remoto válida.".into());
        }
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if value.trim().is_empty() {
                continue;
            }
            checked(
                command(
                    &repo,
                    &[
                        "config",
                        "--local",
                        "--replace-all",
                        "--",
                        key,
                        value.trim(),
                    ],
                    10,
                    16 * 1024,
                )
                .await?,
            )
            .map_err(|error| format!("No se pudo guardar {key} en este repositorio: {error}"))?;
        }
        if !origin.trim().is_empty() {
            let operation = if origin_url(&repo).await?.is_some() {
                "set-url"
            } else {
                "add"
            };
            checked(command(&repo, &["remote", operation, "--", "origin", origin.trim()], 30, 128 * 1024).await?)
                .map_err(|error| format!("La identidad quedó guardada localmente, pero no se pudo configurar origin: {error}"))?;
        }
        return Ok("Configuración guardada en este repositorio.".into());
    }
    let mut owned = Vec::<String>::new();
    let (args, message, seconds): (Vec<&str>, &str, u64) = match &action {
        Action::StageAll => {
            if !status(&repo).await?.files.iter().any(|file| file.unstaged) {
                return Ok("Todos los archivos ya están preparados.".into());
            }
            (vec!["add", "--all"], "Todos los cambios preparados.", 30)
        }
        Action::Stage(target) | Action::Unstage(target) => {
            let current = status(&repo).await?;
            let file = current
                .files
                .iter()
                .find(|file| &file.target == target)
                .ok_or("El archivo ya no aparece entre los cambios. Actualizá el panel.")?;
            owned.push(file.target.clone());
            if let Some(original) = &file.original {
                owned.push(original.clone());
            }
            let mut args = if matches!(action, Action::Stage(_)) {
                vec!["add", "--"]
            } else if command(&repo, &["rev-parse", "--verify", "HEAD"], 10, 4096)
                .await?
                .success
            {
                vec!["restore", "--staged", "--"]
            } else {
                vec!["rm", "--cached", "-f", "--ignore-unmatch", "--"]
            };
            args.extend(owned.iter().map(String::as_str));
            (
                args,
                if matches!(action, Action::Stage(_)) {
                    "Archivo preparado."
                } else {
                    "Archivo retirado del commit."
                },
                30,
            )
        }
        Action::Commit(message) => {
            if message.trim().is_empty() {
                return Err("Escribí el mensaje del commit.".into());
            }
            if message.len() > 16 * 1024 {
                return Err("El mensaje del commit supera 16 KiB.".into());
            }
            if !status(&repo).await?.files.iter().any(|file| file.staged) {
                return Err("Prepará al menos un archivo para el commit.".into());
            }
            (vec!["commit", "-m", message], "Commit creado.", 120)
        }
        Action::Switch(branch) => {
            // Re-read immediately before switching; the UI snapshot may be stale.
            if !status(&repo).await?.files.is_empty() {
                return Err(
                    "Comiteá todos los cambios pendientes antes de cambiar de rama.".into(),
                );
            }
            let (names, current) = branches(&repo).await?;
            if !names.contains(branch) {
                return Err("La rama ya no existe. Actualizá el panel.".into());
            }
            if current.as_ref() == Some(branch) {
                return Ok("Ya estás en esta rama.".into());
            }
            (vec!["switch", "--", branch], "Rama cambiada.", 30)
        }
        Action::Pull => (vec!["pull", "--ff-only"], "Pull completado.", 120),
        Action::Push => {
            let upstream = command(
                &repo,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
                10,
                4096,
            )
            .await?
            .success;
            if upstream {
                (vec!["push"], "Push completado.", 120)
            } else {
                let branch =
                    checked(command(&repo, &["symbolic-ref", "--short", "HEAD"], 10, 4096).await?)?;
                let branch = std::str::from_utf8(&branch)
                    .map_err(|_| "La rama no es UTF-8.")?
                    .trim()
                    .to_string();
                let remotes = checked(command(&repo, &["remote"], 10, 32 * 1024).await?)?;
                let remotes = String::from_utf8_lossy(&remotes)
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let remote = if remotes.iter().any(|remote| remote == "origin") {
                    "origin".to_string()
                } else if remotes.len() == 1 {
                    remotes[0].clone()
                } else {
                    return Err("Configurá el upstream de esta rama antes de hacer push.".into());
                };
                owned.extend([remote, branch]);
                (
                    vec!["push", "--set-upstream", "--", &owned[0], &owned[1]],
                    "Push completado; upstream configurado.",
                    120,
                )
            }
        }
        Action::Remote(url) => {
            if url.trim().is_empty() || url.len() > 4096 {
                return Err("Ingresá una URL o ruta de remoto válida.".into());
            }
            let operation = if origin_url(&repo).await?.is_some() {
                "set-url"
            } else {
                "add"
            };
            (
                vec!["remote", operation, "--", "origin", url.trim()],
                "Remoto origin configurado.",
                30,
            )
        }
        Action::Init | Action::Settings { .. } => unreachable!(),
    };
    checked(command(&repo, &args, seconds, 128 * 1024).await?)?;
    Ok(message.into())
}
pub async fn diff(path: &Path, target: &str, commit: bool) -> Result<String, String> {
    diff_scope(path, target, commit, None).await
}
pub async fn file_diff(path: &Path, target: &str, staged: bool) -> Result<String, String> {
    diff_scope(path, target, false, Some(staged)).await
}
async fn diff_scope(
    path: &Path,
    target: &str,
    commit: bool,
    scope: Option<bool>,
) -> Result<String, String> {
    let repo = root(path).await?;
    if commit {
        if !((target.len() == 40 || target.len() == 64)
            && target.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("Identificador de commit inválido.".into());
        }
        let bytes = checked(
            command(
                &repo,
                &[
                    "show",
                    "--format=fuller",
                    "--stat",
                    "--patch",
                    "--no-ext-diff",
                    "--no-textconv",
                    target,
                    "--",
                ],
                10,
                128 * 1024,
            )
            .await?,
        )?;
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    let current = status(&repo).await?;
    let file = current
        .files
        .iter()
        .find(|file| file.target == target)
        .ok_or("El archivo ya no tiene cambios. Actualizá el panel.")?;
    if file.status.contains("sin seguimiento") {
        let preview = crate::explorer::preview(&repo, target)?;
        let lines = preview.text.lines().collect::<Vec<_>>();
        let mut text = format!(
            "diff --git a/{target} b/{target}\nnew file mode 100644\n--- /dev/null\n+++ b/{target}\n@@ -0,0 +1,{} @@\n",
            lines.len()
        );
        for line in lines {
            text.push('+');
            text.push_str(line);
            text.push('\n');
        }
        if preview.truncated {
            text.push_str("\n[Vista limitada a los primeros 128 KiB]\n");
        }
        return Ok(text);
    }
    let mut text = String::new();
    let paths = std::iter::once(file.target.as_str())
        .chain(file.original.as_deref())
        .collect::<Vec<_>>();
    for staged in [true, false] {
        if scope.is_some_and(|scope| scope != staged) {
            continue;
        }
        let mut args = vec!["diff", "--no-ext-diff", "--no-textconv"];
        if staged {
            args.push("--cached");
        }
        args.push("--");
        args.extend(paths.iter().copied());
        let bytes = checked(command(&repo, &args, 10, 64 * 1024).await?)?;
        if !bytes.is_empty() {
            text.push_str(if staged {
                "PREPARADO PARA COMMIT\n\n"
            } else {
                "CAMBIOS EN TRABAJO\n\n"
            });
            text.push_str(&String::from_utf8_lossy(&bytes));
            text.push('\n');
        }
    }
    if text.is_empty() {
        text = "Git no informó diferencias de texto (puede haber cambios de permisos o archivos binarios).".into();
    }
    Ok(text)
}

fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn parse_status(bytes: &[u8]) -> Result<GitStatus, String> {
    let mut records = bytes.split(|b| *b == 0).filter(|part| !part.is_empty());
    let header = records
        .next()
        .filter(|part| part.starts_with(b"## "))
        .ok_or("Git no informó la rama.")?;
    let branch = clean(&header[3..]);
    let branch = branch
        .strip_prefix("No commits yet on ")
        .or_else(|| branch.strip_prefix("Initial commit on "))
        .unwrap_or(&branch)
        .to_string();
    let mut status = GitStatus {
        branch,
        files: Vec::new(),
    };
    let records = bytes
        .split(|b| *b == 0)
        .skip(1)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() < 4 || record[2] != b' ' {
            return Err("El estado de Git está incompleto.".into());
        }
        let x = record[0];
        let y = record[1];
        let target = std::str::from_utf8(&record[3..])
            .map_err(|_| "Git informó una ruta que no es UTF-8.")?
            .to_string();
        let mut path = clean(&record[3..]);
        let mut original_path = None;
        if [x, y].iter().any(|c| matches!(c, b'R' | b'C')) {
            index += 1;
            let original = records
                .get(index)
                .ok_or("Git no informó la ruta original.")?;
            original_path = Some(
                std::str::from_utf8(original)
                    .map_err(|_| "La ruta original no es UTF-8.")?
                    .to_string(),
            );
            path = format!("{} → {path}", clean(original));
        }
        let description = if x == b'?' && y == b'?' {
            "Nuevo · sin seguimiento".into()
        } else if x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D') {
            "Conflicto".into()
        } else {
            let mut parts = Vec::new();
            if x != b' ' {
                parts.push(format!("Preparado ({})", x as char));
            }
            if y != b' ' {
                parts.push(format!("En trabajo ({})", y as char));
            }
            parts.join(" · ")
        };
        status.files.push(GitFile {
            path,
            target,
            original: original_path,
            staged: x != b' ' && x != b'?',
            unstaged: y != b' ',
            status: description,
            index_code: (x as char).to_string(),
            worktree_code: if y == b'?' {
                "U".into()
            } else {
                (y as char).to_string()
            },
            staged_lines: None,
            unstaged_lines: None,
        });
        index += 1;
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_git(path: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    fn identity(path: &Path) {
        fixture_git(path, &["config", "user.name", "Harness Fixture"]);
        fixture_git(path, &["config", "user.email", "fixture@example.invalid"]);
        fixture_git(path, &["config", "commit.gpgSign", "false"]);
        fixture_git(
            path,
            &["config", "core.hooksPath", ".git/fixture-empty-hooks"],
        );
    }
    #[tokio::test]
    async fn selecting_all_stages_modifications_deletions_and_new_files() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        fixture_git(repo, &["init"]);
        identity(repo);
        for name in ["modified.txt", "deleted.txt"] {
            std::fs::write(repo.join(name), "baseline\n").unwrap();
        }
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        fixture_git(repo, &["add", "--all"]);
        fixture_git(repo, &["commit", "-m", "Baseline"]);
        std::fs::write(repo.join("modified.txt"), "first edit\n").unwrap();
        fixture_git(repo, &["add", "modified.txt"]);
        std::fs::write(repo.join("modified.txt"), "latest edit\n").unwrap();
        std::fs::remove_file(repo.join("deleted.txt")).unwrap();
        std::fs::write(repo.join("new.txt"), "new\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "ignored\n").unwrap();
        perform(repo, Action::StageAll).await.unwrap();
        let state = status(repo).await.unwrap();
        assert_eq!(state.files.len(), 3);
        assert!(state.files.iter().all(|file| file.staged && !file.unstaged));
        assert_eq!(
            fixture_git(repo, &["show", ":modified.txt"]),
            b"latest edit\n"
        );
        assert!(
            perform(repo, Action::StageAll)
                .await
                .unwrap()
                .contains("ya están")
        );
    }
    #[tokio::test]
    async fn repository_identity_is_read_and_saved_without_changing_other_repositories() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fixture_git(first.path(), &["init"]);
        fixture_git(second.path(), &["init"]);
        let inherited = snapshot(second.path()).await.unwrap();
        identity(first.path());
        let before = snapshot(first.path()).await.unwrap();
        assert_eq!(before.user_name.as_deref(), Some("Harness Fixture"));
        assert_eq!(
            before.user_email.as_deref(),
            Some("fixture@example.invalid")
        );
        perform(
            first.path(),
            Action::Settings {
                name: "Proyecto de prueba".into(),
                email: "local@example.invalid".into(),
                origin: "".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            fixture_git(first.path(), &["config", "--local", "--get", "user.name"]),
            "Proyecto de prueba\n".as_bytes()
        );
        assert_eq!(
            fixture_git(first.path(), &["config", "--local", "--get", "user.email"]),
            b"local@example.invalid\n"
        );
        let after = snapshot(first.path()).await.unwrap();
        assert_eq!(after.user_name.as_deref(), Some("Proyecto de prueba"));
        assert_eq!(after.user_email.as_deref(), Some("local@example.invalid"));
        assert!(after.origin_url.is_none());
        let untouched = snapshot(second.path()).await.unwrap();
        assert_eq!(untouched.user_name, inherited.user_name);
        assert_eq!(untouched.user_email, inherited.user_email);
        perform(
            first.path(),
            Action::Settings {
                name: "Otro autor".into(),
                email: "otro@example.invalid".into(),
                origin: "https://example.invalid/repo.git".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            snapshot(first.path()).await.unwrap().origin_url.as_deref(),
            Some("https://example.invalid/repo.git")
        );
        let config = std::fs::read(first.path().join(".git/config")).unwrap();
        assert!(
            perform(
                first.path(),
                Action::Settings {
                    name: "".into(),
                    email: "bad\nemail".into(),
                    origin: "https://example.invalid/repo.git".into(),
                }
            )
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read(first.path().join(".git/config")).unwrap(),
            config
        );
    }
    #[tokio::test]
    async fn origin_configuration_can_be_created_and_updated() {
        let temp = tempfile::tempdir().unwrap();
        fixture_git(temp.path(), &["init"]);
        assert!(snapshot(temp.path()).await.unwrap().origin_url.is_none());
        perform(
            temp.path(),
            Action::Remote("https://example.invalid/first.git".into()),
        )
        .await
        .unwrap();
        perform(
            temp.path(),
            Action::Remote("https://example.invalid/second.git".into()),
        )
        .await
        .unwrap();
        assert_eq!(
            snapshot(temp.path()).await.unwrap().origin_url.as_deref(),
            Some("https://example.invalid/second.git")
        );
        std::fs::write(temp.path().join("new.txt"), "one\ntwo\n").unwrap();
        let diff = file_diff(temp.path(), "new.txt", false).await.unwrap();
        assert!(diff.contains("--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"));
    }
    #[tokio::test]
    async fn switching_branches_requires_clean_index_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        fixture_git(repo, &["init", "-b", "main"]);
        identity(repo);
        let initial = snapshot(repo).await.unwrap();
        assert_eq!(initial.branches, vec!["main"]);
        assert_eq!(initial.current_branch.as_deref(), Some("main"));
        std::fs::write(repo.join("tracked.txt"), "baseline\n").unwrap();
        fixture_git(repo, &["add", "tracked.txt"]);
        fixture_git(repo, &["commit", "-m", "Baseline"]);
        fixture_git(repo, &["branch", "feature"]);
        // Git would allow these changes to follow us: the harness rejects them.
        for stage in [false, true] {
            std::fs::write(repo.join("tracked.txt"), "pending\n").unwrap();
            if stage {
                fixture_git(repo, &["add", "tracked.txt"]);
            }
            let index = std::fs::read(repo.join(".git/index")).unwrap();
            assert!(
                perform(repo, Action::Switch("feature".into()))
                    .await
                    .unwrap_err()
                    .contains("pendientes")
            );
            assert_eq!(branches(repo).await.unwrap().1.as_deref(), Some("main"));
            assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
            assert_eq!(
                std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
                "pending\n"
            );
            fixture_git(
                repo,
                &[
                    "restore",
                    "--source=HEAD",
                    "--staged",
                    "--worktree",
                    "tracked.txt",
                ],
            );
        }
        std::fs::write(repo.join("new.txt"), "new").unwrap();
        assert!(
            perform(repo, Action::Switch("feature".into()))
                .await
                .is_err()
        );
        std::fs::remove_file(repo.join("new.txt")).unwrap();
        perform(repo, Action::Switch("feature".into()))
            .await
            .unwrap();
        assert_eq!(
            snapshot(repo).await.unwrap().current_branch.as_deref(),
            Some("feature")
        );
        assert!(
            perform(repo, Action::Switch("missing".into()))
                .await
                .is_err()
        );
        fixture_git(repo, &["switch", "--detach"]);
        let detached = snapshot(repo).await.unwrap();
        assert!(detached.current_branch.is_none());
        assert_eq!(detached.branches, vec!["feature", "main"]);
        perform(repo, Action::Switch("main".into())).await.unwrap();
    }
    #[test]
    fn numstat_preserves_tab_paths_rename_pairs_binary_and_large_counts() {
        let parsed =
            numstat(b"12\t3\tfile\tname.txt\0-\t-\timage.png\01000000\t42\t\0old name\0new name\0")
                .unwrap();
        assert_eq!(
            parsed["file\tname.txt"],
            LineChanges::Text {
                added: 12,
                removed: 3
            }
        );
        assert_eq!(parsed["image.png"], LineChanges::Binary);
        assert_eq!(
            parsed["new name"],
            LineChanges::Text {
                added: 1_000_000,
                removed: 42
            }
        );
        assert!(numstat(b"bad\t3\tfile\0").is_err());
        assert!(numstat(b"1\t0\t\0old\0").is_err());
    }
    #[tokio::test]
    async fn snapshot_reports_separate_index_and_worktree_line_counts_without_staging() {
        let temp = tempfile::tempdir().unwrap();
        perform(temp.path(), Action::Init).await.unwrap();
        identity(temp.path());
        std::fs::write(temp.path().join("mixed.txt"), "a\nb\nc\n").unwrap();
        perform(temp.path(), Action::Stage("mixed.txt".into()))
            .await
            .unwrap();
        perform(temp.path(), Action::Commit("Baseline".into()))
            .await
            .unwrap();
        std::fs::write(temp.path().join("mixed.txt"), "a\nx\nc\ny\n").unwrap();
        perform(temp.path(), Action::Stage("mixed.txt".into()))
            .await
            .unwrap();
        std::fs::write(temp.path().join("mixed.txt"), "a\nz\ny\nq\nr\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "uno\ndos").unwrap();
        std::fs::write(temp.path().join("empty.txt"), "").unwrap();
        std::fs::write(temp.path().join("binary.dat"), [0, 1, 2, 3]).unwrap();
        let before = std::fs::read(temp.path().join(".git/index")).unwrap();
        let files = snapshot(temp.path()).await.unwrap().status.unwrap().files;
        let mixed = files
            .iter()
            .find(|file| file.target == "mixed.txt")
            .unwrap();
        assert_eq!(
            mixed.staged_lines,
            Some(LineChanges::Text {
                added: 2,
                removed: 1
            })
        );
        assert_eq!(
            mixed.unstaged_lines,
            Some(LineChanges::Text {
                added: 3,
                removed: 2
            })
        );
        assert_eq!(
            files
                .iter()
                .find(|file| file.target == "new.txt")
                .unwrap()
                .unstaged_lines,
            Some(LineChanges::Text {
                added: 2,
                removed: 0
            })
        );
        assert_eq!(
            files
                .iter()
                .find(|file| file.target == "empty.txt")
                .unwrap()
                .unstaged_lines,
            Some(LineChanges::Text {
                added: 0,
                removed: 0
            })
        );
        assert_eq!(
            files
                .iter()
                .find(|file| file.target == "binary.dat")
                .unwrap()
                .unstaged_lines,
            Some(LineChanges::Binary)
        );
        assert_eq!(
            before,
            std::fs::read(temp.path().join(".git/index")).unwrap()
        );
        let prepared = file_diff(temp.path(), "mixed.txt", true).await.unwrap();
        let pending = file_diff(temp.path(), "mixed.txt", false).await.unwrap();
        assert!(prepared.contains("+x") && !prepared.contains("+q"));
        assert!(pending.contains("+q") && !pending.contains("+x"));
    }
    #[tokio::test]
    async fn init_staging_history_and_diffs_preserve_unprepared_work() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!snapshot(temp.path()).await.unwrap().repository);
        perform(temp.path(), Action::Init).await.unwrap();
        identity(temp.path());
        assert!(snapshot(temp.path()).await.unwrap().history.is_empty());
        assert!(perform(temp.path(), Action::Init).await.is_err());
        std::fs::create_dir(temp.path().join("nested")).unwrap();
        std::fs::write(temp.path().join("a [1].txt"), "before\n").unwrap();
        std::fs::write(temp.path().join("leave.txt"), "unprepared\n").unwrap();
        assert!(
            perform(temp.path(), Action::Commit("empty".into()))
                .await
                .is_err()
        );
        assert!(
            perform(temp.path(), Action::Stage("../outside.txt".into()))
                .await
                .is_err()
        );
        perform(
            &temp.path().join("nested"),
            Action::Stage("a [1].txt".into()),
        )
        .await
        .unwrap();
        std::fs::write(temp.path().join("a [1].txt"), "after\n").unwrap();
        perform(temp.path(), Action::Unstage("a [1].txt".into()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a [1].txt")).unwrap(),
            "after\n"
        );
        assert!(
            !status(temp.path())
                .await
                .unwrap()
                .files
                .iter()
                .any(|file| file.staged)
        );
        perform(temp.path(), Action::Stage("a [1].txt".into()))
            .await
            .unwrap();
        let patch = diff(temp.path(), "a [1].txt", false).await.unwrap();
        assert!(patch.contains("PREPARADO") && patch.contains("+after"));
        perform(temp.path(), Action::Commit("First · unicode".into()))
            .await
            .unwrap();
        let snapshot = snapshot(temp.path()).await.unwrap();
        assert_eq!(snapshot.history.len(), 1);
        assert_eq!(snapshot.history[0].title, "First · unicode");
        assert!(
            snapshot
                .status
                .unwrap()
                .files
                .iter()
                .any(|file| file.target == "leave.txt" && !file.staged)
        );
        let committed = diff(temp.path(), &snapshot.history[0].id, true)
            .await
            .unwrap();
        assert!(committed.contains("+after") && !committed.contains("unprepared"));
        std::fs::write(temp.path().join("a [1].txt"), "third\n").unwrap();
        perform(temp.path(), Action::Stage("a [1].txt".into()))
            .await
            .unwrap();
        perform(temp.path(), Action::Unstage("a [1].txt".into()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a [1].txt")).unwrap(),
            "third\n"
        );
        assert!(
            !status(temp.path())
                .await
                .unwrap()
                .files
                .iter()
                .any(|file| file.staged)
        );
        fixture_git(temp.path(), &["restore", "--", "a [1].txt"]);
        fixture_git(temp.path(), &["mv", "--", "a [1].txt", "renamed.txt"]);
        perform(temp.path(), Action::Unstage("renamed.txt".into()))
            .await
            .unwrap();
        assert!(temp.path().join("renamed.txt").exists());
        perform(temp.path(), Action::Stage("a [1].txt".into()))
            .await
            .unwrap();
        perform(temp.path(), Action::Stage("renamed.txt".into()))
            .await
            .unwrap();
        assert!(
            status(temp.path())
                .await
                .unwrap()
                .files
                .iter()
                .any(|file| file.original.as_deref() == Some("a [1].txt"))
        );
        assert!(diff(temp.path(), "--all", true).await.is_err());
    }
    #[tokio::test]
    async fn push_sets_upstream_and_pull_rejects_diverged_history() {
        let remote = tempfile::tempdir().unwrap();
        fixture_git(remote.path(), &["init", "--bare", "--quiet"]);
        let local = tempfile::tempdir().unwrap();
        perform(local.path(), Action::Init).await.unwrap();
        identity(local.path());
        fixture_git(local.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
        std::fs::write(local.path().join("first.txt"), "first\n").unwrap();
        perform(local.path(), Action::Stage("first.txt".into()))
            .await
            .unwrap();
        perform(local.path(), Action::Commit("First".into()))
            .await
            .unwrap();
        perform(
            local.path(),
            Action::Remote(remote.path().to_str().unwrap().into()),
        )
        .await
        .unwrap();
        assert!(snapshot(local.path()).await.unwrap().remote);
        perform(local.path(), Action::Push).await.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&fixture_git(
                local.path(),
                &["rev-parse", "--abbrev-ref", "@{upstream}"]
            ))
            .trim(),
            "origin/main"
        );
        let peer = tempfile::tempdir().unwrap();
        fixture_git(
            peer.path(),
            &[
                "clone",
                "--quiet",
                "--branch",
                "main",
                remote.path().to_str().unwrap(),
                ".",
            ],
        );
        identity(peer.path());
        std::fs::write(peer.path().join("second.txt"), "second\n").unwrap();
        perform(peer.path(), Action::Stage("second.txt".into()))
            .await
            .unwrap();
        perform(peer.path(), Action::Commit("Second".into()))
            .await
            .unwrap();
        perform(peer.path(), Action::Push).await.unwrap();
        perform(local.path(), Action::Pull).await.unwrap();
        assert!(local.path().join("second.txt").exists());
        std::fs::write(local.path().join("local.txt"), "local\n").unwrap();
        perform(local.path(), Action::Stage("local.txt".into()))
            .await
            .unwrap();
        perform(local.path(), Action::Commit("Local".into()))
            .await
            .unwrap();
        std::fs::write(peer.path().join("peer.txt"), "peer\n").unwrap();
        perform(peer.path(), Action::Stage("peer.txt".into()))
            .await
            .unwrap();
        perform(peer.path(), Action::Commit("Peer".into()))
            .await
            .unwrap();
        perform(peer.path(), Action::Push).await.unwrap();
        let before = fixture_git(local.path(), &["rev-parse", "HEAD"]);
        assert!(perform(local.path(), Action::Pull).await.is_err());
        assert_eq!(before, fixture_git(local.path(), &["rev-parse", "HEAD"]));
        assert!(perform(local.path(), Action::Push).await.is_err());
    }
    #[tokio::test]
    async fn repository_status_is_read_only_and_distinguishes_staged_and_new_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(temp.path().join("staged.txt"), "fixture").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "--", "staged.txt"])
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(temp.path().join("new.txt"), "fixture").unwrap();
        let index = std::fs::read(temp.path().join(".git/index")).unwrap();
        let snapshot = status(temp.path()).await.unwrap();
        assert!(
            snapshot
                .files
                .iter()
                .any(|file| file.path == "staged.txt" && file.status.contains("Preparado"))
        );
        assert!(
            snapshot
                .files
                .iter()
                .any(|file| file.path == "new.txt" && file.status.contains("sin seguimiento"))
        );
        assert_eq!(
            std::fs::read(temp.path().join(".git/index")).unwrap(),
            index
        );
        let nonrepo = tempfile::tempdir().unwrap();
        assert!(status(nonrepo.path()).await.is_err());
    }
    #[test]
    fn nul_paths_preserve_renames_and_combined_index_worktree_changes() {
        assert_eq!(
            parse_status(b"## No commits yet on master\0")
                .unwrap()
                .branch,
            "master"
        );
        assert_eq!(
            parse_status(b"## Initial commit on main\0").unwrap().branch,
            "main"
        );
        let status = parse_status(b"## main...origin/main [ahead 2]\0MM src/main.rs\0R  new name.rs\0old.rs\0?? new.txt\0UU conflict.rs\0").unwrap();
        assert!(status.branch.contains("ahead 2"));
        assert_eq!(status.files.len(), 4);
        assert!(status.files[0].status.contains("Preparado"));
        assert!(status.files[0].status.contains("En trabajo"));
        assert_eq!(status.files[1].path, "old.rs → new name.rs");
        assert_eq!(status.files[3].status, "Conflicto");
        assert!(parse_status(b"## main\0R  missing.rs\0").is_err());
        assert!(parse_status(b"## main\0").unwrap().files.is_empty());
    }
}
