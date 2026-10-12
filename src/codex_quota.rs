//! One native Codex OAuth grant for inference, catalog and account quota.
//! Protocol mirrors Codex/Hermes device login; credentials belong to the harness.
use super::*;

pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(super) const BACKEND: &str = "https://chatgpt.com/backend-api/codex";

fn same_account(previous: &Session, identity: &Identity) -> bool {
    if previous.subject == identity.sub {
        return true;
    }
    // SiWC registers a different client/principal. Its sub is not necessarily the
    // native Codex subject. Only the migration can match the issuer-signed email;
    // after migration, native reauthorization always requires the exact subject.
    previous.client_id != CLIENT_ID
        && !previous.email.is_empty()
        && identity.email_verified != Some(false)
        && identity
            .email
            .as_deref()
            .is_some_and(|email| email.eq_ignore_ascii_case(&previous.email))
}

pub(super) fn native_tokens(tokens: &Value, previous: Option<&Session>) -> Result<Session, String> {
    if tokens["token_type"]
        .as_str()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
    {
        return Err("Credencial de Codex inválida.".into());
    }
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Codex no devolvió acceso.")?
        .to_owned();
    let refresh_token = tokens["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| previous.map(|s| s.refresh_token.clone()))
        .filter(|s| !s.is_empty())
        .ok_or("Codex no devolvió una sesión renovable.")?;
    let exp = access_token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|claims| claims["exp"].as_u64());
    // JWT expiry is only a scheduling hint from the HTTPS token response.
    let expires_at = tokens["expires_in"]
        .as_u64()
        .filter(|ttl| *ttl > 0 && *ttl <= 30 * 86400)
        .map(|ttl| now() + ttl)
        .or(exp)
        .filter(|exp| *exp > now() && *exp <= now() + 30 * 86400)
        .ok_or("Codex no informó una vigencia válida.")?;
    Ok(Session {
        client_id: CLIENT_ID.into(),
        subject: previous.map(|s| s.subject.clone()).unwrap_or_default(),
        email: previous.map(|s| s.email.clone()).unwrap_or_default(),
        access_token,
        refresh_token,
        id_token: tokens["id_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous.map(|s| s.id_token.clone()))
            .ok_or("Codex no devolvió una identidad verificable.")?,
        expires_at,
        scopes: tokens["scope"]
            .as_str()
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .or_else(|| previous.map(|s| s.scopes.clone()))
            .unwrap_or_default(),
    })
}

impl Connection {
    pub fn uses_native_auth(&self) -> Result<bool, String> {
        Ok(self
            .session()?
            .is_some_and(|s| s.client_id == CLIENT_ID && !s.access_token.is_empty()))
    }

    pub(super) fn inference_resource(&self) -> Result<String, String> {
        Ok(if self.uses_native_auth()? {
            BACKEND.into()
        } else {
            self.resource.clone()
        })
    }

    pub(super) async fn login_native_inner(
        &self,
        notice: impl Fn(String),
    ) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let previous = self.session()?;
        let response = self
            .client
            .post(format!("{AUTH}/api/accounts/deviceauth/usercode"))
            .timeout(Duration::from_secs(20))
            .json(&json!({"client_id":CLIENT_ID}))
            .send()
            .await
            .map_err(|_| "No se pudo iniciar la autorización de Codex.")?;
        let device = bounded_json(response).await?;
        let user_code = device["user_code"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Codex no devolvió un código de autorización.")?;
        let device_id = device["device_auth_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Respuesta de autorización inválida.")?;
        let interval = device["interval"]
            .as_u64()
            .or_else(|| device["interval"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(5)
            .clamp(1, 60);
        notice(format!(
            "Ingresá {user_code} en https://auth.openai.com/codex/device con la misma cuenta del harness. Esta autorización sirve para chats y cuota."
        ));
        webbrowser::open("https://auth.openai.com/codex/device")
            .map_err(|_| "No se pudo abrir el navegador para autorizar Codex.")?;
        let authorization = loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let response = self
                .client
                .post(format!("{AUTH}/api/accounts/deviceauth/token"))
                .timeout(Duration::from_secs(20))
                .json(&json!({"device_auth_id":device_id,"user_code":user_code}))
                .send()
                .await
                .map_err(|_| "Se interrumpió la autorización de Codex.")?;
            match response.status().as_u16() {
                403 | 404 => continue,
                200 => break bounded_json(response).await?,
                status => {
                    return Err(format!(
                        "La autorización de Codex fue rechazada (HTTP {status})."
                    ));
                }
            }
        };
        let code = authorization["authorization_code"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Falta el código OAuth.")?;
        let verifier = authorization["code_verifier"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Falta la verificación OAuth.")?;
        let response = self
            .client
            .post(format!("{AUTH}/oauth/token"))
            .timeout(Duration::from_secs(20))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code),
                ("code_verifier", verifier),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ])
            .send()
            .await
            .map_err(|_| "No se pudo completar la autorización de Codex.")?;
        let tokens = bounded_json(response).await?;
        let id_token = tokens["id_token"]
            .as_str()
            .ok_or("Codex no devolvió una identidad verificable.")?;
        let keys: JwkSet = serde_json::from_value(
            bounded_json(
                self.client
                    .get(format!("{AUTH}/.well-known/jwks.json"))
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .map_err(|_| "No se pudo verificar la cuenta de Codex.")?,
            )
            .await?,
        )
        .map_err(|_| "Claves de identidad inválidas.")?;
        let identity = validate_identity_nonce(id_token, &keys, CLIENT_ID, None)?;
        if previous
            .as_ref()
            .is_some_and(|s| !s.subject.is_empty() && !same_account(s, &identity))
        {
            return Err("Usá la misma cuenta de ChatGPT que ya autorizaste en el harness.".into());
        }
        let mut session = native_tokens(&tokens, None)?;
        session.subject = identity.sub;
        session.email = identity.email.unwrap_or_default();
        // Verify read access before atomically replacing the previous grant.
        self.read_account_usage(
            "https://chatgpt.com/backend-api/wham/usage",
            &session.access_token,
        )
        .await?;
        self.save(&session)?;
        Ok("Codex conectado · autorización única para chats, modelos y cuota.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_refresh_preserves_identity_and_rotates_credentials() {
        assert!(native_tokens(&json!({}), None).is_err());
        let mut session = native_tokens(&json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","id_token":"fixture-id","expires_in":3600}), None).unwrap();
        session.subject = "fixture-account".into();
        let rotated = native_tokens(&json!({"access_token":"fixture-next","refresh_token":"fixture-rotated","expires_in":3600}), Some(&session)).unwrap();
        assert_eq!(rotated.subject, "fixture-account");
        assert_eq!(rotated.id_token, "fixture-id");
        assert_eq!(rotated.refresh_token, "fixture-rotated");
        assert_eq!(rotated.access_token, "fixture-next");
    }

    #[test]
    fn migration_matches_signed_account_email_but_native_reauth_requires_subject() {
        let mut previous = native_tokens(&json!({"access_token":"fixture","refresh_token":"fixture","id_token":"fixture","expires_in":3600}), None).unwrap();
        previous.subject = "native-sub".into();
        previous.email = "fixture@example.test".into();
        let identity = Identity {
            sub: "different-sub".into(),
            nonce: String::new(),
            email: Some("fixture@example.test".into()),
            email_verified: Some(true),
        };
        assert!(!same_account(&previous, &identity));
        previous.client_id = "siwc-fixture-client".into();
        assert!(same_account(&previous, &identity));
        assert!(!same_account(
            &previous,
            &Identity {
                email: Some("other@example.test".into()),
                ..identity
            }
        ));
    }
    #[cfg(windows)]
    #[test]
    fn one_encrypted_native_session_persists_and_routes_catalog_and_inference() {
        let temp = tempfile::tempdir().unwrap();
        let connection = Connection::test_connection(temp.path(), "unused".into());
        let mut session = connection.session().unwrap().unwrap();
        session.client_id = CLIENT_ID.into();
        connection.save(&session).unwrap();
        assert_eq!(connection.inference_resource().unwrap(), BACKEND);
        drop(connection);
        let restored = Connection::new(temp.path()).unwrap();
        assert!(restored.uses_native_auth().unwrap());
        assert!(restored.has_session().unwrap());
    }
}
