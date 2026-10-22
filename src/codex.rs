//! Direct Codex OAuth and Responses transport. No CLI or API-key fallback.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
#[cfg(test)]
use reqwest::Url;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, watch};
#[cfg(test)]
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const AUTH: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const MAX_JSON: usize = 2 * 1024 * 1024;
fn codex_routing(mut request: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    // Decode only routing hints; HTTPS bearer authorization validates the token.
    if let Some(claims) = token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        let auth = &claims["https://api.openai.com/auth"];
        for (header, value) in [
            ("ChatGPT-Account-ID", auth["chatgpt_account_id"].as_str()),
            (
                "x-openai-internal-codex-residency",
                auth["chatgpt_data_residency"]
                    .as_str()
                    .or(auth["chatgpt_compute_residency"].as_str()),
            ),
        ] {
            if let Some(value) = value.filter(|s| !s.is_empty()) {
                request = request.header(header, value);
            }
        }
    }
    request
}
#[path = "codex_quota.rs"]
mod quota_auth;

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    client_id: String,
    subject: String,
    email: String,
    access_token: String,
    refresh_token: String,
    id_token: String,
    expires_at: u64,
    scopes: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelChoice {
    pub slug: String,
    pub label: String,
    #[serde(default)]
    pub context_window: Option<u64>,
}

pub struct Connection {
    client: Client,
    resource: String,
    root: PathBuf,
    gate: Mutex<()>,
    // Serialize credential ownership across app processes, not just chats.
    _lock: fs::File,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    while cancel.changed().await.is_ok() {
        if *cancel.borrow() {
            return;
        }
    }
}

async fn bounded_json(mut response: Response) -> Result<Value, String> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Se interrumpió la respuesta de OpenAI.")?
    {
        if bytes.len() + chunk.len() > MAX_JSON {
            return Err("Respuesta de OpenAI demasiado grande.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        format!(
            "OpenAI devolvió una respuesta inválida (HTTP {}).",
            status.as_u16()
        )
    })?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), &value));
    }
    Ok(value)
}

// Retain only bounded error fields, never the complete provider event.
fn stream_error(event: &Value) -> (String, Value) {
    fn field(event: &Value, key: &str) -> String {
        event["error"][key]
            .as_str()
            .or_else(|| event[key].as_str())
            .unwrap_or("")
            .split_whitespace()
            .take(100)
            .map(|word| {
                if word.contains("://") {
                    "[URL omitida]"
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .take(500)
            .collect()
    }
    let message = field(event, "message");
    let code = field(event, "code");
    let kind = field(event, "type");
    let detail = json!({"message": message, "code": code, "type": kind});
    let suffix = if message.is_empty() {
        if code.is_empty() {
            String::new()
        } else {
            format!(": {code}")
        }
    } else {
        format!(": {message}")
    };
    (
        format!(
            "OpenAI informó un error durante el streaming{suffix}. No se reintentó la generación."
        ),
        detail,
    )
}

fn http_error(status: u16, value: &Value) -> String {
    let code = value["error"]["code"]
        .as_str()
        .or(value["error"].as_str())
        .unwrap_or("");
    match code {
        "subscription_sharing_usage_limit_exceeded" | "subscription_sharing_usage_unavailable" => "El plan no dispone de cuota para esta aplicación. Revisá ChatGPT → Settings → Usage. No se cambió a API de pago.".into(),
        "invalid_grant" => "La sesión de ChatGPT expiró o fue revocada. Volvé a iniciar sesión.".into(),
        _ => match status {
            401 => "ChatGPT rechazó la sesión. Volvé a iniciar sesión.".into(),
            403 => "ChatGPT no autorizó el modelo o el uso del plan para esta aplicación.".into(),
            429 => "OpenAI alcanzó un límite de uso o de solicitudes. Esperá antes de volver a enviar.".into(),
            _ => {
                let detail: String = value["error"]["message"].as_str()
                    .or_else(|| value["message"].as_str())
                    .unwrap_or("").chars().filter(|c| !c.is_control()).take(500).collect();
                if detail.is_empty() {
                    format!("La solicitud a OpenAI falló (HTTP {status}). No se reintentó la generación.")
                } else {
                    format!("La solicitud a OpenAI falló (HTTP {status}): {detail}. No se reintentó la generación.")
                }
            },
        }
    }
}

impl Connection {
    /// Read-only account quota, using this application's credential only.
    /// This backend route is also used by Hermes; SiWC grants may not authorize it.
    pub async fn account_usage(&self) -> Result<crate::quota::AccountUsage, String> {
        if !self.uses_native_auth()? {
            return Err(
                "Volvé a autorizar Codex para habilitar chats y cuota con un solo acceso.".into(),
            );
        }
        let token = self.access_token().await?;
        self.read_account_usage("https://chatgpt.com/backend-api/wham/usage", &token)
            .await
    }

    #[cfg(test)]
    async fn account_usage_at(&self, endpoint: &str) -> Result<crate::quota::AccountUsage, String> {
        let token = self.access_token().await?;
        self.read_account_usage(endpoint, &token).await
    }

    async fn read_account_usage(
        &self,
        endpoint: &str,
        token: &str,
    ) -> Result<crate::quota::AccountUsage, String> {
        let request = self
            .client
            .get(endpoint)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(20));
        let request = codex_routing(request, token);
        let response = request
            .send()
            .await
            .map_err(|_| "No se pudo consultar la cuota de Codex.")?;
        if !response.status().is_success() {
            return Err(format!(
                "Cuota no disponible con esta sesión (HTTP {}).",
                response.status().as_u16()
            ));
        }
        crate::quota::AccountUsage::parse(&bounded_json(response).await?)
    }
    pub fn new(root: &Path) -> Result<Arc<Self>, String> {
        fs::create_dir_all(root)
            .map_err(|_| "No se pudo preparar el almacenamiento de autenticación.")?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("codex-session.lock"))
            .map_err(|_| "No se pudo abrir la sesión del harness.")?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(
            |_| "Otra ventana del harness está usando esta sesión. Cerrala antes de abrir otra.",
        )?;
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("ascurra_harness/0.1")
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "No se pudo preparar el cliente HTTP.")?;
        Ok(Arc::new(Self {
            client,
            resource: RESOURCE.into(),
            root: root.into(),
            gate: Mutex::new(()),
            _lock: lock,
        }))
    }

    fn session(&self) -> Result<Option<Session>, String> {
        let path = self.root.join("codex-auth.enc");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path).map_err(|_| "No se pudo leer la sesión protegida.")?;
        if bytes.len() > MAX_JSON {
            return Err("El archivo de sesión no es válido.".into());
        }
        serde_json::from_slice(&unprotect(&bytes)?)
            .map(Some)
            .map_err(|_| "La sesión protegida no es válida.".into())
    }

    fn save(&self, session: &Session) -> Result<(), String> {
        let bytes = serde_json::to_vec(session).map_err(|_| "No se pudo preparar la sesión.")?;
        let protected = protect(&bytes)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|_| "No se pudo guardar la sesión.")?;
        file.write_all(&protected)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| "No se pudo guardar la sesión.")?;
        file.persist(self.root.join("codex-auth.enc"))
            .map_err(|_| "No se pudo reemplazar la sesión.")?;
        Ok(())
    }

    pub fn status(&self) -> Result<String, String> {
        Ok(match self.session()? {
            Some(s) if !s.access_token.is_empty() => format!(
                "Sesión guardada · Cuenta: {} · Acceso al modelo pendiente de verificar.",
                if s.email.is_empty() {
                    "ChatGPT"
                } else {
                    &s.email
                }
            ),
            _ => "Sin conexión. Continuá con ChatGPT; no necesitás instalar Codex.".into(),
        })
    }

    pub fn has_session(&self) -> Result<bool, String> {
        Ok(self.session()?.is_some_and(|session| {
            !session.access_token.is_empty() && !session.refresh_token.is_empty()
        }))
    }

    #[cfg(all(test, windows))]
    pub(crate) fn test_connection(root: &Path, endpoint: String) -> Arc<Self> {
        let mut connection = Self::new(root).unwrap();
        let inner = Arc::get_mut(&mut connection).unwrap();
        inner.client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        inner.resource = endpoint;
        inner
            .save(&Session {
                client_id: "fixture-client".into(),
                subject: "fixture-account".into(),
                email: String::new(),
                access_token: "fixture-access".into(),
                refresh_token: "fixture-refresh".into(),
                id_token: "fixture-id".into(),
                expires_at: now() + 3600,
                scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
            })
            .unwrap();
        connection
    }

    pub async fn login(
        &self,
        mut cancel: watch::Receiver<bool>,
        notice: impl Fn(String),
    ) -> Result<String, String> {
        tokio::select! {
            _ = cancelled(&mut cancel) => Err("Inicio de sesión cancelado.".into()),
            result = tokio::time::timeout(Duration::from_secs(900), self.login_native_inner(notice)) =>
                result.map_err(|_| "El inicio de sesión venció. Volvé a intentarlo.".to_owned())?,
        }
    }

    async fn access_token(&self) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let mut session = self
            .session()?
            .ok_or("Iniciá sesión con ChatGPT desde Configurar agentes.")?;
        if session.access_token.is_empty() || session.refresh_token.is_empty() {
            return Err("Iniciá sesión con ChatGPT desde Configurar agentes.".into());
        }
        if session.expires_at <= now() + 90 {
            let native = session.client_id == quota_auth::CLIENT_ID;
            let mut fields = vec![
                ("grant_type", "refresh_token"),
                ("client_id", session.client_id.as_str()),
                ("refresh_token", session.refresh_token.as_str()),
            ];
            if !native {
                fields.push(("resource", RESOURCE));
            }
            let response = self
                .client
                .post(format!(
                    "{AUTH}{}",
                    if native {
                        "/oauth/token"
                    } else {
                        "/api/accounts/oauth/token"
                    }
                ))
                .timeout(Duration::from_secs(30))
                .form(&fields)
                .send()
                .await
                .map_err(|_| "No se pudo renovar la sesión. No se envió el prompt.")?;
            let tokens = bounded_json(response).await?;
            session = if native {
                quota_auth::native_tokens(&tokens, Some(&session))?
            } else {
                token_session(&tokens, &session.client_id, Some(&session))?
            };
            self.save(&session)?;
        }
        Ok(session.access_token)
    }

    pub async fn models(&self) -> Result<Vec<ModelChoice>, String> {
        let token = self.access_token().await?;
        let native = self.uses_native_auth()?;
        let mut value = Value::Null;
        // Backend catalog compatibility, distinct from our truthful User-Agent/package
        // version. Hermes uses the latest-catalog sentinel then the legacy sentinel.
        for version in if native {
            vec![Some("99.0.0"), Some("0.0.0")]
        } else {
            vec![None]
        } {
            let mut request = self
                .client
                .get(format!("{}/models", self.inference_resource()?));
            if let Some(version) = version {
                request = request.query(&[("client_version", version)]);
            }
            value = bounded_json(
                codex_routing(request, &token)
                    .bearer_auth(&token)
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                    .map_err(|_| "No se pudo consultar el catálogo de la cuenta.")?,
            )
            .await?;
            if value["models"]
                .as_array()
                .is_some_and(|models| !models.is_empty())
            {
                break;
            }
        }
        let models = value["models"]
            .as_array()
            .ok_or("El catálogo de la cuenta no es válido.")?
            .iter()
            .filter(|m| m["visibility"].as_str() == Some("list"))
            .filter_map(|m| {
                Some(ModelChoice {
                    slug: m["slug"].as_str()?.into(),
                    label: m["display_name"]
                        .as_str()
                        .unwrap_or(m["slug"].as_str()?)
                        .into(),
                    context_window: m["context_window"].as_u64().filter(|n| *n > 0),
                })
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            return Err("La cuenta no anunció modelos disponibles para esta aplicación.".into());
        }
        Ok(models)
    }

    pub async fn logout(&self) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let session = self.session()?;
        let mut revoked = true;
        if let Some(mut session) = session {
            let result = self
                .client
                .post(format!("{AUTH}/api/accounts/oauth/revoke"))
                .timeout(Duration::from_secs(20))
                .form(&[
                    ("token", session.refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", session.client_id.as_str()),
                ])
                .send()
                .await;
            revoked = result.is_ok_and(|r| r.status().as_u16() == 200);
            session.access_token.clear();
            session.refresh_token.clear();
            session.id_token.clear();
            session.expires_at = 0;
            self.save(&session)?;
        }
        Ok(if revoked { "Sesión del harness desconectada." } else { "Sesión local borrada. No se confirmó la revocación remota: desconectá Ascurra Harness en ChatGPT Settings." }.into())
    }

    pub async fn generate(
        &self,
        body: Value,
        mut cancel: watch::Receiver<bool>,
        mut text: impl FnMut(String),
    ) -> Result<Generation, GenerationError> {
        let sent = std::sync::atomic::AtomicBool::new(false);
        let mut diagnostics = json!({"phase":"prepare"});
        let result = tokio::select! { _ = cancelled(&mut cancel) => Err("Cancelado.".into()), result = async {
            let token = self.access_token().await?;
            if self.uses_native_auth()? {
                let quota = self.read_account_usage("https://chatgpt.com/backend-api/wham/usage", &token).await?;
                ensure_plan_quota(&quota)?;
            }
            diagnostics["phase"] = json!("send_request");
            sent.store(true, std::sync::atomic::Ordering::Relaxed);
            let response = codex_routing(self.client.post(format!("{}/responses", self.inference_resource()?)), &token).bearer_auth(token)
                .header("Accept", "text/event-stream").json(&body).send().await.map_err(|error| {
                    diagnostics["failure"] = json!("request_transport");
                    diagnostics["transport"] = crate::transport_diagnostics::transport_error(error);
                    "No se pudo conectar con OpenAI. No se reintentó la generación."
                })?;
            diagnostics["http_status"] = json!(response.status().as_u16());
            diagnostics["request_id"] = json!(response.headers().get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_').take(128).collect::<String>()));
            if !response.status().is_success() {
                diagnostics["phase"] = json!("http_response");
                diagnostics["failure"] = json!("http_status");
                bounded_json(response).await?;
                return Err("Respuesta inesperada de OpenAI.".into());
            }
            read_stream(response, &mut text, &mut diagnostics).await
        } => result };
        result.map_err(|message| GenerationError {
            message,
            usage_unknown: sent.load(std::sync::atomic::Ordering::Relaxed),
            diagnostics,
        })
    }
}

fn ensure_plan_quota(quota: &crate::quota::AccountUsage) -> Result<(), String> {
    if quota
        .windows
        .iter()
        .any(|window| window.remaining_percent <= 0.0)
    {
        Err("La cuota de cuenta está agotada. Se detuvo el pedido antes de generar para evitar usar créditos.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn parse_callback(
    request: &[u8],
    redirect: &str,
    state: &str,
    issued: Option<&str>,
) -> Result<(String, String), String> {
    let line = std::str::from_utf8(request)
        .map_err(|_| "Retorno inválido.")?
        .lines()
        .next()
        .ok_or("Retorno vacío.")?;
    let words: Vec<_> = line.split_whitespace().collect();
    if words.len() != 3 || words[0] != "GET" {
        return Err("Método de retorno inválido.".into());
    }
    let base = Url::parse(redirect).unwrap();
    let url = base
        .join(words[1])
        .map_err(|_| "URL de retorno inválida.")?;
    if url.origin() != base.origin() || url.path() != "/auth/callback" {
        return Err("Ruta de retorno inválida.".into());
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let get = |name: &str| -> Result<String, String> {
        let values = pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.to_string())
            .collect::<Vec<_>>();
        if values.len() != 1 {
            return Err("Parámetro de retorno ausente o duplicado.".into());
        }
        Ok(values[0].clone())
    };
    if get("state")? != state {
        return Err("El estado OAuth no coincide.".into());
    }
    if pairs.iter().any(|(key, _)| key == "error") {
        return Err("Autorización rechazada por el usuario.".into());
    }
    let supplied = pairs.iter().any(|(key, _)| key == "client_id");
    let client = if supplied {
        get("client_id")?
    } else {
        issued
            .ok_or("Registro incompleto: falta client_id.")?
            .into()
    };
    if client.is_empty()
        || client == "dynamic_agent_client"
        || issued.is_some_and(|expected| client != expected)
    {
        return Err("La identidad del cliente OAuth no coincide.".into());
    }
    let code = get("code")?;
    if code.is_empty() {
        return Err("Código OAuth vacío.".into());
    }
    Ok((code, client))
}

#[derive(Deserialize)]
struct Identity {
    sub: String,
    #[serde(default)]
    nonce: String,
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}
fn validate_identity_nonce(
    token: &str,
    keys: &JwkSet,
    client: &str,
    nonce: Option<&str>,
) -> Result<Identity, String> {
    let header = decode_header(token).map_err(|_| "Identidad de OpenAI inválida.")?;
    if header.alg != Algorithm::RS256 {
        return Err("Algoritmo de identidad no admitido.".into());
    }
    let jwk = header
        .kid
        .as_ref()
        .and_then(|kid| keys.find(kid))
        .ok_or("No se encontró la clave de identidad.")?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| "Clave de identidad inválida.")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[AUTH]);
    validation.set_audience(&[client]);
    validation.leeway = 15;
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| "No se pudo verificar la firma, vigencia o destinatario de la identidad.")?
        .claims;
    if nonce.is_some_and(|nonce| identity.nonce != nonce) || identity.sub.is_empty() {
        return Err("La identidad no pertenece a este intento de autorización.".into());
    }
    Ok(identity)
}

fn token_session(
    tokens: &Value,
    client: &str,
    previous: Option<&Session>,
) -> Result<Session, String> {
    if tokens["token_type"]
        .as_str()
        .is_some_and(|s| !s.eq_ignore_ascii_case("bearer"))
    {
        return Err("Tipo de credencial no admitido.".into());
    }
    let scopes = tokens["scope"]
        .as_str()
        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .or_else(|| previous.map(|s| s.scopes.clone()))
        .ok_or("OpenAI no informó los permisos concedidos.")?;
    if !scopes.iter().any(|s| s == "chatgpt.tokens.use.direct")
        || !scopes.iter().any(|s| s == "resource.invoke")
    {
        return Err(
            "No se concedió permiso para usar el plan ChatGPT. No se habilitó inferencia.".into(),
        );
    }
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("OpenAI no devolvió una credencial de acceso.")?
        .into();
    let refresh_token = tokens["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| previous.map(|s| s.refresh_token.clone()))
        .ok_or("OpenAI no devolvió una sesión renovable.")?;
    let ttl = tokens["expires_in"]
        .as_u64()
        .filter(|ttl| *ttl > 0 && *ttl < 365 * 24 * 3600)
        .ok_or("OpenAI no informó una vigencia válida.")?;
    Ok(Session {
        client_id: client.into(),
        subject: previous.map(|s| s.subject.clone()).unwrap_or_default(),
        email: previous.map(|s| s.email.clone()).unwrap_or_default(),
        access_token,
        refresh_token,
        id_token: tokens["id_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous.map(|s| s.id_token.clone()))
            .ok_or("Falta la identidad de la sesión.")?,
        expires_at: now() + ttl,
        scopes,
    })
}

#[cfg(windows)]
fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI current-user scope; every successful allocation is released below.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err("Windows no pudo proteger o abrir la sesión de este usuario.".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}
#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, true)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, false)
}
#[cfg(not(windows))]
fn protect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("Falta implementar el almacén seguro de credenciales en este sistema.".into())
}
#[cfg(not(windows))]
fn unprotect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("Falta implementar el almacén seguro de credenciales en este sistema.".into())
}

pub struct GenerationError {
    pub message: String,
    pub usage_unknown: bool,
    pub diagnostics: Value,
}

pub struct Generation {
    pub response: Value,
    pub completed: bool,
}

#[derive(Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
}
impl SseDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Value>, String> {
        if self.pending.len() + bytes.len() > MAX_JSON {
            return Err("Evento de streaming demasiado grande.".into());
        }
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        loop {
            let lf = self
                .pending
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|pos| (pos, 2));
            let crlf = self
                .pending
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|pos| (pos, 4));
            let boundary = match (lf, crlf) {
                (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            let Some((pos, length)) = boundary else {
                break;
            };
            let frame = std::str::from_utf8(&self.pending[..pos])
                .map_err(|_| "Streaming UTF-8 inválido.")?;
            let data = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() && data != "[DONE]" {
                events.push(
                    serde_json::from_str(&data).map_err(|_| "Evento de streaming inválido.")?,
                );
            }
            self.pending.drain(..pos + length);
        }
        Ok(events)
    }
}

async fn read_stream(
    response: Response,
    text: &mut impl FnMut(String),
    diagnostics: &mut Value,
) -> Result<Generation, String> {
    diagnostics["phase"] = json!("stream");
    let started = std::time::Instant::now();
    let mut chunks_received = 0u64;
    let mut bytes_received = 0u64;
    let mut events_received = 0u64;
    diagnostics["stream"] = json!({"chunks_received":0,"bytes_received":0,"events_received":0,"last_event":null,"last_chunk_ms":null,"idle_timeout_seconds":180});
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut batch = String::new();
    let mut length = 0usize;
    let mut last = std::time::Instant::now();
    let mut output = StreamOutput::default();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(180), stream.next()).await;
        let chunk = match next {
            Err(_) => {
                diagnostics["failure"] = json!("stream_idle_timeout");
                diagnostics["stream"]["elapsed_ms"] = json!(started.elapsed().as_millis());
                return Err("OpenAI dejó de responder durante 180 segundos. El consumo de la solicitud queda incompleto.".into());
            }
            Ok(None) => break,
            Ok(Some(Err(error))) => {
                diagnostics["failure"] = json!("stream_transport");
                diagnostics["transport"] = crate::transport_diagnostics::transport_error(error);
                diagnostics["stream"]["elapsed_ms"] = json!(started.elapsed().as_millis());
                return Err(
                    "Se interrumpió el streaming. El consumo de la solicitud queda incompleto."
                        .into(),
                );
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        chunks_received += 1;
        bytes_received = bytes_received.saturating_add(chunk.len() as u64);
        diagnostics["stream"]["chunks_received"] = json!(chunks_received);
        diagnostics["stream"]["bytes_received"] = json!(bytes_received);
        diagnostics["stream"]["last_chunk_ms"] = json!(started.elapsed().as_millis());
        let events = decoder.feed(&chunk).map_err(|message| {
            diagnostics["failure"] = json!("stream_decode");
            message
        })?;
        for event in events {
            events_received += 1;
            diagnostics["stream"]["events_received"] = json!(events_received);
            diagnostics["stream"]["last_event"] = json!(
                event["type"]
                    .as_str()
                    .unwrap_or("unknown")
                    .chars()
                    .take(80)
                    .collect::<String>()
            );
            output.push(&event).map_err(|message| {
                diagnostics["failure"] = json!("stream_protocol");
                message
            })?;
            match event["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    let delta = event["delta"].as_str().ok_or("Delta de texto inválido.")?;
                    length += delta.len();
                    if length > MAX_JSON {
                        return Err("La respuesta superó el límite local de texto.".into());
                    }
                    batch.push_str(delta);
                    if last.elapsed() >= Duration::from_millis(60) {
                        text(std::mem::take(&mut batch));
                        last = std::time::Instant::now();
                    }
                }
                "response.completed" | "response.failed" | "response.incomplete" => {
                    if !batch.is_empty() {
                        text(std::mem::take(&mut batch));
                    }
                    let completed = event["type"] == "response.completed";
                    let mut response = event["response"].clone();
                    if !response.is_object() {
                        return Err("Evento terminal sin respuesta válida.".into());
                    }
                    output.finish(&mut response)?;
                    return Ok(Generation {
                        response,
                        completed,
                    });
                }
                "error" => {
                    diagnostics["failure"] = json!("stream_error");
                    let (message, detail) = stream_error(&event);
                    diagnostics["provider_error"] = detail;
                    return Err(message);
                }
                _ => {}
            }
        }
    }
    if !batch.is_empty() {
        text(batch);
    }
    diagnostics["failure"] = json!("stream_eof_without_terminal");
    diagnostics["stream"]["elapsed_ms"] = json!(started.elapsed().as_millis());
    Err("El streaming terminó sin confirmación final. El consumo queda incompleto.".into())
}

/// Completed items are authoritative even when the terminal response has a sparse output.
#[derive(Default)]
struct StreamOutput {
    items: std::collections::BTreeMap<usize, Value>,
    parts: std::collections::BTreeMap<(usize, usize), String>,
    item_bytes: usize,
}
impl StreamOutput {
    fn push(&mut self, event: &Value) -> Result<(), String> {
        let index = event["output_index"].as_u64().unwrap_or(0) as usize;
        let part = event["content_index"].as_u64().unwrap_or(0) as usize;
        if index > 127 || part > 127 {
            return Err("El streaming contiene demasiados elementos.".into());
        }
        match event["type"].as_str().unwrap_or("") {
            "response.output_item.done" => {
                if !event["item"].is_object() {
                    return Err("Elemento final de streaming inválido.".into());
                }
                let item = event["item"].clone();
                self.item_bytes += serde_json::to_vec(&item)
                    .map_err(|_| "Elemento final inválido.")?
                    .len();
                if let Some(previous) = self.items.insert(index, item) {
                    self.item_bytes -= serde_json::to_vec(&previous)
                        .map_err(|_| "Elemento final inválido.")?
                        .len();
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.parts
                    .entry((index, part))
                    .or_default()
                    .push_str(event["delta"].as_str().unwrap_or(""));
            }
            "response.output_text.done" => {
                self.parts
                    .insert((index, part), event["text"].as_str().unwrap_or("").into());
            }
            "response.refusal.done" => {
                self.parts.insert(
                    (index, part),
                    event["refusal"].as_str().unwrap_or("").into(),
                );
            }
            _ => {}
        }
        let size = self.parts.values().map(String::len).sum::<usize>() + self.item_bytes;
        if size > MAX_JSON {
            return Err("La respuesta superó el límite local de salida.".into());
        }
        Ok(())
    }

    fn finish(self, response: &mut Value) -> Result<(), String> {
        let mut items = response["output"].as_array().cloned().unwrap_or_default();
        let mut merged = items
            .into_iter()
            .enumerate()
            .collect::<std::collections::BTreeMap<_, _>>();
        merged.extend(self.items);
        items = merged.into_values().collect();
        // Text-only streams still need assistant history for the next turn.
        if visible_text(&items).trim().is_empty() && !self.parts.is_empty() {
            let text = self.parts.into_values().collect::<Vec<_>>().join("\n\n");
            if !text.trim().is_empty() {
                items.push(json!({"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]}));
            }
        }
        if serde_json::to_vec(&items)
            .map_err(|_| "Salida final inválida.")?
            .len()
            > MAX_JSON
        {
            return Err("La respuesta superó el límite local de salida.".into());
        }
        response["output"] = Value::Array(items);
        Ok(())
    }
}

pub fn visible_text(output: &[Value]) -> String {
    output
        .iter()
        .filter(|item| item["type"] == "message" && item["role"] == "assistant")
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter_map(|part| match part["type"].as_str() {
            Some("output_text") => part["text"].as_str(),
            Some("refusal") => part["refusal"].as_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn response_error(response: &Value) -> String {
    stream_error(response).0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_errors_preserve_flat_and_nested_details_without_fake_http_status() {
        for event in [
            json!({"type":"error", "code":"invalid_request", "message":"Invalid input"}),
            json!({"type":"error", "error":{"type":"invalid_request_error", "code":"invalid_request", "message":"Invalid input"}}),
        ] {
            let (message, details) = stream_error(&event);
            assert!(message.contains("Invalid input"));
            assert!(!message.contains("HTTP 400"));
            assert_eq!(details["code"], "invalid_request");
            assert_eq!(details["message"], "Invalid input");
        }
    }

    #[test]
    fn stream_error_details_are_bounded_and_omit_urls() {
        let (_, details) = stream_error(
            &json!({"message":format!("https://example.test/?token=secret {}", "x".repeat(2000)), "request":{"secret":"hidden"}}),
        );
        assert!(details["message"].as_str().unwrap().chars().count() <= 500);
        assert!(!details.to_string().contains("secret"));
        assert!(details.get("request").is_none());
        let (message, _) = stream_error(&json!({"type":"error"}));
        assert!(!message.contains("HTTP"));
    }

    async fn diagnostic_stream_fixture(truncated: bool) -> (Result<Generation, String>, Value) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = "data: {\"type\":\"response.created\"}\n\n";
        let length = if truncated {
            body.len() + 100
        } else {
            body.len()
        };
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            socket.read(&mut request).await.unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: fixture_request\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let response = Client::new()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        let mut diagnostics =
            json!({"http_status":response.status().as_u16(),"request_id":"fixture_request"});
        let result = read_stream(response, &mut |_| {}, &mut diagnostics).await;
        server.await.unwrap();
        (result, diagnostics)
    }

    #[tokio::test]
    async fn truncated_stream_keeps_transport_causes() {
        let (result, diagnostics) = diagnostic_stream_fixture(true).await;
        assert!(result.is_err());
        assert_eq!(diagnostics["failure"], "stream_transport");
        assert_eq!(diagnostics["http_status"], 200);
        assert_eq!(diagnostics["request_id"], "fixture_request");
        assert!(
            diagnostics["transport"]["causes"]
                .as_array()
                .is_some_and(|causes| !causes.is_empty())
        );
        assert!(!diagnostics.to_string().contains("http://"));
    }

    #[tokio::test]
    async fn stream_eof_is_distinct_from_transport_error() {
        let (result, diagnostics) = diagnostic_stream_fixture(false).await;
        assert!(result.is_err());
        assert_eq!(diagnostics["failure"], "stream_eof_without_terminal");
        assert_eq!(diagnostics["stream"]["events_received"], 1);
        assert_eq!(diagnostics["stream"]["last_event"], "response.created");
        assert!(diagnostics["transport"].is_null());
    }
    #[test]
    fn bad_request_includes_provider_detail() {
        let error = http_error(
            400,
            &json!({"error":{"message":"Unsupported reasoning effort: minimal"}}),
        );
        assert!(error.contains("Unsupported reasoning effort: minimal"));
        assert!(error.contains("HTTP 400"));
        assert!(error.contains("No se reintentó"));
        assert!(
            !http_error(400, &json!({"error":{"message":"x".repeat(1000)}}))
                .contains(&"x".repeat(501))
        );
    }
    #[test]
    fn legacy_efforts_are_migrated_only_for_incompatible_models() {
        use crate::settings::{codex_effort, codex_efforts};
        assert_eq!(codex_effort("gpt-6.1-sol", "minimal"), "low");
        assert_eq!(codex_effort("gpt-6.1-sol", "none"), "low");
        assert_eq!(codex_effort("gpt-6-astra", "none"), "low");
        assert_eq!(codex_effort("gpt-6-luna", "none"), "none");
        assert_eq!(codex_effort("gpt-5", "minimal"), "minimal");
        assert_eq!(codex_effort("gpt-6.1-sol", ""), "");
        assert_eq!(codex_effort("gpt-6.1-sol", "high"), "high");
        assert!(!codex_efforts("gpt-6.1-sol").contains(&"minimal"));
        assert!(!codex_efforts("gpt-6.1-sol").contains(&"none"));
    }
    #[test]
    fn completed_items_restore_sparse_terminal_output_without_duplicate_text() {
        let mut output = StreamOutput::default();
        output.push(&json!({"type":"response.output_text.delta","delta":"Hola","output_index":1,"content_index":0})).unwrap();
        let message = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hola completo."}]});
        let reasoning = json!({"type":"reasoning","encrypted_content":"fixture-private-reasoning"});
        output
            .push(&json!({"type":"response.output_item.done","output_index":1,"item":message}))
            .unwrap();
        output
            .push(&json!({"type":"response.output_item.done","output_index":0,"item":reasoning}))
            .unwrap();
        let mut response = json!({"output":[]});
        output.finish(&mut response).unwrap();
        assert_eq!(response["output"].as_array().unwrap().len(), 2);
        assert_eq!(response["output"][0]["type"], "reasoning");
        assert_eq!(
            visible_text(response["output"].as_array().unwrap()),
            "Hola completo."
        );
    }

    #[test]
    fn text_done_and_refusals_are_visible_and_retained_in_history() {
        let mut output = StreamOutput::default();
        output
            .push(&json!({"type":"response.output_text.done","text":"Respuesta sin deltas."}))
            .unwrap();
        let mut response = json!({"output":[]});
        output.finish(&mut response).unwrap();
        assert_eq!(
            visible_text(response["output"].as_array().unwrap()),
            "Respuesta sin deltas."
        );
        assert_eq!(
            visible_text(&[
                json!({"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"No puedo realizar esa acción."}]})
            ]),
            "No puedo realizar esa acción."
        );
    }
    #[test]
    fn oauth_callback_rejects_cross_attempt_and_duplicate_parameters() {
        let redirect = "http://127.0.0.1:12345/auth/callback";
        let request =
            |query: &str| format!("GET /auth/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(
            parse_callback(
                request("code=x&state=wrong&client_id=oaiapp_test").as_bytes(),
                redirect,
                "expected",
                None
            )
            .is_err()
        );
        assert!(
            parse_callback(
                request("code=x&state=expected&state=expected&client_id=oaiapp_test").as_bytes(),
                redirect,
                "expected",
                None
            )
            .is_err()
        );
        assert!(
            parse_callback(
                request("code=x&state=expected&client_id=dynamic_agent_client").as_bytes(),
                redirect,
                "expected",
                None
            )
            .is_err()
        );
        assert!(
            parse_callback(
                request("code=x&state=expected&client_id=other").as_bytes(),
                redirect,
                "expected",
                Some("saved")
            )
            .is_err()
        );
        assert_eq!(
            parse_callback(
                request("code=x&state=expected&client_id=oaiapp_test").as_bytes(),
                redirect,
                "expected",
                None
            )
            .unwrap(),
            ("x".into(), "oaiapp_test".into())
        );
    }
    #[test]
    fn tokens_without_plan_permission_are_not_accepted() {
        let value = json!({"access_token":"fixture", "refresh_token":"fixture", "id_token":"fixture", "expires_in":3600, "scope":"openid profile"});
        assert!(token_session(&value, "fixture-client", None).is_err());
    }

    #[test]
    fn exhausted_quota_stops_native_generation_even_if_other_window_has_capacity() {
        let quota = crate::quota::AccountUsage::parse(&json!({"rate_limit": {
            "primary_window":{"used_percent":100,"limit_window_seconds":18000},
            "secondary_window":{"used_percent":20,"limit_window_seconds":604800}
        }}))
        .unwrap();
        assert!(ensure_plan_quota(&quota).is_err());
        let available = crate::quota::AccountUsage::parse(&json!({"rate_limit": {
            "primary_window":{"used_percent":50,"limit_window_seconds":18000}
        }}))
        .unwrap();
        assert!(ensure_plan_quota(&available).is_ok());
    }
    #[test]
    fn sse_handles_fragmented_unicode_and_crlf_frames() {
        let event = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"á\"}\r\n\r\n";
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for byte in event.as_bytes() {
            events.extend(decoder.feed(&[*byte]).unwrap());
        }
        assert_eq!(events[0]["delta"], "á");
        assert!(decoder.feed(b"data: [DONE]\n\n").unwrap().is_empty());
        assert!(decoder.feed(b"data: broken\n\n").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn credential_file_is_encrypted_and_round_trips() {
        let plain = b"fixture-token-not-a-real-credential";
        let encrypted = protect(plain).unwrap();
        assert!(!encrypted.windows(plain.len()).any(|w| w == plain));
        assert_eq!(unprotect(&encrypted).unwrap(), plain);
    }

    #[cfg(windows)]
    #[test]
    fn saved_session_survives_reopening_the_connection() {
        let temp = tempfile::tempdir().unwrap();
        let connection = Connection::test_connection(temp.path(), "http://127.0.0.1:1/v1".into());
        assert!(connection.has_session().unwrap());
        drop(connection);
        let restored = Connection::new(temp.path()).unwrap();
        assert!(restored.has_session().unwrap());
        assert!(restored.status().unwrap().contains("Sesión guardada"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn quota_is_a_read_only_request_and_rejection_does_not_clear_auth() {
        for (status, body) in [
            (
                200,
                r#"{"rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":18000}}}"#,
            ),
            (401, "rejected"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/usage", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 8192];
                let mut length = 0;
                while !bytes[..length].windows(4).any(|part| part == b"\r\n\r\n") {
                    length += stream.read(&mut bytes[length..]).await.unwrap();
                    assert!(length < bytes.len());
                }
                let request = String::from_utf8_lossy(&bytes[..length]).to_lowercase();
                assert!(request.starts_with("get /usage "));
                assert!(request.contains("authorization: bearer fixture-access"));
                let response = format!(
                    "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let temp = tempfile::tempdir().unwrap();
            let connection = Connection::test_connection(temp.path(), "unused".into());
            let result = connection.account_usage_at(&endpoint).await;
            if status == 200 {
                assert_eq!(result.unwrap().windows[0].remaining_percent, 60.0);
            } else {
                assert!(result.unwrap_err().contains("HTTP 401"));
            }
            assert!(connection.has_session().unwrap());
            server.await.unwrap();
        }
    }
}
