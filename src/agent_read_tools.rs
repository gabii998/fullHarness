//! Read-only agent discovery tools. Output and traversal are deliberately bounded.
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const OUTPUT: usize = 7600; // Leave room for response metadata under 8 KiB.
const FILE_BYTES: u64 = 16 * 1024 * 1024;
const SCAN_BYTES: u64 = 64 * 1024 * 1024;
const FILES: usize = 5000;

pub fn strings<'a>(args: &'a Value, name: &str, empty: bool) -> Result<Vec<&'a str>, String> {
    let values = args[name]
        .as_array()
        .filter(|a| a.len() <= 10 && (empty || !a.is_empty()))
        .ok_or_else(|| {
            format!(
                "{name}: se requieren {}–10 valores.",
                if empty { 0 } else { 1 }
            )
        })?;
    values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 1024)
                .ok_or_else(|| format!("{name}: texto inválido o demasiado largo."))
        })
        .collect()
}
fn number(args: &Value, name: &str, max: u64) -> Result<usize, String> {
    args[name]
        .as_u64()
        .filter(|n| *n <= max)
        .map(|n| n as usize)
        .ok_or_else(|| format!("{name}: valor fuera de rango."))
}
fn push(rows: &mut Vec<Value>, row: Value, bytes: &mut usize) -> bool {
    let cost = row.to_string().len() + 1;
    if *bytes + cost > OUTPUT {
        return false;
    }
    *bytes += cost;
    rows.push(row);
    true
}

// Do not follow symlinks, including directory links/junctions that resolve outside
// the repository or into private harness data. Sort for reproducible pagination.
fn files(root: &Path, private: &Path, paths: &[PathBuf]) -> Result<(Vec<PathBuf>, bool), String> {
    let mut pending = paths.to_vec();
    let mut found = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut limited = false;
    while let Some(path) = pending.pop() {
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| "No se pudo inspeccionar una ruta.")?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let resolved = path
            .canonicalize()
            .map_err(|_| "No se pudo resolver una ruta de búsqueda.")?;
        if !resolved.starts_with(root)
            || resolved.starts_with(private)
            || !visited.insert(resolved.clone())
        {
            continue;
        }
        if visited.len() > 20000 {
            limited = true;
            break;
        }
        if metadata.is_dir() {
            let mut entries = Vec::new();
            for entry in fs::read_dir(&path).map_err(|_| "No se pudo listar el directorio.")? {
                let entry = entry.map_err(|_| "No se pudo leer una entrada.")?;
                if matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "target" | "node_modules" | ".venv" | "__pycache__")
                ) {
                    continue;
                }
                entries.push(entry.path());
                if entries.len() + pending.len() > 20000 {
                    limited = true;
                    break;
                }
            }
            entries.sort();
            pending.extend(entries.into_iter().rev());
        } else if metadata.is_file() {
            if metadata.len() > FILE_BYTES {
                limited = true;
                continue;
            }
            found.insert(resolved);
            if found.len() >= FILES {
                limited = true;
                break;
            }
        }
    }
    Ok((found.into_iter().collect(), limited))
}
fn text(path: &Path, scanned: &mut u64) -> Result<Option<String>, String> {
    let size = fs::metadata(path)
        .map_err(|_| "No se pudo leer el tamaño del archivo.")?
        .len();
    if size > FILE_BYTES || *scanned + size > SCAN_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "No se pudo abrir el archivo.")?
        .take(FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer el archivo.")?;
    *scanned += bytes.len() as u64;
    if bytes.len() as u64 > FILE_BYTES || bytes.contains(&0) {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok()) // Binary/non-UTF-8 files are not text results.
}

pub fn search(
    root: &Path,
    private: &Path,
    paths: &[PathBuf],
    args: &Value,
) -> Result<Value, String> {
    let patterns = strings(args, "patterns", false)?;
    let use_regex = args["regex"].as_bool().ok_or("regex debe ser booleano.")?;
    let context = number(args, "context_lines", 5)?;
    let offset = number(args, "offset", 1_000_000)?;
    let limit = number(args, "limit", 100)?;
    if limit == 0 {
        return Err("limit debe ser positivo.".into());
    }
    let expressions = if use_regex {
        Some(
            patterns
                .iter()
                .map(|p| {
                    regex::RegexBuilder::new(p)
                        .size_limit(1024 * 1024)
                        .build()
                        .map_err(|_| "Expresión regular inválida o demasiado grande.".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        None
    };
    let (files, mut scan_limited) = files(root, private, paths)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut scanned = 0;
    let mut seen = 0;
    let mut more = false;
    'scan: for path in files {
        if scanned >= SCAN_BYTES {
            scan_limited = true;
            break;
        }
        let Some(contents) = text(&path, &mut scanned)? else {
            if fs::metadata(&path)
                .map(|m| scanned + m.len() > SCAN_BYTES)
                .unwrap_or(false)
            {
                scan_limited = true;
            }
            continue;
        };
        let mut lines = contents.lines().enumerate();
        let mut previous = std::collections::VecDeque::with_capacity(context + 1);
        while let Some((index, line)) = lines.next() {
            previous.push_back(line);
            if previous.len() > context + 1 {
                previous.pop_front();
            }
            let matched = expressions
                .as_ref()
                .map(|r| r.iter().any(|r| r.is_match(line)))
                .unwrap_or_else(|| patterns.iter().any(|p| line.contains(p)));
            if !matched {
                continue;
            }
            if seen < offset {
                seen += 1;
                continue;
            }
            if rows.len() == limit {
                more = true;
                break 'scan;
            }
            let start = index.saturating_sub(context);
            let excerpt = previous
                .iter()
                .copied()
                .chain(lines.clone().take(context).map(|(_, line)| line))
                .collect::<Vec<_>>()
                .join("\n");
            let row = json!({"path":path.strip_prefix(root).unwrap().to_string_lossy(),
                "line":index + 1,"start_line":start + 1,"text":excerpt});
            if !push(&mut rows, row, &mut bytes) {
                // Oversized individual matches still expose their location, avoiding
                // pagination that repeatedly gets stuck on one huge source line.
                if rows.is_empty() {
                    let row = json!({"path":path.strip_prefix(root).unwrap().to_string_lossy(),
                        "line":index + 1,"text_omitted":true});
                    push(&mut rows, row, &mut bytes);
                }
                more = true;
                break 'scan;
            }
            seen += 1;
        }
    }
    Ok(
        json!({"next_offset":offset + rows.len(),"matches":rows,"more":more,"scan_limited":scan_limited}),
    )
}

pub fn outline(root: &Path, paths: &[PathBuf], args: &Value) -> Result<Value, String> {
    let offset = number(args, "offset", 1_000_000)?;
    let limit = number(args, "limit", 100)?;
    if limit == 0 {
        return Err("limit debe ser positivo.".into());
    }
    // Deliberately a lightweight declaration heuristic, not a language parser.
    let declaration = regex::Regex::new(r"^(?:(?:pub(?:\([^)]*\))?|export|default|async|unsafe|static|abstract)\s+)*(fn|struct|enum|trait|mod|type|def|class|function|interface)\s+([\p{L}_$][\p{L}\p{N}_$]*)").unwrap();
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut seen = 0;
    let mut scanned = 0;
    let mut more = false;
    'files: for path in paths {
        if !path.is_file() {
            return Err("file_outline requiere archivos, no directorios.".into());
        }
        let contents = text(path, &mut scanned)?
            .ok_or("El archivo no es UTF-8, es binario o supera el límite de lectura.")?;
        let markdown = matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("md" | "markdown")
        );
        let mut fence: Option<(u8, usize)> = None;
        for ((index, line), previous) in contents
            .lines()
            .enumerate()
            .zip(std::iter::once("").chain(contents.lines()))
        {
            let trimmed = line.trim_start();
            let mut item = None;
            if markdown {
                let first = trimmed.as_bytes().first().copied();
                if matches!(first, Some(b'`' | b'~')) {
                    let marker = first.unwrap();
                    let n = trimmed.bytes().take_while(|&b| b == marker).count();
                    if n >= 3 {
                        match fence {
                            None => fence = Some((marker, n)),
                            Some((m, length))
                                if m == marker && n >= length && trimmed[n..].trim().is_empty() =>
                            {
                                fence = None
                            }
                            _ => {}
                        }
                        continue;
                    }
                }
                if fence.is_some() {
                    continue;
                }
                let level = trimmed.bytes().take_while(|&b| b == b'#').count();
                if (1..=6).contains(&level)
                    && trimmed
                        .as_bytes()
                        .get(level)
                        .map(|b| b.is_ascii_whitespace())
                        .unwrap_or(false)
                {
                    item = Some(
                        json!({"kind":"heading","level":level,"name":trimmed[level..].trim()}),
                    );
                } else if index > 0
                    && !trimmed.is_empty()
                    && (trimmed.bytes().all(|b| b == b'=') || trimmed.bytes().all(|b| b == b'-'))
                {
                    let previous = previous.trim();
                    if !previous.is_empty() {
                        item = Some(
                            json!({"kind":"heading","level":if trimmed.starts_with('=') {1} else {2},"name":previous}),
                        );
                    }
                }
            } else if let Some(capture) = declaration.captures(trimmed) {
                item = Some(json!({"kind":&capture[1],"name":&capture[2]}));
            }
            let Some(mut item) = item else {
                continue;
            };
            if let Some(name) = item["name"].as_str().filter(|name| name.len() > 512) {
                let mut end = 512;
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                let short = name[..end].to_owned();
                item["name"] = json!(short);
                item["name_truncated"] = json!(true);
            }
            if seen < offset {
                seen += 1;
                continue;
            }
            if rows.len() == limit {
                more = true;
                break 'files;
            }
            item["path"] = json!(
                path.strip_prefix(root)
                    .map_err(|_| "Ruta fuera del repositorio.")?
                    .to_string_lossy()
            );
            item["line"] = json!(if markdown && !trimmed.starts_with('#') {
                index
            } else {
                index + 1
            });
            if !push(&mut rows, item, &mut bytes) {
                more = true;
                break 'files;
            }
            seen += 1;
        }
    }
    Ok(json!({"next_offset":offset + rows.len(),"items":rows,"more":more,"heuristic":true}))
}

pub async fn git(
    root: &Path,
    args: &Value,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<Value, String> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let operation = args["operation"].as_str().ok_or("operation inválida.")?;
    let staged = args["staged"]
        .as_bool()
        .ok_or("staged debe ser booleano.")?;
    let offset = number(args, "offset", 1_000_000)?;
    let limit = number(args, "limit", 8192)?;
    if limit == 0 || (operation == "status" && limit > 100) {
        return Err("limit: 1–100 para status, 1–8192 para diff.".into());
    }
    let paths = strings(args, "paths", true)?;
    let mut command = tokio::process::Command::new("git");
    command.current_dir(root).args([
        "--no-pager",
        "--no-optional-locks",
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.quotePath=false",
        "-c",
        "protocol.allow=never",
    ]);
    match operation {
        "status" => {
            command.args([
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=normal",
                "--ignore-submodules=all",
            ]);
        }
        "diff" => {
            command.args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=all",
            ]);
            if staged {
                command.arg("--cached");
            }
        }
        _ => return Err("operation debe ser status o diff.".into()),
    }
    command.arg("--");
    if paths.is_empty() {
        command.arg(".");
    } else {
        command.args(paths);
    }
    command.env_clear();
    for key in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "PATHEXT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|_| "No se pudo iniciar Git.")?;
    #[cfg(windows)]
    let _job = super::ProcessJob::attach(child.id().ok_or("Git no tiene PID.")?)?;
    let mut stdout = child.stdout.take().ok_or("Git no abrió stdout.")?;
    let mut stderr = child.stderr.take().ok_or("Git no abrió stderr.")?;
    let work = async {
        let out = async {
            let mut data = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = stdout
                    .read(&mut chunk)
                    .await
                    .map_err(|_| "No se pudo leer Git.")?;
                if n == 0 {
                    break;
                }
                if data.len() + n > 1024 * 1024 {
                    return Err("La salida supera 1 MiB; restringí paths.".to_string());
                }
                data.extend_from_slice(&chunk[..n]);
            }
            Ok::<_, String>(data)
        };
        let err = async {
            let mut chunk = [0u8; 4096];
            while stderr
                .read(&mut chunk)
                .await
                .map_err(|_| "No se pudo leer stderr.")?
                != 0
            {}
            Ok::<_, String>(())
        };
        let (output, _) = tokio::try_join!(out, err)?;
        if !child
            .wait()
            .await
            .map_err(|_| "No se pudo esperar a Git.")?
            .success()
        {
            return Err(
                "Git no pudo completar la lectura. Verificá que el proyecto sea un repositorio."
                    .into(),
            );
        }
        Ok::<_, String>(output)
    };
    let data = tokio::select! {
        _ = crate::codex::cancelled(&mut cancel) => return Err("Lectura Git cancelada.".into()),
        result = tokio::time::timeout(std::time::Duration::from_secs(30), work) => result.map_err(|_| "Git superó 30 segundos.")??,
    };
    if operation == "diff" {
        if offset > data.len() {
            return Err("offset supera la longitud del diff.".into());
        }
        let text = std::str::from_utf8(&data).map_err(|_| "El diff no es UTF-8 válido.")?;
        if !text.is_char_boundary(offset) {
            return Err("offset debe estar en un límite UTF-8.".into());
        }
        let mut end = (offset + limit.min(OUTPUT)).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        while end > offset
            && json!({"diff":&text[offset..end],"next_offset":end,"more":end < text.len()})
                .to_string()
                .len()
                > 8192
        {
            end -= 1;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
        }
        if end == offset && end < text.len() {
            return Err("limit es demasiado pequeño para el siguiente carácter UTF-8.".into());
        }
        return Ok(json!({"diff":&text[offset..end],"next_offset":end,"more":end < text.len()}));
    }
    let mut records = data.split(|&b| b == 0).filter(|r| !r.is_empty());
    let mut branch = String::new();
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        let text = std::str::from_utf8(record).map_err(|_| "Git devolvió rutas no UTF-8.")?;
        if let Some(name) = text.strip_prefix("## ") {
            branch = name.to_owned();
            continue;
        }
        if record.len() < 4 {
            return Err("Estado Git inválido.".into());
        }
        let original = if record[..2].contains(&b'R') || record[..2].contains(&b'C') {
            Some(
                std::str::from_utf8(records.next().ok_or("Rename Git incompleto.")?)
                    .map_err(|_| "Ruta no UTF-8.")?,
            )
        } else {
            None
        };
        entries.push(json!({"path":&text[3..],"index":&text[..1],"worktree":&text[1..2],"original":original}));
    }
    let branch_truncated = branch.len() > 512;
    if branch_truncated {
        let mut end = 512;
        while !branch.is_char_boundary(end) {
            end -= 1;
        }
        branch.truncate(end);
    }
    let mut rows = Vec::new();
    let mut bytes = json!(branch).to_string().len() + 128;
    for row in entries.iter().skip(offset).take(limit) {
        if !push(&mut rows, row.clone(), &mut bytes) {
            if rows.is_empty() {
                push(
                    &mut rows,
                    json!({"path_omitted":true,"index":row["index"],"worktree":row["worktree"]}),
                    &mut bytes,
                );
            }
            break;
        }
    }
    let next = offset + rows.len();
    Ok(
        json!({"branch":branch,"branch_truncated":branch_truncated,"files":rows,"next_offset":next,"more":next < entries.len()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_literal_regex_exclusions_and_outline() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let private = temp.path().join("private");
        fs::create_dir_all(root.join("target")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("target/hidden.rs"), "needle").unwrap();
        fs::write(root.join("a.rs"), "pub fn needle() {}\nstruct Other;\n").unwrap();
        fs::write(root.join("binary"), b"needle\0").unwrap();
        let args =
            json!({"patterns":["needle"],"regex":false,"context_lines":0,"offset":0,"limit":10});
        let result = search(&root, &private, &[root.clone()], &args).unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result["matches"][0]["line"], 1);
        let args = json!({"patterns":["["],"regex":true,"context_lines":0,"offset":0,"limit":10});
        assert!(search(&root, &private, &[root.clone()], &args).is_err());
        fs::write(root.join("doc.md"), "# Top\n```md\n# Hidden\n```\n## End\n").unwrap();
        let result = outline(
            &root,
            &[root.join("doc.md"), root.join("a.rs")],
            &json!({"offset":0,"limit":10}),
        )
        .unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 4);
        assert_eq!(result["items"][1]["name"], "End");
    }
    #[tokio::test]
    async fn git_status_diff_pagination_and_read_only_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .expect("Git is required by this test");
        assert!(init.success());
        fs::write(root.join("file.txt"), "first\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "--", "file.txt"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        let index = fs::read(root.join(".git/index")).unwrap();
        let (_sender, cancel) = tokio::sync::watch::channel(false);
        let status = git(
            root,
            &json!({"operation":"status","paths":[],"staged":false,"offset":0,"limit":10}),
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(status["files"][0]["path"], "file.txt");
        assert_eq!(status["files"][0]["index"], "A");
        let args =
            json!({"operation":"diff","paths":["file.txt"],"staged":true,"offset":0,"limit":8192});
        let patch = git(root, &args, cancel.clone()).await.unwrap();
        assert!(patch["diff"].as_str().unwrap().contains("+first"));
        let mut small = args.clone();
        small["limit"] = json!(12);
        let first = git(root, &small, cancel.clone()).await.unwrap();
        assert_eq!(first["more"], true);
        small["offset"] = first["next_offset"].clone();
        let second = git(root, &small, cancel.clone()).await.unwrap();
        assert_eq!(
            format!(
                "{}{}",
                first["diff"].as_str().unwrap(),
                second["diff"].as_str().unwrap()
            ),
            &patch["diff"].as_str().unwrap()[..24]
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        let mut invalid = args;
        invalid["operation"] = json!("checkout");
        assert!(git(root, &invalid, cancel).await.is_err());
    }
    #[cfg(unix)]
    #[test]
    fn search_does_not_follow_private_or_broken_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let private = root.join("private");
        fs::create_dir_all(&private).unwrap();
        fs::write(private.join("secret"), "needle").unwrap();
        std::os::unix::fs::symlink(&private, root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("broken")).unwrap();
        let result = search(
            &root,
            &private,
            &[root.clone()],
            &json!({"patterns":["needle"],"regex":false,"context_lines":0,"offset":0,"limit":10}),
        )
        .unwrap();
        assert!(result["matches"].as_array().unwrap().is_empty());
    }
    #[test]
    fn search_pagination_and_oversized_match() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.as_path().join("a"), "hit\nhit\nhit\n").unwrap();
        let mut args =
            json!({"patterns":["hit"],"regex":false,"context_lines":0,"offset":0,"limit":1});
        let result = search(
            root.as_path(),
            &root.as_path().join("private"),
            &[root.as_path().to_owned()],
            &args,
        )
        .unwrap();
        assert_eq!(result["next_offset"], 1);
        assert_eq!(result["more"], true);
        args["offset"] = json!(2);
        assert_eq!(
            search(
                root.as_path(),
                &root.as_path().join("private"),
                &[root.as_path().to_owned()],
                &args
            )
            .unwrap()["more"],
            false
        );
        fs::write(root.as_path().join("a"), "hit".repeat(4000)).unwrap();
        args["offset"] = json!(0);
        let result = search(
            root.as_path(),
            &root.as_path().join("private"),
            &[root.as_path().to_owned()],
            &args,
        )
        .unwrap();
        assert_eq!(result["matches"][0]["text_omitted"], true);
        assert!(result.to_string().len() <= 8192);
    }
}
