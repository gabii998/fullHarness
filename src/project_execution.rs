//! Project executions shared by the UI and agent tools. No automatic execution on discovery.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    #[serde(default = "dot")]
    pub directory: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
fn dot() -> String {
    ".".into()
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub id: String,
    pub configuration: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub elapsed_seconds: u64,
    pub error: Option<String>,
}
struct Run {
    root: PathBuf,
    status: Mutex<Status>,
    start: Instant,
    stop: AtomicBool,
    logs: PathBuf,
}
#[derive(Default)]
pub struct Manager {
    runs: Mutex<HashMap<String, Arc<Run>>>,
}
impl Manager {
    pub fn shared() -> Arc<Self> {
        static INSTANCE: OnceLock<Arc<Manager>> = OnceLock::new();
        INSTANCE.get_or_init(|| Arc::new(Self::default())).clone()
    }
    pub fn statuses(&self, root: &Path) -> Result<Vec<Status>, String> {
        let root = canonical(root)?;
        let mut result = self
            .runs
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.root == root)
            .map(|r| {
                let mut s = r.status.lock().unwrap().clone();
                if s.state == "running" || s.state == "stopping" {
                    s.elapsed_seconds = r.start.elapsed().as_secs();
                }
                s
            })
            .collect::<Vec<_>>();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
    pub fn start(
        &self,
        root: &Path,
        data: &Path,
        configuration: Configuration,
    ) -> Result<String, String> {
        let root = canonical(root)?;
        let cwd = validate(&root, &configuration)?;
        let mut runs = self.runs.lock().unwrap();
        if runs.values().any(|r| {
            let s = r.status.lock().unwrap();
            r.root == root
                && s.configuration == configuration.id
                && matches!(s.state.as_str(), "running" | "stopping")
        }) {
            return Err("Esta configuración ya está ejecutándose.".into());
        }
        if runs
            .values()
            .filter(|r| {
                matches!(
                    r.status.lock().unwrap().state.as_str(),
                    "running" | "stopping"
                )
            })
            .count()
            >= 8
        {
            return Err("Máximo de ocho ejecuciones simultáneas.".into());
        }
        // Keep at most 64 completed executions in memory; their disk logs remain available.
        if runs.len() >= 64 {
            if let Some(id) = runs
                .iter()
                .filter(|(_, r)| {
                    !matches!(
                        r.status.lock().unwrap().state.as_str(),
                        "running" | "stopping"
                    )
                })
                .min_by_key(|(_, r)| r.start)
                .map(|(id, _)| id.clone())
            {
                runs.remove(&id);
            }
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let logs = data.join("project-runs").join(&id);
        fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        let program = if configuration.program == "adb" {
            crate::logcat::detect_adb().unwrap_or_else(|| PathBuf::from("adb"))
        } else {
            PathBuf::from(&configuration.program)
        };
        let mut command = Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
            .args(&configuration.args)
            .current_dir(cwd)
            .envs(&configuration.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|e| {
            format!(
                "No se pudo iniciar {}: {e}. Verificá Node/Java/SDK y PATH.",
                configuration.program
            )
        })?;
        #[cfg(windows)]
        let mut job = Some(match crate::tool_runtime::ProcessJob::attach(child.id()) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        });
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let run = Arc::new(Run {
            root,
            logs: logs.clone(),
            status: Mutex::new(Status {
                id: id.clone(),
                configuration: configuration.id,
                state: "running".into(),
                exit_code: None,
                elapsed_seconds: 0,
                error: None,
            }),
            start: Instant::now(),
            stop: AtomicBool::new(false),
        });
        runs.insert(id.clone(), run.clone());
        std::thread::spawn(move || {
            let out_path = logs.join("stdout.txt");
            let err_path = logs.join("stderr.txt");
            let out = std::thread::spawn(move || {
                crate::tools::capture_output(stdout, &out_path, 1024 * 1024)
            });
            let err = std::thread::spawn(move || {
                crate::tools::capture_output(stderr, &err_path, 1024 * 1024)
            });
            let outcome = loop {
                if run.stop.load(Ordering::Relaxed) {
                    #[cfg(unix)]
                    {
                        let _ = Command::new("kill")
                            .args(["-KILL", "--", &format!("-{}", child.id())])
                            .status();
                    }
                    #[cfg(windows)]
                    {
                        drop(job.take());
                    }
                    let _ = child.kill();
                    break child.wait();
                }
                match child.try_wait() {
                    Ok(Some(s)) => break Ok(s),
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(e);
                    }
                }
            };
            // Descendants are owned by the execution even when the parent exits first.
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .status();
            }
            #[cfg(windows)]
            {
                drop(job.take());
            }
            let out = out.join();
            let err = err.join();
            let mut status = run.status.lock().unwrap();
            status.elapsed_seconds = run.start.elapsed().as_secs();
            match outcome {
                Ok(s) => {
                    status.exit_code = s.code();
                    status.state = if run.stop.load(Ordering::Relaxed) {
                        "stopped"
                    } else if s.success() {
                        "finished"
                    } else {
                        "failed"
                    }
                    .into();
                }
                Err(e) => {
                    status.state = "failed".into();
                    status.error = Some(e.to_string());
                }
            }
            let truncated_out = out.ok().and_then(Result::ok);
            let truncated_err = err.ok().and_then(Result::ok);
            let _ = fs::write(logs.join("metadata.json"), serde_json::to_vec(&serde_json::json!({"status":*status,"stdout_truncated":truncated_out,"stderr_truncated":truncated_err})).unwrap());
        });
        Ok(id)
    }
    pub fn stop(&self, root: &Path, id: &str) -> Result<(), String> {
        let run = self.find(root, id)?;
        let mut s = run.status.lock().unwrap();
        if s.state == "running" {
            s.state = "stopping".into();
            run.stop.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
    pub fn shutdown(&self) {
        for r in self.runs.lock().unwrap().values() {
            r.stop.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !self.runs.lock().unwrap().values().any(|r| {
                matches!(
                    r.status.lock().unwrap().state.as_str(),
                    "running" | "stopping"
                )
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn find(&self, root: &Path, id: &str) -> Result<Arc<Run>, String> {
        let root = canonical(root)?;
        self.runs
            .lock()
            .unwrap()
            .get(id)
            .filter(|r| r.root == root)
            .cloned()
            .ok_or("Ejecución desconocida para este proyecto.".into())
    }
    pub fn tail(&self, root: &Path, id: &str, stream: &str) -> Result<String, String> {
        let run = self.find(root, id)?;
        let file = match stream {
            "stdout" => "stdout.txt",
            "stderr" => "stderr.txt",
            _ => return Err("Flujo desconocido.".into()),
        };
        let path = run.logs.join(file);
        if !path.exists() {
            return Ok(String::new());
        }
        let length = fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let v = self.logs(root, id, stream, length.saturating_sub(8192), 8192)?;
        let mut text = v["text"].as_str().unwrap_or("").to_string();
        if length >= 1024 * 1024 {
            text.push_str(
                "\n[Captura limitada a 1 MiB; la salida adicional se drena sin guardarse.]",
            );
        }
        Ok(text)
    }
    pub fn logs(
        &self,
        root: &Path,
        id: &str,
        stream: &str,
        offset: u64,
        limit: usize,
    ) -> Result<serde_json::Value, String> {
        if !(1..=8192).contains(&limit) {
            return Err("Límite de lectura: 1–8192 bytes.".into());
        }
        let run = self.find(root, id)?;
        let file = match stream {
            "stdout" => "stdout.txt",
            "stderr" => "stderr.txt",
            "metadata" => "metadata.json",
            _ => return Err("Flujo desconocido.".into()),
        };
        let path = run.logs.join(file);
        if !path.exists() {
            return Ok(serde_json::json!({"text":"","next_offset":offset,"more":false}));
        }
        let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
        let length = f.metadata().map_err(|e| e.to_string())?.len();
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        f.take(limit as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(
            serde_json::json!({"text":String::from_utf8_lossy(&bytes),"next_offset":offset + bytes.len() as u64,"more":offset + (bytes.len() as u64) < length}),
        )
    }
}
fn canonical(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize().map_err(|e| e.to_string())
}
pub fn validate(root: &Path, c: &Configuration) -> Result<PathBuf, String> {
    if c.id.is_empty()
        || c.id.len() > 128
        || c.name.is_empty()
        || c.name.len() > 256
        || c.program.is_empty()
        || c.args.len() > 128
        || serde_json::to_vec(c).unwrap().len() > 16384
    {
        return Err("Configuración inválida o demasiado grande.".into());
    }
    let p = Path::new(&c.directory);
    if p.is_absolute()
        || p.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("El directorio debe estar dentro del proyecto.".into());
    }
    let cwd = canonical(&root.join(p))?;
    if !cwd.starts_with(canonical(root)?) || !cwd.is_dir() {
        return Err("El directorio sale del proyecto.".into());
    }
    if c.env
        .keys()
        .any(|k| k.is_empty() || k.contains(['=', '\0']))
        || c.env.values().any(|v| v.contains('\0'))
        || c.program.contains('\0')
        || c.args.iter().any(|a| a.contains('\0'))
    {
        return Err("Argumentos o variables inválidos.".into());
    }
    Ok(cwd)
}
fn config_path(data: &Path, root: &Path) -> Result<PathBuf, String> {
    let key = format!(
        "{:x}",
        Sha256::digest(canonical(root)?.to_string_lossy().as_bytes())
    );
    Ok(data.join("project-execution").join(format!("{key}.json")))
}
pub fn has_saved_configurations(data: &Path, root: &Path) -> bool {
    config_path(data, root).is_ok_and(|p| p.exists())
}
pub fn load(data: &Path, root: &Path) -> Result<Vec<Configuration>, String> {
    let path = config_path(data, root)?;
    if !path.exists() {
        return detect(root);
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("Configuraciones demasiado grandes.".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
pub fn save(data: &Path, root: &Path, configurations: &[Configuration]) -> Result<(), String> {
    if configurations.len() > 128 {
        return Err("Máximo de 128 configuraciones.".into());
    }
    let mut ids = std::collections::HashSet::new();
    for c in configurations {
        validate(root, c)?;
        if !ids.insert(&c.id) {
            return Err("ID de configuración duplicado.".into());
        }
    }
    let bytes = serde_json::to_vec_pretty(configurations).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("Configuraciones demasiado grandes.".into());
    }
    let path = config_path(data, root)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut tmp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    tmp.write_all(&bytes).map_err(|e| e.to_string())?;
    tmp.as_file().sync_all().map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
/// Only bounded, shallow manifest inspection. Build files are never evaluated.
pub fn detect(root: &Path) -> Result<Vec<Configuration>, String> {
    let root = canonical(root)?;
    let mut dirs = vec![root.clone()];
    let mut cursor = 0;
    while cursor < dirs.len() && dirs.len() < 100 {
        let parent = dirs[cursor].clone();
        cursor += 1;
        if parent.strip_prefix(&root).unwrap().components().count() >= 3 {
            continue;
        }
        if let Ok(entries) = fs::read_dir(parent) {
            let mut entries = entries.take(200).flatten().collect::<Vec<_>>();
            entries.sort_by_key(|e| e.file_name());
            for d in entries {
                let name = d.file_name();
                let name = name.to_string_lossy();
                if dirs.len() >= 100 {
                    break;
                }
                if !name.starts_with('.')
                    && !matches!(
                        name.as_ref(),
                        "node_modules" | "target" | "build" | "vendor" | "dist"
                    )
                    && d.file_type().is_ok_and(|t| t.is_dir())
                    && canonical(&d.path()).is_ok_and(|p| p.starts_with(&root))
                {
                    dirs.push(d.path());
                }
            }
        }
    }
    let mut result = Vec::new();
    for dir in dirs {
        if !canonical(&dir)?.starts_with(&root) {
            continue;
        }
        let relative = dir
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let directory = if relative.is_empty() { "." } else { &relative };
        let mut add = |key: &str, name: &str, program: &str, args: Vec<String>| {
            if result.len() < 128 {
                result.push(Configuration {
                    id: format!("{directory}:{key}"),
                    name: format!("{directory} · {name}"),
                    program: program.into(),
                    args,
                    directory: directory.into(),
                    env: BTreeMap::new(),
                });
            }
        };
        if let Some(package) = bounded_text(&dir.join("package.json"))
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            let manager = package["packageManager"]
                .as_str()
                .and_then(|s| s.split('@').next())
                .filter(|s| matches!(*s, "npm" | "pnpm" | "yarn"))
                .unwrap_or_else(|| {
                    if dir.join("pnpm-lock.yaml").exists() || root.join("pnpm-lock.yaml").exists() {
                        "pnpm"
                    } else if dir.join("yarn.lock").exists() || root.join("yarn.lock").exists() {
                        "yarn"
                    } else {
                        "npm"
                    }
                });
            let program = if cfg!(windows) {
                format!("{manager}.cmd")
            } else {
                manager.into()
            };
            if let Some(scripts) = package["scripts"].as_object() {
                for (key, value) in scripts.iter().take(64) {
                    if value.is_string() {
                        add(
                            &format!("node:{key}"),
                            &format!("Node · {key}"),
                            &program,
                            vec!["run".into(), key.clone()],
                        );
                    }
                }
            }
        }
        let gradle = bounded_text(&dir.join("build.gradle.kts"))
            .or_else(|| bounded_text(&dir.join("build.gradle")));
        let settings = bounded_text(&dir.join("settings.gradle.kts"))
            .or_else(|| bounded_text(&dir.join("settings.gradle")))
            .unwrap_or_default();
        let root_build = bounded_text(&root.join("build.gradle.kts"))
            .or_else(|| bounded_text(&root.join("build.gradle")))
            .unwrap_or_default();
        let wrapper_dir = dir
            .ancestors()
            .take_while(|p| p.starts_with(&root))
            .find(|p| p.join("gradlew").exists() || p.join("gradlew.bat").exists())
            .unwrap_or(&root);
        if let Some(build) = gradle {
            let wrapper = wrapper_dir.join(if cfg!(windows) {
                "gradlew.bat"
            } else {
                "gradlew"
            });
            if wrapper.exists() {
                let module = if wrapper_dir != dir.as_path() {
                    format!(
                        ":{}:",
                        dir.strip_prefix(wrapper_dir)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', ":")
                            .replace('/', ":")
                    )
                } else {
                    String::new()
                };
                let program = wrapper.to_string_lossy().into_owned();
                if build.contains("org.springframework.boot")
                    || (build.contains("spring") && root_build.contains("org.springframework.boot"))
                {
                    for task in ["bootRun", "test", "build"] {
                        add(
                            &format!("spring:{task}"),
                            &format!("Spring · {task}"),
                            &program,
                            vec![format!("{module}{task}")],
                        );
                    }
                }
                let android = build.contains("com.android.application")
                    || build.contains("android.application")
                    || (build.contains("android {")
                        && root_build.contains("com.android.application"));
                if android {
                    for task in [
                        "assembleDebug",
                        "installDebug",
                        "testDebugUnitTest",
                        "connectedDebugAndroidTest",
                    ] {
                        add(
                            &format!("android:{task}"),
                            &format!("Android · {task}"),
                            &program,
                            vec![format!("{module}{task}")],
                        );
                    }
                    add(
                        "android:devices",
                        "Android · dispositivos",
                        "adb",
                        vec!["devices".into(), "-l".into()],
                    );
                } else if !build.contains("org.springframework.boot")
                    && settings.is_empty()
                    && build.contains("kotlin")
                {
                    for task in ["test", "build"] {
                        add(
                            &format!("kotlin:{task}"),
                            &format!("Kotlin · {task}"),
                            &program,
                            vec![format!("{module}{task}")],
                        );
                    }
                }
            }
        }
        if let Some(pom) = bounded_text(&dir.join("pom.xml")) {
            let wrapper = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };
            if dir.join(wrapper).exists() {
                let program = dir.join(wrapper).to_string_lossy().into_owned();
                for task in if pom.contains("spring-boot") {
                    vec!["spring-boot:run", "test", "package"]
                } else {
                    vec!["test", "package"]
                } {
                    add(
                        &format!("maven:{task}"),
                        &format!("Maven · {task}"),
                        &program,
                        vec![task.into()],
                    );
                }
            }
        }
    }
    Ok(result)
}
fn bounded_text(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    if f.metadata().ok()?.len() > 256 * 1024 {
        return None;
    }
    let mut s = String::new();
    f.take(256 * 1024).read_to_string(&mut s).ok()?;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_without_executing_and_persists() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("package.json"),
            r#"{"packageManager":"pnpm@9","scripts":{"dev":"server","test":"tests"}}"#,
        )
        .unwrap();
        fs::write(root.path().join("gradlew"), "not executable").unwrap();
        fs::write(root.path().join("gradlew.bat"), "not executable").unwrap();
        fs::write(
            root.path().join("build.gradle.kts"),
            "plugins { id(\"org.springframework.boot\") }",
        )
        .unwrap();
        fs::create_dir(root.path().join("app")).unwrap();
        fs::write(
            root.path().join("app/build.gradle.kts"),
            "plugins { id(\"com.android.application\") }",
        )
        .unwrap();
        let c = detect(root.path()).unwrap();
        assert!(
            c.iter()
                .any(|c| c.args == ["run", "dev"] && c.program.starts_with("pnpm"))
        );
        assert!(c.iter().any(|c| c.args == ["bootRun"]));
        assert!(c.iter().any(|c| c.args == [":app:installDebug"]));
        save(data.path(), root.path(), &c).unwrap();
        assert_eq!(load(data.path(), root.path()).unwrap(), c);
        let mut bad = c[0].clone();
        bad.directory = "../".into();
        assert!(validate(root.path(), &bad).is_err());
        assert!(save(data.path(), root.path(), &[c[0].clone(), c[0].clone()]).is_err());
    }
    #[test]
    fn executions_are_shared_scoped_and_logs_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let m = Manager::shared();
        let c = Configuration {
            id: uuid::Uuid::new_v4().to_string(),
            name: "test".into(),
            program: if cfg!(windows) { "cmd" } else { "sh" }.into(),
            args: if cfg!(windows) {
                vec!["/C".into(), "echo hello".into()]
            } else {
                vec!["-c".into(), "echo hello".into()]
            },
            directory: dot(),
            env: BTreeMap::new(),
        };
        let id = m.start(root.path(), data.path(), c).unwrap();
        for _ in 0..100 {
            if m.statuses(root.path()).unwrap()[0].state != "running" {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(m.statuses(root.path()).unwrap()[0].state, "finished");
        assert!(
            m.logs(root.path(), &id, "stdout", 0, 8192).unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("hello")
        );
        assert!(m.stop(other.path(), &id).is_err());
        assert!(m.logs(root.path(), &id, "stdout", 0, 8193).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn prevents_duplicates_and_stops_long_running_jobs() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let manager = Manager::default();
        let c = Configuration {
            id: "long-job".into(),
            name: "Long job".into(),
            program: if cfg!(windows) {
                "powershell.exe"
            } else {
                "sh"
            }
            .into(),
            args: if cfg!(windows) {
                vec![
                    "-NoProfile".into(),
                    "-Command".into(),
                    "Write-Output ready; Start-Sleep -Seconds 30".into(),
                ]
            } else {
                vec!["-c".into(), "echo ready; sleep 30".into()]
            },
            directory: ".".into(),
            env: Default::default(),
        };
        let id = manager.start(root.path(), data.path(), c.clone()).unwrap();
        assert!(manager.start(root.path(), data.path(), c).is_err());
        assert_eq!(manager.statuses(root.path()).unwrap()[0].state, "running");
        manager.stop(root.path(), &id).unwrap();
        for _ in 0..100 {
            if manager.statuses(root.path()).unwrap()[0].state == "stopped" {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(manager.statuses(root.path()).unwrap()[0].state, "stopped");
    }
    #[test]
    fn detects_nested_node_android_and_maven_projects() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("packages/web")).unwrap();
        fs::write(root.path().join("yarn.lock"), "").unwrap();
        fs::write(
            root.path().join("packages/web/package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.path().join("mobile/app")).unwrap();
        fs::write(root.path().join("mobile/gradlew"), "").unwrap();
        fs::write(root.path().join("mobile/gradlew.bat"), "").unwrap();
        fs::write(
            root.path().join("mobile/app/build.gradle.kts"),
            "alias(libs.plugins.android.application)",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("server")).unwrap();
        fs::write(root.path().join("server/mvnw"), "").unwrap();
        fs::write(root.path().join("server/mvnw.cmd"), "").unwrap();
        fs::write(root.path().join("server/pom.xml"), "spring-boot").unwrap();
        let configs = detect(root.path()).unwrap();
        assert!(
            configs
                .iter()
                .any(|c| c.directory == "packages/web" && c.program.starts_with("yarn"))
        );
        assert!(
            configs
                .iter()
                .any(|c| c.program.contains("mobile") && c.args == [":app:assembleDebug"])
        );
        assert!(configs.iter().any(|c| c.args == ["spring-boot:run"]));
    }
}
