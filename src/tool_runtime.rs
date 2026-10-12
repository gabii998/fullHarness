use crate::codex::cancelled;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::watch};

const INSPECTION: usize = 8192;
const CAPTURE: u64 = 1024 * 1024;
pub struct Tools {
    root: PathBuf,
    artifacts: PathBuf,
    private: PathBuf,
    approved_http: std::sync::Mutex<Option<(String, String, crate::http_client::Prepared)>>,
}

pub fn schemas() -> Value {
    let definitions = [
        (
            "list_files",
            "List one repository directory. Bounded names only; no recursive scan.",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
        (
            "register_image",
            "Register a PNG or JPEG image from the repository in the current chat gallery. The image stays on disk; returns status/ref only. Maximum 16 MiB and 16 megapixels.",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
        (
            "read_file",
            "Explicit bounded UTF-8 file inspection. Maximum 8192 bytes; offset is in bytes.",
            json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":8192}}),
            vec!["path", "offset", "limit"],
        ),
        (
            "write_file",
            "Write a repository file after user approval. Returns status and artifact reference, never content. Parent directory must exist; maximum 65536 bytes.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            vec!["path", "content"],
        ),
        (
            "run_command",
            "Execute a program with an argument array in the repository after user approval. No shell is added. stdout/stderr stay in artifacts; returns only status/ref. 120s maximum.",
            json!({"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"timeout_seconds":{"type":"integer","minimum":1,"maximum":120}}),
            vec!["program", "args", "timeout_seconds"],
        ),
        (
            "http_collections",
            "Discover exposed HTTP collections in this project. Empty collection lists collections; a collection ID lists requests. Paginated metadata only, never variable values.",
            json!({"collection":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":20}}),
            vec!["collection", "offset", "limit"],
        ),
        (
            "http_request",
            "Execute one saved request from an exposed HTTP collection after approval. Returns status/ref only. Body and headers stay in artifacts; inspect_result reads bounded stdout/metadata explicitly.",
            json!({"collection":{"type":"string"},"request":{"type":"string"}}),
            vec!["collection", "request"],
        ),
        (
            "inspect_result",
            "Explicitly read a bounded part of stdout, stderr or metadata for a returned artifact reference; maximum 8192 bytes.",
            json!({"reference":{"type":"string"},"stream":{"type":"string","enum":["stdout","stderr","metadata"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":8192}}),
            vec!["reference", "stream", "offset", "limit"],
        ),
    ];
    json!([{"type":"namespace","name":"harness","description":"Local repository tools executed exclusively by Ascurra Harness.","tools": definitions.into_iter().map(|(name, description, properties, required)| json!({"type":"function","name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"strict":true})).collect::<Vec<_>>()}])
}

impl Tools {
    pub fn data_root(&self) -> &Path {
        &self.private
    }
    pub fn new(root: &Path, data: &Path, chat: u64) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|_| "El directorio del proyecto ya no existe.")?;
        let private = data
            .canonicalize()
            .map_err(|_| "No se pudo resolver el directorio de datos.")?;
        let artifacts = private.join("artifacts").join(chat.to_string());
        fs::create_dir_all(&artifacts)
            .map_err(|_| "No se pudo crear el directorio de resultados.")?;
        Ok(Self {
            root,
            artifacts,
            private,
            approved_http: std::sync::Mutex::new(None),
        })
    }

    fn path(&self, value: &str, writing: bool) -> Result<PathBuf, String> {
        let relative = Path::new(value);
        if relative.is_absolute()
            || relative.components().any(|p| {
                matches!(
                    p,
                    Component::ParentDir | Component::Prefix(_) | Component::RootDir
                )
            })
        {
            return Err("La ruta debe permanecer dentro del repositorio.".into());
        }
        let joined = self.root.join(relative);
        let resolved = if writing && !joined.exists() {
            let parent = joined
                .parent()
                .ok_or("Ruta de archivo inválida.")?
                .canonicalize()
                .map_err(|_| "El directorio padre debe existir.")?;
            parent.join(joined.file_name().ok_or("Nombre de archivo inválido.")?)
        } else {
            joined
                .canonicalize()
                .map_err(|_| "El archivo o directorio no existe.")?
        };
        if !resolved.starts_with(&self.root) || resolved.starts_with(&self.private) {
            return Err(
                "La ruta sale del repositorio o apunta a los datos privados del harness.".into(),
            );
        }
        // Windows streams and device names aren't ordinary repository files.
        #[cfg(windows)]
        for part in relative.components() {
            if let Component::Normal(name) = part {
                let name = name.to_string_lossy();
                let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                if name.contains(':')
                    || name.ends_with([' ', '.'])
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && stem.as_bytes()[3].is_ascii_digit())
                {
                    return Err("Nombre de archivo no admitido.".into());
                }
            }
        }
        Ok(resolved)
    }

    pub fn requires_approval(name: &str) -> bool {
        matches!(name, "write_file" | "run_command" | "http_request")
    }

    pub fn approval(&self, name: &str, arguments: &Value) -> Result<String, String> {
        match name {
            "http_request" => {
                let collection = field(arguments, "collection")?;
                let request = field(arguments, "request")?;
                let config = crate::http_client::load(&self.private, &self.root)?;
                let prepared = crate::http_client::prepare(&config, collection, request, true)?;
                let description = format!(
                    "Request HTTP guardada: {}\n{} {}{}\nBody: {} bytes · timeout: {} s\nSe ejecutará la versión guardada al solicitar esta aprobación.",
                    prepared.name,
                    prepared.method,
                    prepared.url.origin().ascii_serialization(),
                    prepared.url.path(),
                    prepared.body.len(),
                    prepared.timeout_seconds
                );
                *self.approved_http.lock().unwrap() =
                    Some((collection.into(), request.into(), prepared));
                Ok(description)
            }
            "write_file" => {
                let path = self.path(field(arguments, "path")?, true)?;
                let content = field(arguments, "content")?;
                if content.len() > 65536 {
                    return Err("La escritura supera 65536 bytes.".into());
                }
                Ok(format!(
                    "Escribir {}\n\nContenido nuevo completo:\n{}",
                    path.display(),
                    content
                ))
            }
            "run_command" => {
                let (program, args, timeout) = command_arguments(arguments)?;
                let mut description = format!(
                    "Directorio: {}\nPrograma: {}\nTimeout: {} s\nArgumentos: {}",
                    self.root.display(),
                    program,
                    timeout,
                    args.len()
                );
                // Display decoded argv verbatim: JSON serialization escapes script newlines.
                // Do not replace literal backslash sequences within executable arguments.
                for (index, argument) in args.iter().enumerate() {
                    description.push_str(&format!(
                        "\n\nArgumento {} ({} bytes):\n{}",
                        index + 1,
                        argument.len(),
                        argument
                    ));
                }
                description.push_str("\n\nEl comando aprobado puede acceder a recursos fuera del repositorio; todavía no hay aislamiento del sistema operativo.");
                Ok(description)
            }
            _ => Err("Esta herramienta no requiere aprobación.".into()),
        }
    }

    pub fn denied(&self, name: &str) -> String {
        self.failure(name, "Acción rechazada por el usuario.")
    }
    fn failure(&self, name: &str, error: &str) -> String {
        self.result(name, json!({"ok":false,"error":error}), false)
            .unwrap_or_else(|_| "{\"ok\":false}".into())
    }

    fn result(&self, name: &str, metadata: Value, ok: bool) -> Result<String, String> {
        let reference = uuid::Uuid::new_v4().simple().to_string();
        let directory = self.artifacts.join(&reference);
        fs::create_dir(&directory).map_err(|_| "No se pudo crear el resultado.")?;
        fs::write(
            directory.join("metadata.json"),
            serde_json::to_vec(&json!({"tool":name,"result":metadata})).unwrap(),
        )
        .map_err(|_| "No se pudo guardar el resultado.")?;
        Ok(json!({"ok":ok,"ref":reference}).to_string())
    }

    pub async fn execute(
        &self,
        name: &str,
        arguments: &Value,
        cancel: watch::Receiver<bool>,
    ) -> String {
        let result = match name {
            "http_collections" => crate::http_client::load(&self.private, &self.root)
                .and_then(|config| {
                    crate::http_client::discover(
                        &config,
                        field(arguments, "collection")?,
                        integer(arguments, "offset")?,
                        integer(arguments, "limit")?,
                    )
                })
                .map(|v| v.to_string()),
            "http_request" => self.http_request(arguments, cancel).await,
            "read_file" => self.read(arguments).map(|v| v.to_string()),
            "list_files" => self.list(arguments).map(|v| v.to_string()),
            "inspect_result" => self.inspect(arguments).map(|v| v.to_string()),
            "write_file" => self.write(arguments),
            "run_command" => self.run(arguments, cancel).await,
            "register_image" | "register_capture" => {
                let path = self.path(field(arguments, "path").unwrap_or(""), false);
                path.and_then(|path| {
                    let reference = uuid::Uuid::new_v4().simple().to_string();
                    let directory = self.artifacts.join(&reference);
                    let capture = crate::captures::import(&path, &directory)?;
                    fs::write(directory.join("metadata.json"), serde_json::to_vec(&json!({
                        "tool":"register_image", "file":capture.file_name().and_then(|s|s.to_str())
                    })).unwrap()).map_err(|_| "No se pudieron guardar los metadatos de la captura.")?;
                    Ok(json!({"ok":true,"ref":reference}).to_string())
                })
            }
            _ => Err("Herramienta desconocida.".into()),
        };
        result.unwrap_or_else(|error| self.failure(name, &error))
    }
    async fn http_request(
        &self,
        arguments: &Value,
        cancel: watch::Receiver<bool>,
    ) -> Result<String, String> {
        let collection = field(arguments, "collection")?;
        let request = field(arguments, "request")?;
        let prepared = self
            .approved_http
            .lock()
            .unwrap()
            .take()
            .filter(|(c, r, _)| c == collection && r == request)
            .map(|(_, _, p)| p)
            .ok_or("La request HTTP necesita aprobación previa.")?;
        let current = crate::http_client::load(&self.private, &self.root)?;
        if !current
            .collections
            .iter()
            .any(|c| c.id == collection && c.exposed && c.requests.iter().any(|r| r.id == request))
        {
            return Err("La colección o request dejó de estar expuesta al agente.".into());
        }
        let response = crate::http_client::send(prepared, cancel).await?;
        let reference = uuid::Uuid::new_v4().simple().to_string();
        crate::http_client::retain(&self.artifacts.join(&reference), &response)?;
        Ok(json!({"ok":(200..300).contains(&response.status),"ref":reference,"status":response.status,"bytes":response.body.len(),"truncated":response.truncated}).to_string())
    }

    fn read(&self, arguments: &Value) -> Result<Value, String> {
        let path = self.path(field(arguments, "path")?, false)?;
        inspect_file(&path, arguments)
    }
    fn list(&self, arguments: &Value) -> Result<Value, String> {
        let path = self.path(field(arguments, "path")?, false)?;
        let mut entries = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        for entry in fs::read_dir(path).map_err(|_| "No se pudo listar el directorio.")? {
            let entry = entry.map_err(|_| "No se pudo leer una entrada.")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), ".git" | "target" | "node_modules") {
                continue;
            }
            let row = json!({"name":name,"directory":entry.file_type().map_err(|_| "No se pudo leer el tipo de archivo.")?.is_dir()});
            let cost = row.to_string().len();
            if bytes + cost > INSPECTION || entries.len() >= 200 {
                truncated = true;
                break;
            }
            bytes += cost;
            entries.push(row);
        }
        entries.sort_by_key(|e| e["name"].as_str().unwrap_or("").to_string());
        Ok(json!({"entries":entries,"truncated":truncated}))
    }
    fn inspect(&self, arguments: &Value) -> Result<Value, String> {
        let reference = field(arguments, "reference")?;
        if reference.len() != 32 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Referencia de resultado inválida.".into());
        }
        let filename = match field(arguments, "stream")? {
            "stdout" => "stdout.txt",
            "stderr" => "stderr.txt",
            "metadata" => "metadata.json",
            _ => return Err("Salida desconocida.".into()),
        };
        inspect_file(&self.artifacts.join(reference).join(filename), arguments)
    }
    fn write(&self, arguments: &Value) -> Result<String, String> {
        let path = self.path(field(arguments, "path")?, true)?;
        let content = field(arguments, "content")?;
        if content.len() > 65536 {
            return Err("La escritura supera el límite local.".into());
        }
        let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("Ruta inválida.")?)
            .map_err(|_| "No se pudo preparar el archivo.")?;
        if let Ok(metadata) = fs::metadata(&path) {
            file.as_file()
                .set_permissions(metadata.permissions())
                .map_err(|_| "No se pudieron conservar los permisos del archivo.")?;
        }
        file.write_all(content.as_bytes())
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| "No se pudo guardar el archivo.")?;
        // Resolve again immediately before publishing; ordinary symlink escapes fail closed.
        if self.path(field(arguments, "path")?, true)? != path {
            return Err("La ruta cambió durante la escritura.".into());
        }
        file.persist(&path)
            .map_err(|_| "No se pudo reemplazar el archivo.")?;
        self.result(
            "write_file",
            json!({"path":field(arguments,"path")?,"bytes":content.len()}),
            true,
        )
    }

    async fn run(
        &self,
        arguments: &Value,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<String, String> {
        let (program, args, timeout) = command_arguments(arguments)?;
        let reference = uuid::Uuid::new_v4().simple().to_string();
        let directory = self.artifacts.join(&reference);
        fs::create_dir(&directory).map_err(|_| "No se pudo crear la captura del comando.")?;
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // No inherited credential environment. Basic OS/toolchain environment only.
        command.env_clear();
        // Approved capture scripts can write images directly to this chat artifact.
        command.env("HARNESS_ARTIFACT_DIR", &directory);
        for key in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "HOME",
            "APPDATA",
            "LOCALAPPDATA",
            "PATHEXT",
            "COMSPEC",
            "CARGO_HOME",
            "RUSTUP_HOME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|_| "No se pudo iniciar el comando.")?;
        #[cfg(windows)]
        let job = ProcessJob::attach(child.id().ok_or("El comando no tiene PID.")?)?;
        let stdout = child.stdout.take().ok_or("No se pudo capturar stdout.")?;
        let stderr = child.stderr.take().ok_or("No se pudo capturar stderr.")?;
        let mut out = tokio::spawn(capture(stdout, directory.join("stdout.txt")));
        let mut err = tokio::spawn(capture(stderr, directory.join("stderr.txt")));
        let status = tokio::select! {
            _ = cancelled(&mut cancel) => None,
            result = tokio::time::timeout(Duration::from_secs(timeout), child.wait()) => match result { Ok(Ok(status)) => Some(status), _ => None },
        };
        let stopped = if *cancel.borrow() {
            "cancelled"
        } else {
            "timed_out"
        };
        if status.is_none() {
            #[cfg(windows)]
            if let Some(pid) = child.id() {
                let shell = PathBuf::from(
                    std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
                )
                .join("System32/taskkill.exe");
                let _ = tokio::process::Command::new(shell)
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x08000000)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            let _ = child.kill().await;
        }
        #[cfg(windows)]
        drop(job); // Close owned job: stop descendants, including on future cancellation.
        // Background descendants can keep pipe handles open; bound capture teardown too.
        let captures = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(&mut out, &mut err)
        })
        .await;
        if captures.is_err() {
            out.abort();
            err.abort();
        }
        let (stdout_truncated, stderr_truncated) = match captures {
            Ok((Ok(Ok(a)), Ok(Ok(b)))) => (a, b),
            _ => (true, true),
        };
        let ok = status.is_some_and(|s| s.success());
        fs::write(directory.join("metadata.json"), serde_json::to_vec(&json!({"program":program,"args":arguments["args"],"exit_code":status.and_then(|s| s.code()),"state":if status.is_some(){"finished"}else{stopped},"stdout_truncated":stdout_truncated,"stderr_truncated":stderr_truncated})).unwrap()).map_err(|_| "No se pudo guardar el resultado del comando.")?;
        Ok(if status.is_some() {
            json!({"ok":ok,"ref":reference})
        } else {
            json!({"state":stopped,"ref":reference})
        }
        .to_string())
    }
}

#[cfg(windows)]
pub(crate) struct ProcessJob(isize);
#[cfg(windows)]
impl ProcessJob {
    pub(crate) fn attach(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{
                JobObjects::*,
                Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
            },
        };
        // Kernel handles can be closed from any thread. Store the owned handle as
        // an integer so this guard can move with a Send future between workers.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err("No se pudo supervisar el proceso del comando.".into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                CloseHandle(handle);
                return Err("No se pudo configurar la cancelación del comando.".into());
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                CloseHandle(handle);
                return Err("No se pudo supervisar el proceso del comando.".into());
            }
            let attached = AssignProcessToJobObject(handle, process);
            CloseHandle(process);
            if attached == 0 {
                CloseHandle(handle);
                return Err("Windows no permitió supervisar este comando.".into());
            }
            Ok(Self(handle as isize))
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as *mut core::ffi::c_void);
        }
    }
}

fn field<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    arguments[name]
        .as_str()
        .ok_or_else(|| format!("Falta el campo {name}."))
}
fn integer(arguments: &Value, name: &str) -> Result<usize, String> {
    arguments[name]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("{name} debe ser un entero positivo."))
}
fn command_arguments(arguments: &Value) -> Result<(&str, Vec<&str>, u64), String> {
    let program = field(arguments, "program")?;
    let args = arguments["args"]
        .as_array()
        .ok_or("args debe ser una lista.")?
        .iter()
        .map(|arg| arg.as_str().ok_or("Los argumentos deben ser texto."))
        .collect::<Result<Vec<_>, _>>()?;
    let timeout = arguments["timeout_seconds"]
        .as_u64()
        .filter(|v| (1..=120).contains(v))
        .ok_or("Tiempo del comando inválido.")?;
    if program.is_empty()
        || program.len() > 4096
        || args.len() > 100
        || args.iter().map(|s| s.len()).sum::<usize>() > 65536
    {
        return Err("Comando demasiado grande o vacío.".into());
    }
    Ok((program, args, timeout))
}
fn inspect_file(path: &Path, arguments: &Value) -> Result<Value, String> {
    let offset = arguments["offset"].as_u64().ok_or("Offset inválido.")?;
    let limit = arguments["limit"]
        .as_u64()
        .filter(|v| (1..=INSPECTION as u64).contains(v))
        .ok_or("Límite de lectura inválido.")?;
    let mut file = fs::File::open(path).map_err(|_| "No se pudo abrir el archivo solicitado.")?;
    if !file
        .metadata()
        .map_err(|_| "No se pudo inspeccionar el archivo.")?
        .is_file()
    {
        return Err("La ruta no es un archivo regular.".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "No se pudo ubicar la lectura.")?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer el archivo.")?;
    let more = bytes.len() as u64 > limit;
    bytes.truncate(limit as usize);
    Ok(
        json!({"text":String::from_utf8_lossy(&bytes),"next_offset":offset+bytes.len() as u64,"more":more}),
    )
}
async fn capture(
    mut input: impl tokio::io::AsyncRead + Unpin,
    path: PathBuf,
) -> Result<bool, std::io::Error> {
    let mut file = fs::File::create(path)?;
    let mut stored = 0u64;
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let keep = (CAPTURE.saturating_sub(stored)).min(count as u64) as usize;
        file.write_all(&buffer[..keep])?;
        stored += keep as u64;
        truncated |= keep < count;
    }
    Ok(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn capture_tool_retains_images_outside_context_and_rejects_repository_escape() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let data = temp.path().join("data");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&data).unwrap();
        image::RgbaImage::from_pixel(20, 10, image::Rgba([10, 20, 30, 255]))
            .save(repo.join("screen.png"))
            .unwrap();
        let tools = Tools::new(&repo, &data, 7).unwrap();
        let (_, cancel) = watch::channel(false);
        let reply = tools
            .execute(
                "register_image",
                &json!({"path":"screen.png"}),
                cancel.clone(),
            )
            .await;
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply.as_object().unwrap().len(), 2);
        let (items, partial) = crate::captures::list(&data, &[7]);
        assert!(!partial);
        assert_eq!(items.len(), 1);
        assert!(items[0].path.starts_with(data.join("artifacts/7")));
        let escape = tools
            .execute("register_image", &json!({"path":"../screen.png"}), cancel)
            .await;
        assert_eq!(serde_json::from_str::<Value>(&escape).unwrap()["ok"], false);
    }
    #[test]
    fn command_approval_displays_decoded_arguments_without_changing_script_literals() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let tools = Tools::new(&repo, temp.path(), 1).unwrap();
        let script = "# descripción\nprint('texto \\n literal')\nsave(\"README.md\")";
        let description = tools
            .approval(
                "run_command",
                &json!({
                    "program": "python", "args": ["-c", script, ""], "timeout_seconds": 30
                }),
            )
            .unwrap();
        assert!(description.contains("Programa: python\nTimeout: 30 s\nArgumentos: 3"));
        assert!(description.contains(&format!(
            "Argumento 2 ({} bytes):\n{}",
            script.len(),
            script
        )));
        assert!(description.contains("Argumento 3 (0 bytes):\n"));
        assert!(!description.contains("\\nsave"));
        assert!(description.contains("texto \\n literal"));
    }
    #[tokio::test]
    async fn http_tools_require_exposure_and_freeze_approval_with_bounded_replies() {
        use crate::http_client::{Collection, Config, Request};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let data = temp.path().join("data");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&data).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = Config::default();
        config
            .variables
            .insert("TOKEN".into(), "private-value".into());
        config.collections.push(Collection {
            id: "c".into(),
            name: "Local API".into(),
            exposed: false,
            requests: vec![Request {
                id: "r".into(),
                url: format!("http://{address}/approved"),
                headers: "Authorization: Bearer {{TOKEN}}".into(),
                ..Default::default()
            }],
        });
        crate::http_client::save(&data, &repo, &config).unwrap();
        let tools = Tools::new(&repo, &data, 1).unwrap();
        let args = json!({"collection":"c","request":"r"});
        assert!(Tools::requires_approval("http_request"));
        assert!(tools.approval("http_request", &args).is_err());
        config.collections[0].exposed = true;
        crate::http_client::save(&data, &repo, &config).unwrap();
        let (tx, rx) = watch::channel(false);
        let listing = tools
            .execute(
                "http_collections",
                &json!({"collection":"","offset":0,"limit":20}),
                rx.clone(),
            )
            .await;
        assert!(listing.contains("Local API"));
        assert!(!listing.contains("private-value"));
        assert!(
            !tools
                .approval("http_request", &args)
                .unwrap()
                .contains("private-value")
        );
        config.collections[0].requests[0].url = "http://127.0.0.1:1/not-approved".into();
        crate::http_client::save(&data, &repo, &config).unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(s.read_u8().await.unwrap());
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("GET /approved HTTP/1.1"));
            assert!(header.contains("Bearer private-value"));
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nanswer")
                .await
                .unwrap();
        });
        let reply = tools.execute("http_request", &args, rx.clone()).await;
        server.await.unwrap();
        assert!(reply.len() < 256);
        assert!(!reply.contains("answer"));
        assert!(!reply.contains("private-value"));
        let value: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(value["status"], 200);
        let body = tools
            .execute(
                "inspect_result",
                &json!({"reference":value["ref"],"stream":"stdout","offset":0,"limit":8192}),
                rx.clone(),
            )
            .await;
        assert!(body.contains("answer"));
        let again: Value =
            serde_json::from_str(&tools.execute("http_request", &args, rx).await).unwrap();
        assert_eq!(again["ok"], false);
        drop(tx);
    }
    #[tokio::test]
    async fn tool_artifacts_stay_out_of_action_replies_and_inspection_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let data = temp.path().join("data");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&data).unwrap();
        let tools = Tools::new(&repo, &data, 1).unwrap();
        let (_, cancel) = watch::channel(false);
        let output = tools
            .execute(
                "write_file",
                &json!({"path":"file.txt","content":"private-result"}),
                cancel.clone(),
            )
            .await;
        assert!(!output.contains("private-result"));
        assert!(
            serde_json::from_str::<Value>(&output).unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        let read = tools
            .execute(
                "read_file",
                &json!({"path":"file.txt","offset":0,"limit":4}),
                cancel.clone(),
            )
            .await;
        assert_eq!(
            serde_json::from_str::<Value>(&read).unwrap()["text"],
            "priv"
        );
        assert!(tools.path("../data/codex-auth.enc", false).is_err());
        assert!(tools.path("../escape.txt", true).is_err());
        assert!(
            tools
                .inspect(&json!({"reference":"../data","stream":"metadata","offset":0,"limit":4}))
                .is_err()
        );
        assert!(Tools::requires_approval("run_command"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn command_output_is_private_and_cancellation_stops_the_owned_job() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let data = temp.path().join("data");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&data).unwrap();
        let tools = Tools::new(&repo, &data, 1).unwrap();
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let (_keepalive, receiver) = watch::channel(false);
        let arguments = json!({"program":shell,"args":["-NoProfile","-NonInteractive","-Command","Write-Output 'private command fixture'; exit 0"],"timeout_seconds":15});
        let result: Value =
            serde_json::from_str(&tools.execute("run_command", &arguments, receiver).await)
                .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result.as_object().unwrap().len(), 2);
        assert!(!result.to_string().contains("private command fixture"));
        let inspected = tools
            .inspect(&json!({"reference":result["ref"],"stream":"stdout","offset":0,"limit":8192}))
            .unwrap();
        assert!(
            inspected["text"]
                .as_str()
                .unwrap()
                .contains("private command fixture")
        );
        let (sender, receiver) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            sender.send(true).unwrap();
        });
        let arguments = json!({"program":shell,"args":["-NoProfile","-NonInteractive","-Command","Start-Sleep -Seconds 30"],"timeout_seconds":60});
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            tools.execute("run_command", &arguments, receiver),
        )
        .await
        .unwrap();
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["state"], "cancelled");
    }
}
