//! Project-scoped saved HTTP collections. Variables stay outside repository/model context.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::watch;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub variables: BTreeMap<String, String>,
    pub collections: Vec<Collection>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub exposed: bool,
    pub requests: Vec<Request>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    30
}
impl Default for Request {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: "Nueva request".into(),
            method: "GET".into(),
            url: "{{BASE_URL}}/".into(),
            headers: String::new(),
            body: String::new(),
            timeout_seconds: 30,
        }
    }
}
pub fn folder(data: &Path, project: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let key = Sha256::digest(project.to_string_lossy().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    data.join("http").join(key)
}
pub fn load(data: &Path, project: &Path) -> Result<Config, String> {
    let path = folder(data, project).join("collections.json");
    if !path.exists() {
        return Ok(Config::default());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "No se pudieron abrir las colecciones HTTP.")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudieron leer las colecciones HTTP.")?;
    if bytes.len() > 1024 * 1024 {
        return Err("Las colecciones superan 1 MiB.".into());
    }
    let config = serde_json::from_slice(&bytes)
        .map_err(|_| "El archivo de colecciones HTTP es inválido.")?;
    validate(&config)?;
    Ok(config)
}
pub fn validate(config: &Config) -> Result<(), String> {
    if config.collections.len() > 50 || config.variables.len() > 128 {
        return Err("Límite: 50 colecciones y 128 variables por proyecto.".into());
    }
    let mut ids = HashSet::new();
    let mut requests = 0;
    for (key, value) in &config.variables {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || value.len() > 8192
        {
            return Err("Nombre o tamaño de variable inválido.".into());
        }
    }
    for collection in &config.collections {
        if collection.name.trim().is_empty()
            || collection.name.len() > 160
            || !valid_id(&collection.id)
            || !ids.insert(collection.id.clone())
        {
            return Err("Nombre o ID de colección inválido.".into());
        }
        for r in &collection.requests {
            requests += 1;
            if requests > 500
                || !valid_id(&r.id)
                || !ids.insert(r.id.clone())
                || r.name.trim().is_empty()
                || r.name.len() > 160
                || r.url.len() > 8192
                || r.headers.len() > 16384
                || r.body.len() > 65536
                || !(1..=120).contains(&r.timeout_seconds)
                || !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
                    .contains(&r.method.as_str())
            {
                return Err(
                    "Request inválida o demasiado grande (máximo 500 requests, body 64 KiB)."
                        .into(),
                );
            }
        }
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub fn save(data: &Path, project: &Path, config: &Config) -> Result<(), String> {
    validate(config)?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|_| "No se pudo serializar HTTP.")?;
    if bytes.len() > 1024 * 1024 {
        return Err("Las colecciones superan 1 MiB.".into());
    }
    let folder = folder(data, project);
    fs::create_dir_all(&folder).map_err(|_| "No se pudo crear el directorio HTTP.")?;
    let mut temp = tempfile::NamedTempFile::new_in(&folder)
        .map_err(|_| "No se pudo preparar el guardado HTTP.")?;
    use std::io::Write;
    temp.write_all(&bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "No se pudo guardar HTTP.")?;
    temp.persist(folder.join("collections.json"))
        .map_err(|_| "No se pudieron reemplazar las colecciones HTTP.")?;
    Ok(())
}
fn expand(template: &str, variables: &BTreeMap<String, String>) -> Result<String, String> {
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest.find("}}").ok_or("Variable sin cierre }}.")?;
        let key = rest[..end].trim();
        result.push_str(
            variables
                .get(key)
                .ok_or_else(|| format!("Variable no definida: {key}"))?,
        );
        rest = &rest[end + 2..];
        if result.len() > 256 * 1024 {
            return Err("La request expandida supera el límite.".into());
        }
    }
    result.push_str(rest);
    if result.len() > 256 * 1024 {
        return Err("La request expandida supera el límite.".into());
    }
    Ok(result)
}
#[derive(Clone)]
pub struct Prepared {
    pub name: String,
    pub method: reqwest::Method,
    pub url: reqwest::Url,
    pub headers: reqwest::header::HeaderMap,
    pub body: String,
    pub timeout_seconds: u64,
}
pub fn prepare(
    config: &Config,
    collection: &str,
    request: &str,
    agent: bool,
) -> Result<Prepared, String> {
    validate(config)?;
    let collection = config
        .collections
        .iter()
        .find(|c| c.id == collection && (!agent || c.exposed))
        .ok_or("La colección no existe o no está expuesta al agente.")?;
    let r = collection
        .requests
        .iter()
        .find(|r| r.id == request)
        .ok_or("La request no pertenece a esta colección.")?;
    let url = reqwest::Url::parse(&expand(&r.url, &config.variables)?)
        .map_err(|_| "URL HTTP inválida.")?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Usá HTTP/HTTPS y configurá credenciales en headers, no en la URL.".into());
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for line in expand(&r.headers, &config.variables)?
        .lines()
        .filter(|s| !s.trim().is_empty())
    {
        let (key, value) = line
            .split_once(':')
            .ok_or("Cada header debe usar Nombre: valor.")?;
        let key = reqwest::header::HeaderName::from_bytes(key.trim().as_bytes())
            .map_err(|_| "Nombre de header inválido.")?;
        let value = reqwest::header::HeaderValue::from_str(value.trim())
            .map_err(|_| "Valor de header inválido.")?;
        headers.append(key, value);
    }
    let body = expand(&r.body, &config.variables)?;
    Ok(Prepared {
        name: format!("{} / {}", collection.name, r.name),
        method: reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|_| "Método inválido.")?,
        url,
        headers,
        body,
        timeout_seconds: r.timeout_seconds,
    })
}
pub fn exposed(config: &Config) -> Value {
    json!({"collections":config.collections.iter().filter(|c|c.exposed).map(|c|json!({"id":c.id,"name":c.name,"requests":c.requests.iter().map(|r|json!({"id":r.id,"name":r.name,"method":r.method})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"variable_names":config.variables.keys().collect::<Vec<_>>()})
}
pub fn discover(
    config: &Config,
    collection: &str,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    if !(1..=20).contains(&limit) {
        return Err("Usá un límite de 1 a 20.".into());
    }
    if collection.is_empty() {
        let rows = config
            .collections
            .iter()
            .filter(|c| c.exposed)
            .collect::<Vec<_>>();
        Ok(
            json!({"collections":rows.iter().skip(offset).take(limit).map(|c|json!({"id":c.id,"name":c.name,"requests":c.requests.len()})).collect::<Vec<_>>(),"more":offset.saturating_add(limit)<rows.len()}),
        )
    } else {
        let c = config
            .collections
            .iter()
            .find(|c| c.id == collection && c.exposed)
            .ok_or("La colección no está expuesta al agente.")?;
        Ok(
            json!({"collection":c.id,"requests":c.requests.iter().skip(offset).take(limit).map(|r|json!({"id":r.id,"name":r.name,"method":r.method})).collect::<Vec<_>>(),"more":offset.saturating_add(limit)<c.requests.len()}),
        )
    }
}
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub duration_ms: u128,
    pub headers: String,
    pub body: Vec<u8>,
    pub truncated: bool,
}
pub async fn send(
    request: Prepared,
    mut cancel: watch::Receiver<bool>,
) -> Result<Response, String> {
    if *cancel.borrow() {
        return Err("Cancelado.".into());
    }
    let started = Instant::now();
    let timeout = request.timeout_seconds;
    let operation = async move {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(timeout))
            .build()
            .map_err(|_| "No se pudo crear el cliente HTTP.")?;
        let response = client
            .request(request.method, request.url)
            .headers(request.headers)
            .body(request.body)
            .send()
            .await
            .map_err(|_| "No se pudo enviar la request: conexión, TLS o timeout.")?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .take(128)
            .map(|(k, v)| {
                format!(
                    "{}: {}",
                    k,
                    v.to_str()
                        .unwrap_or("[binario]")
                        .chars()
                        .take(1024)
                        .collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Falló la lectura de la respuesta HTTP.")?;
            let keep = chunk
                .len()
                .min((1024 * 1024usize).saturating_sub(body.len()));
            body.extend_from_slice(&chunk[..keep]);
            if keep < chunk.len() {
                truncated = true;
                break;
            }
        }
        Ok(Response {
            status,
            duration_ms: started.elapsed().as_millis(),
            headers,
            body,
            truncated,
        })
    };
    tokio::select! { result=tokio::time::timeout(Duration::from_secs(timeout),operation)=>result.map_err(|_|"La request agotó el tiempo límite.".to_string())?, _=crate::codex::cancelled(&mut cancel)=>Err("Cancelado.".into()) }
}
pub fn retain(folder: &Path, response: &Response) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|_| "No se pudo crear el resultado HTTP.")?;
    fs::write(folder.join("stdout.txt"), &response.body)
        .map_err(|_| "No se pudo guardar la respuesta HTTP.")?;
    fs::write(folder.join("metadata.json"),json!({"status":response.status,"duration_ms":response.duration_ms,"headers":response.headers,"body_bytes":response.body.len(),"truncated":response.truncated}).to_string()).map_err(|_|"No se pudieron guardar los metadatos HTTP.")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn collections_persist_scope_hide_variables_and_send_a_real_local_request() {
        let data = tempfile::tempdir().unwrap();
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = Config::default();
        config
            .variables
            .insert("BASE_URL".into(), format!("http://{address}"));
        config
            .variables
            .insert("TOKEN".into(), "private-token".into());
        config.collections.push(Collection {
            id: "c".into(),
            name: "API".into(),
            exposed: false,
            requests: vec![Request {
                id: "r".into(),
                method: "POST".into(),
                url: "{{BASE_URL}}/test".into(),
                headers: "X-Test: {{TOKEN}}".into(),
                body: "hello".into(),
                ..Default::default()
            }],
        });
        save(data.path(), one.path(), &config).unwrap();
        assert!(
            load(data.path(), two.path())
                .unwrap()
                .collections
                .is_empty()
        );
        assert!(
            exposed(&config)["collections"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(prepare(&config, "c", "r", true).is_err());
        config.collections[0].exposed = true;
        save(data.path(), one.path(), &config).unwrap();
        assert!(
            !exposed(&load(data.path(), one.path()).unwrap())
                .to_string()
                .contains("private-token")
        );
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let b = socket.read_u8().await.unwrap();
                bytes.push(b);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let mut body = [0u8; 5];
            socket.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"hello");
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.starts_with("POST /test HTTP/1.1"));
            assert!(text.to_lowercase().contains("x-test: private-token"));
            socket
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .await
                .unwrap();
        });
        let (_sender, cancel) = watch::channel(false);
        let response = send(prepare(&config, "c", "r", true).unwrap(), cancel)
            .await
            .unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.body, b"ok");
        server.await.unwrap();
        let folder = data.path().join("response");
        retain(&folder, &response).unwrap();
        assert_eq!(fs::read(folder.join("stdout.txt")).unwrap(), b"ok");
        config.variables.remove("BASE_URL");
        assert!(prepare(&config, "c", "r", true).is_err());
    }
}
