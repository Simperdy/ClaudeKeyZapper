//! OIDC sign-in (authorization code + PKCE, loopback redirect, public client) and token refresh for SSO profiles.
//! The gateway verifies the OIDC access token (or the ID token, per profile); KeyZapper only obtains and refreshes it.
//! Messages never contain token material.

use crate::errors::{Error, Result};
use crate::models::{OidcTokenType, Profile};
use crate::tokens::{TokenSet, TokenStore};
use crate::{gateway, l};
use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::OsRng;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_SCOPE: &str = "openid profile offline_access";
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// Claude Code's own default for `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` when the profile does not set it.
const DEFAULT_HELPER_TTL_MS: i64 = 300_000;
const TTL_ENV: &str = "CLAUDE_CODE_API_KEY_HELPER_TTL_MS";

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn fail(message: impl Into<String>) -> Error {
    Error::Oidc(message.into())
}

/// https required; http only for loopback hosts (tests, local Keycloak).
pub fn validate_url(text: &str) -> Result<url::Url> {
    let url = url::Url::parse(text.trim()).map_err(|_| fail(l!("Ungültige URL: %@", text)))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Ok(url)
    } else {
        Err(fail(l!("Die SSO-Adresse muss https verwenden: %@", text)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
}

pub fn discover(issuer: &str) -> Result<Discovery> {
    validate_url(issuer)?;
    let trimmed = issuer.trim().trim_end_matches('/');
    let url = format!("{trimmed}/.well-known/openid-configuration");
    let (status, body) = http_get(&url)?;
    if status != 200 {
        return Err(fail(l!("Discovery-Dokument nicht abrufbar (HTTP %@).", status)));
    }
    let json: Value = serde_json::from_str(&body).map_err(|_| fail(l!("Discovery-Dokument ist kein gültiges JSON.")))?;
    let field = |name: &str| json.get(name).and_then(Value::as_str).map(String::from);
    if field("issuer").map(|i| i.trim_end_matches('/').to_string()).as_deref() != Some(trimmed) {
        return Err(fail(l!("Der Issuer im Discovery-Dokument stimmt nicht mit der Konfiguration überein.")));
    }
    let (Some(authorization_endpoint), Some(token_endpoint)) = (field("authorization_endpoint"), field("token_endpoint")) else {
        return Err(fail(l!("Das Discovery-Dokument enthält keine Endpunkte.")));
    };
    validate_url(&authorization_endpoint)?;
    validate_url(&token_endpoint)?;
    Ok(Discovery { authorization_endpoint, token_endpoint })
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    B64URL.encode(buf)
}

/// S256 code challenge of a PKCE verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    B64URL.encode(Sha256::digest(verifier.as_bytes()))
}

/// (verifier, challenge)
pub fn new_pkce() -> (String, String) {
    let verifier = random_b64(32);
    let challenge = pkce_challenge(&verifier);
    (verifier, challenge)
}

struct Config<'a> {
    issuer: &'a str,
    client_id: &'a str,
    scope: String,
}

fn config(profile: &Profile) -> Result<Config<'_>> {
    let issuer = profile.oidc_issuer.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let client_id = profile.oidc_client_id.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let (Some(issuer), Some(client_id)) = (issuer, client_id) else {
        return Err(fail(l!("Für das SSO-Profil fehlen Issuer oder Client-ID.")));
    };
    let scope = profile.oidc_scope.as_deref().map(str::trim).filter(|v| !v.is_empty()).unwrap_or(DEFAULT_SCOPE).to_string();
    Ok(Config { issuer, client_id, scope })
}

fn http_get(url: &str) -> Result<(u16, String)> {
    let mut response = gateway::agent(HTTP_TIMEOUT).get(url).call().map_err(|e| fail(e.to_string()))?;
    let status = response.status().as_u16();
    Ok((status, response.body_mut().read_to_string().unwrap_or_default()))
}

fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<(u16, Value)> {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish();
    let mut response = gateway::agent(HTTP_TIMEOUT)
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .send(&body)
        .map_err(|e| fail(e.to_string()))?;
    let status = response.status().as_u16();
    let json = serde_json::from_str(&response.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
    Ok((status, json))
}

/// `exp` claim of a JWT, read without verifying the signature (the token comes straight from the token endpoint).
fn jwt_exp(token: &str) -> Option<i64> {
    let payload = B64URL.decode(token.split('.').nth(1)?.trim_end_matches('=')).ok()?;
    let exp = serde_json::from_slice::<Value>(&payload).ok()?.get("exp")?.clone();
    exp.as_i64().or_else(|| exp.as_f64().map(|v| v as i64))
}

/// Token endpoint reply -> tokens. 400/401 (`invalid_grant` & co) mean the session is over.
/// For `OidcTokenType::Id` the ID token is kept and expires per its `exp` claim (`expires_in` is the access token's).
fn token_response(profile_id: &str, kind: OidcTokenType, status: u16, json: &Value, previous_refresh: Option<&str>, now: i64) -> Result<TokenSet> {
    if status == 400 || status == 401 {
        return Err(Error::SessionExpired(profile_id.to_string()));
    }
    if !(200..300).contains(&status) {
        return Err(fail(l!("Token-Endpunkt antwortete mit HTTP %@.", status)));
    }
    let text = |name: &str| json.get(name).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
    let token = match kind {
        OidcTokenType::Access => text("access_token").ok_or_else(|| fail(l!("Die Antwort enthält kein Access-Token.")))?,
        OidcTokenType::Id => text("id_token").ok_or_else(|| fail(l!("Die Antwort enthält kein ID-Token. Der Scope muss „openid“ enthalten (bei Entra ID zusätzlich „profile“).")))?,
    };
    let Some(refresh_token) = text("refresh_token").or_else(|| previous_refresh.map(String::from)) else {
        return Err(fail(l!("Kein Refresh-Token erhalten. Bei Entra ID muss der Scope „offline_access“ enthalten sein.")));
    };
    let expires_at = match kind {
        OidcTokenType::Access => {
            // An explicit numeric lifetime is authoritative even when it is <= 0: the token is
            // already due (or expired), exactly as before. Only absent/unparseable metadata may
            // fall back to the legacy one-hour cache, and both paths stay unverified for Hermes.
            let declared = json.get("expires_in").and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
            match declared {
                Some(seconds) if seconds > 0 => now.checked_add(seconds).map(|expiry| (expiry, true)),
                Some(seconds) => Some((now.saturating_add(seconds), false)),
                None => jwt_exp(&token).filter(|expiry| *expiry > now).map(|expiry| (expiry, true)),
            }.unwrap_or_else(|| (now.saturating_add(3600), false))
        }
        OidcTokenType::Id => {
            let expiry = jwt_exp(&token).filter(|expiry| *expiry > now).ok_or_else(|| fail(l!("Das ID-Token enthält kein gültiges zukünftiges Ablaufdatum (exp).")))?;
            (expiry, true)
        }
    };
    Ok(TokenSet { refresh_token, access_token: Some(token), expires_at: Some(expires_at.0), token_type: kind, expiry_verified: Some(expires_at.1) })
}

/// Seconds an access token must stay valid to be handed out: Claude Code's helper TTL plus 60 s.
fn margin_secs(profile: &Profile) -> i64 {
    let ttl_ms = profile
        .environment
        .as_ref()
        .and_then(|env| env.get(TTL_ENV))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(DEFAULT_HELPER_TTL_MS);
    ttl_ms / 1000 + 60
}

#[derive(Clone, PartialEq, Eq)]
pub struct GatewayCredential {
    pub token: String,
    pub expires_at: i64,
}

fn cached(tokens: &TokenSet, kind: OidcTokenType, now: i64, margin: i64, require_verified_expiry: bool) -> Option<GatewayCredential> {
    match (&tokens.access_token, tokens.expires_at) {
        (Some(token), Some(expires_at))
            if tokens.token_type == kind
                && !token.is_empty()
                && expires_at - now > margin
                && (!require_verified_expiry || tokens.expiry_verified == Some(true)) =>
        {
            Some(GatewayCredential { token: token.clone(), expires_at })
        }
        _ => None,
    }
}

/// Bearer token for the gateway (access or ID token per profile), with the expiry of that same token.
/// Requires verified expiry metadata; nonconforming access-token providers are rejected rather than trusted to use the legacy one-hour fallback.
pub fn gateway_credential(profile: &Profile, store: &TokenStore, now: i64) -> Result<GatewayCredential> {
    credential(profile, store, now, true)
}

/// Legacy token-only lookup retains compatibility with cached records lacking expiry provenance.
pub fn access_token(profile: &Profile, store: &TokenStore, now: i64) -> Result<String> {
    credential(profile, store, now, false).map(|credential| credential.token)
}

fn credential(profile: &Profile, store: &TokenStore, now: i64, require_verified_expiry: bool) -> Result<GatewayCredential> {
    let (margin, kind) = (margin_secs(profile), profile.oidc_token_type);
    if let Some(credential) = store.read(&profile.id)?.as_ref().and_then(|t| cached(t, kind, now, margin, require_verified_expiry)) {
        return Ok(credential);
    }
    let config = config(profile)?;
    if let Some(dir) = store.file.parent() {
        crate::paths::create_private_dir(dir).map_err(|e| Error::KeyStore(e.to_string()))?;
    }
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(store.lock_file())?;
    lock.lock()?;
    // Dropping `lock` releases it.
    let Some(tokens) = store.read(&profile.id)?.filter(|t| !t.refresh_token.is_empty()) else {
        return Err(Error::SessionExpired(profile.id.clone()));
    };
    if let Some(credential) = cached(&tokens, kind, now, margin, require_verified_expiry) {
        return Ok(credential);
    }
    let discovery = discover(config.issuer)?;
    let (status, json) = post_form(
        &discovery.token_endpoint,
        &[("grant_type", "refresh_token"), ("refresh_token", &tokens.refresh_token), ("client_id", config.client_id), ("scope", &config.scope)],
    )?;
    let fresh = token_response(&profile.id, kind, status, &json, Some(&tokens.refresh_token), now)?;
    let result = GatewayCredential {
        token: fresh.access_token.as_deref().ok_or_else(|| fail(l!("Die Antwort enthält kein Token.")))?.to_string(),
        expires_at: fresh.expires_at.ok_or_else(|| fail(l!("Die Antwort enthält kein Ablaufdatum.")))?,
    };
    store.write(&profile.id, &fresh)?;
    if require_verified_expiry && fresh.expiry_verified != Some(true) {
        return Err(fail(l!("Der Identity Provider lieferte kein verifiziertes Ablaufdatum. Bitte den OIDC-Anbieter konfigurieren, ein gültiges expires_in oder ein JWT exp zu liefern.")));
    }
    Ok(result)
}

/// Interactive sign-in. `open_browser` receives the authorization URL. Fails after `timeout`.
pub fn login(profile: &Profile, store: &TokenStore, open_browser: impl FnOnce(&str) -> Result<()>, timeout: Duration) -> Result<()> {
    let config = config(profile)?;
    let discovery = discover(config.issuer)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
    let (verifier, challenge) = new_pkce();
    let state = random_b64(16);
    let mut auth = validate_url(&discovery.authorization_endpoint)?;
    auth.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", config.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &config.scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let deadline = Instant::now() + timeout;
    open_browser(auth.as_str())?;
    let code = wait_for_callback(&listener, &state, deadline)?;
    let (status, json) = post_form(
        &discovery.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", config.client_id),
            ("code_verifier", &verifier),
        ],
    )?;
    if status == 400 || status == 401 {
        return Err(fail(l!("Der Identity Provider hat die Anmeldung abgelehnt (HTTP %@).", status)));
    }
    store.write(&profile.id, &token_response(&profile.id, profile.oidc_token_type, status, &json, None, unix_now())?)
}

fn wait_for_callback(listener: &TcpListener, state: &str, deadline: Instant) -> Result<String> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(result) = handle_connection(stream, state) {
                    return result;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(fail(l!("Zeitüberschreitung bei der Anmeldung.")));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// None = not the callback (favicon, wrong state, garbage): keep waiting.
fn handle_connection(mut stream: TcpStream, state: &str) -> Option<Result<String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 8192];
    let mut len = 0;
    while len < buf.len() && !buf[..len].windows(2).any(|w| w == b"\r\n") {
        match stream.read(&mut buf[len..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => len += n,
        }
    }
    let head = String::from_utf8_lossy(&buf[..len]);
    let target = head.lines().next().and_then(|line| {
        let mut parts = line.split(' ');
        (parts.next()? == "GET").then(|| parts.next()).flatten()
    });
    let url = target.and_then(|t| url::Url::parse(&format!("http://127.0.0.1{t}")).ok()).filter(|u| u.path() == "/callback");
    let Some(url) = url else {
        respond(&mut stream, 404, &l!("Nicht gefunden."));
        return None;
    };
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    if param("state").as_deref() != Some(state) {
        respond(&mut stream, 400, &l!("Ungültige Anmeldeantwort."));
        return None;
    }
    if let Some(error) = param("error") {
        respond(&mut stream, 400, &l!("Anmeldung fehlgeschlagen. Du kannst dieses Fenster schließen."));
        let detail = param("error_description").unwrap_or_default();
        return Some(Err(fail(format!("{error} {detail}").trim().to_string())));
    }
    let Some(code) = param("code").filter(|c| !c.is_empty()) else {
        respond(&mut stream, 400, &l!("Ungültige Anmeldeantwort."));
        return None;
    };
    respond(&mut stream, 200, &l!("Anmeldung erfolgreich. Du kannst dieses Fenster schließen und zu KeyZapper zurückkehren."));
    Some(Ok(code))
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let html = format!("<!doctype html><meta charset=\"utf-8\"><title>KeyZapper</title><body style=\"font-family:sans-serif;margin:3em\"><p>{message}</p></body>");
    let reason = if status == 200 { "OK" } else { "Error" };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.flush();
}

/// Removes the profile's tokens (sign-out).
pub fn logout(profile: &Profile, store: &TokenStore) -> Result<()> {
    store.delete(&profile.id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SsoStatus {
    pub logged_in: bool,
    pub expires_at: Option<i64>,
}

/// Whether a refresh token exists, and when the cached access token expires.
pub fn status(profile: &Profile, store: &TokenStore) -> Result<SsoStatus> {
    let tokens = store.read(&profile.id)?.filter(|t| !t.refresh_token.is_empty());
    Ok(SsoStatus { logged_in: tokens.is_some(), expires_at: tokens.and_then(|t| t.expires_at) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Handler = Box<dyn Fn(&str, &str) -> (u16, String) + Send>;

    /// Tiny HTTP server on 127.0.0.1; returns the base URL and the received `path body` log.
    fn mock(handler: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let log = Arc::new(Mutex::new(Vec::new()));
        let (thread_log, handler): (_, Handler) = (log.clone(), Box::new(handler));
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, length) = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    data.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&data).into_owned();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let length = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok())).unwrap_or(0usize);
                        break (i + 4, length);
                    }
                    if n == 0 {
                        break (data.len(), 0);
                    }
                };
                while data.len() < head_end + length {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                let path = text.split(' ').nth(1).unwrap_or("").to_string();
                let body = text[head_end.min(text.len())..].to_string();
                thread_log.lock().unwrap().push(format!("{path} {body}"));
                let (status, reply) = handler(&path, &body);
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            }
        });
        (base, log)
    }

    fn idp(token_status: u16, token_body: impl Into<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let token_body = token_body.into();
        let base = Arc::new(Mutex::new(String::new()));
        let shared = base.clone();
        let (url, log) = mock(move |path, _| {
            let base = shared.lock().unwrap().clone();
            if path.ends_with("/.well-known/openid-configuration") {
                (200, format!(r#"{{"issuer":"{base}/","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token"}}"#))
            } else {
                (token_status, token_body.clone())
            }
        });
        *base.lock().unwrap() = url.clone();
        (url, log)
    }

    fn store() -> TokenStore {
        let dir = std::env::temp_dir().join(format!("kz-oidc-{}", uuid::Uuid::new_v4()));
        TokenStore { file: dir.join("sso-tokens.json") }
    }

    fn profile(issuer: &str) -> Profile {
        let mut p = Profile::new(crate::models::new_id(), "SSO".into(), "https://gw.example.test".into(), String::new());
        p.auth_type = crate::models::AuthType::Oidc;
        p.oidc_issuer = Some(issuer.into());
        p.oidc_client_id = Some("client-1".into());
        p
    }

    fn tokens(access: Option<&str>, expires_at: Option<i64>) -> TokenSet {
        TokenSet { refresh_token: "R1".into(), access_token: access.map(String::from), expires_at, token_type: OidcTokenType::Access, expiry_verified: None }
    }

    /// Unsigned JWT with the given payload (KeyZapper only reads `exp`).
    fn jwt(payload: &str) -> String {
        format!("eyJhbGciOiJub25lIn0.{}.sig", B64URL.encode(payload))
    }

    fn token_posts(log: &Arc<Mutex<Vec<String>>>) -> usize {
        log.lock().unwrap().iter().filter(|l| l.starts_with("/token")).count()
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (verifier, challenge) = new_pkce();
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge, pkce_challenge(&verifier));
    }

    #[test]
    fn issuer_must_be_https_or_loopback() {
        assert!(validate_url("https://login.microsoftonline.com/t/v2.0").is_ok());
        assert!(validate_url("http://127.0.0.1:8080/realms/x").is_ok());
        assert!(validate_url("http://localhost/x").is_ok());
        assert!(validate_url("http://idp.example.test").is_err());
        assert!(validate_url("ftp://127.0.0.1").is_err());
    }

    #[test]
    fn discovery_rejects_other_issuer() {
        let (url, _) = mock(|_, _| (200, r#"{"issuer":"https://evil.example","authorization_endpoint":"https://a","token_endpoint":"https://t"}"#.into()));
        assert!(discover(&url).is_err());
        let (url, _) = idp(200, "{}");
        assert!(discover(&url).is_ok());
        assert!(discover(&format!("{url}/")).is_ok());
    }

    #[test]
    fn cached_token_is_used_without_network() {
        let (store, p) = (store(), profile("http://127.0.0.1:1"));
        store.write(&p.id, &tokens(Some("A1"), Some(10_000))).unwrap();
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), "A1");
        // Inside the margin (300 s + 60 s) a refresh is needed, which fails on the closed port without leaking tokens.
        let err = access_token(&p, &store, 10_000 - 359).unwrap_err();
        assert!(matches!(err, Error::Oidc(_)));
        assert!(!err.to_string().contains("R1"));
    }

    #[test]
    fn credential_keeps_expiry_of_the_returned_cached_token() {
        let (store, p) = (store(), profile("http://127.0.0.1:1"));
        let mut cached = tokens(Some("A1"), Some(10_000));
        cached.expiry_verified = Some(true);
        store.write(&p.id, &cached).unwrap();
        let c = gateway_credential(&p, &store, 1_000).unwrap();
        assert_eq!(c.token, "A1");
        assert_eq!(c.expires_at, 10_000);
    }

    #[test]
    fn credential_returns_the_rotated_token_and_its_expiry() {
        let (url, log) = idp(200, r#"{"access_token":"A2","refresh_token":"R2","expires_in":3600}"#);
        let (store, p) = (store(), profile(&url));
        store.write(&p.id, &tokens(Some("A1"), Some(1_100))).unwrap();
        let c = gateway_credential(&p, &store, 1_000).unwrap();
        assert_eq!((c.token.as_str(), c.expires_at), ("A2", 4_600));
        assert_eq!(token_posts(&log), 1);
    }

    #[test]
    fn credential_uses_future_id_token_exp_not_access_token_lifetime() {
        let id = jwt(r#"{"exp":5000,"oid":"o1"}"#);
        let (url, _) = idp(200, format!(r#"{{"access_token":"A2","id_token":"{id}","refresh_token":"R2","expires_in":3600}}"#));
        let (store, mut p) = (store(), profile(&url));
        p.oidc_token_type = OidcTokenType::Id;
        store.write(&p.id, &tokens(None, None)).unwrap();
        let c = gateway_credential(&p, &store, 1_000).unwrap();
        assert_eq!((c.token.as_str(), c.expires_at), (id.as_str(), 5_000));
    }

    #[test]
    fn concurrent_credentials_share_the_rotated_token_and_expiry() {
        let (url, log) = idp(200, r#"{"access_token":"A2","refresh_token":"R2","expires_in":3600}"#);
        let (store, p) = (store(), profile(&url));
        store.write(&p.id, &tokens(Some("A1"), Some(1_100))).unwrap();
        let (store2, p2) = (store.clone(), p.clone());
        let first = std::thread::spawn(move || gateway_credential(&p, &store, 1_000).unwrap());
        let second = std::thread::spawn(move || gateway_credential(&p2, &store2, 1_000).unwrap());
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_eq!((first.token.as_str(), first.expires_at), ("A2", 4_600));
        assert_eq!((first.token.as_str(), first.expires_at), (second.token.as_str(), second.expires_at));
        assert_eq!(token_posts(&log), 1);
    }

    #[test]
    fn strict_credentials_reject_unverified_fallback_expiry_but_legacy_access_token_keeps_it() {
        let (url, log) = idp(200, r#"{"access_token":"A2","refresh_token":"R2"}"#);
        let (store, p) = (store(), profile(&url));
        store.write(&p.id, &tokens(Some("A1"), Some(10_000))).unwrap();
        let err = match gateway_credential(&p, &store, 1_000) {
            Err(err) => err,
            Ok(_) => panic!("strict credentials must reject unverified expiry"),
        };
        assert!(matches!(err, Error::Oidc(_)));
        assert!(!err.to_string().contains("A2") && !err.to_string().contains("R2"));
        assert_eq!(token_posts(&log), 1);
        let saved = store.read(&p.id).unwrap().unwrap();
        assert_eq!(saved.expiry_verified, Some(false));
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), "A2");
    }

    #[test]
    fn nonpositive_lifetime_never_becomes_a_future_legacy_cache() {
        for lifetime in [serde_json::json!(0), serde_json::json!(-1), serde_json::json!("0"), serde_json::json!("-1")] {
            let response = serde_json::json!({"access_token":"opaque", "refresh_token":"synthetic-refresh", "expires_in":lifetime});
            let (url, log) = idp(200, response.to_string());
            let (store, p) = (store(), profile(&url));
            store.write(&p.id, &tokens(None, None)).unwrap();
            assert!(access_token(&p, &store, 1_000).is_ok());
            assert!(store.read(&p.id).unwrap().unwrap().expires_at.unwrap() <= 1_000);
            assert!(access_token(&p, &store, 1_001).is_ok());
            assert_eq!(token_posts(&log), 2, "second legacy access must refresh");
            assert!(gateway_credential(&p, &store, 1_002).is_err());
            assert_eq!(token_posts(&log), 3);
        }
    }

    #[test]
    fn margin_follows_helper_ttl() {
        let mut p = profile("http://127.0.0.1:1");
        assert_eq!(margin_secs(&p), 360);
        p.environment = Some([(TTL_ENV.to_string(), "60000".to_string())].into());
        assert_eq!(margin_secs(&p), 120);
    }

    #[test]
    fn access_expiry_is_verified_only_from_positive_duration_or_future_jwt_exp() {
        let valid = token_response("p", OidcTokenType::Access, 200, &serde_json::json!({"access_token":"opaque","refresh_token":"R","expires_in":60}), None, 1_000).unwrap();
        assert_eq!((valid.expires_at, valid.expiry_verified), (Some(1_060), Some(true)));
        for value in [serde_json::Value::Null, serde_json::json!("bad")] {
            let mut response = serde_json::json!({"access_token":"opaque","refresh_token":"R"});
            if !value.is_null() { response["expires_in"] = value; }
            let saved = token_response("p", OidcTokenType::Access, 200, &response, None, 1_000).unwrap();
            assert_eq!((saved.expires_at, saved.expiry_verified), (Some(4_600), Some(false)));
        }
        // Explicit non-positive lifetimes must not become a positive legacy cache.
        for (value, expected) in [
            (serde_json::json!(0), 1_000), (serde_json::json!("0"), 1_000),
            (serde_json::json!(-1), 999), (serde_json::json!("-1"), 999),
        ] {
            let response = serde_json::json!({"access_token":"opaque","refresh_token":"R","expires_in":value});
            let saved = token_response("p", OidcTokenType::Access, 200, &response, None, 1_000).unwrap();
            assert_eq!((saved.expires_at, saved.expiry_verified), (Some(expected), Some(false)));
        }
        let future = jwt(r#"{"exp":5000}"#);
        let response = serde_json::json!({"access_token":future,"refresh_token":"R","expires_in":"bad"});
        let saved = token_response("p", OidcTokenType::Access, 200, &response, None, 1_000).unwrap();
        assert_eq!((saved.expires_at, saved.expiry_verified), (Some(5_000), Some(true)));
    }

    #[test]
    fn refresh_stores_rotated_token_once() {
        let (url, log) = idp(200, r#"{"access_token":"A2","refresh_token":"R2","expires_in":3600}"#);
        let (store, p) = (store(), profile(&url));
        store.write(&p.id, &tokens(Some("A1"), Some(1_100))).unwrap();
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), "A2");
        assert_eq!(access_token(&p, &store, 1_001).unwrap(), "A2");
        let saved = store.read(&p.id).unwrap().unwrap();
        assert_eq!((saved.refresh_token.as_str(), saved.expires_at), ("R2", Some(4_600)));
        let posts: Vec<_> = log.lock().unwrap().iter().filter(|l| l.starts_with("/token")).cloned().collect();
        assert_eq!(posts.len(), 1);
        assert!(posts[0].contains("grant_type=refresh_token") && posts[0].contains("refresh_token=R1") && posts[0].contains("client_id=client-1"));
    }

    #[test]
    fn refresh_keeps_old_refresh_token_when_not_rotated() {
        let (url, _) = idp(200, r#"{"access_token":"A2","expires_in":3600}"#);
        let (store, p) = (store(), profile(&url));
        store.write(&p.id, &tokens(None, None)).unwrap();
        assert_eq!(access_token(&p, &store, 0).unwrap(), "A2");
        assert_eq!(store.read(&p.id).unwrap().unwrap().refresh_token, "R1");
    }

    #[test]
    fn invalid_grant_and_missing_tokens_mean_session_expired() {
        let (url, _) = idp(400, r#"{"error":"invalid_grant"}"#);
        let (store, p) = (store(), profile(&url));
        assert_eq!(access_token(&p, &store, 0), Err(Error::SessionExpired(p.id.clone())));
        store.write(&p.id, &tokens(None, None)).unwrap();
        assert_eq!(access_token(&p, &store, 0), Err(Error::SessionExpired(p.id.clone())));
        let (url, _) = idp(500, "oops");
        let other = profile(&url);
        store.write(&other.id, &tokens(None, None)).unwrap();
        assert!(matches!(access_token(&other, &store, 0), Err(Error::Oidc(_))));
    }

    #[test]
    fn login_runs_code_flow_with_pkce_and_stores_tokens() {
        let (url, log) = idp(200, r#"{"access_token":"A1","refresh_token":"R1","expires_in":600}"#);
        let (store, p) = (store(), profile(&url));
        login(
            &p,
            &store,
            |auth| {
                let auth = url::Url::parse(auth).unwrap();
                let q: std::collections::HashMap<_, _> = auth.query_pairs().into_owned().collect();
                assert_eq!(q["code_challenge_method"], "S256");
                assert_eq!(q["client_id"], "client-1");
                assert!(q["scope"].contains("offline_access"));
                let callback = format!("{}?code=CODE1&state={}", q["redirect_uri"], q["state"]);
                std::thread::spawn(move || {
                    // Stray request first: must be ignored.
                    let stray = q["redirect_uri"].replace("/callback", "/favicon.ico");
                    let _ = ureq::get(&stray).call();
                    let _ = ureq::get(&callback).call();
                });
                Ok(())
            },
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(store.read(&p.id).unwrap().unwrap().refresh_token, "R1");
        let post = log.lock().unwrap().iter().find(|l| l.starts_with("/token")).cloned().unwrap();
        assert!(post.contains("grant_type=authorization_code") && post.contains("code=CODE1") && post.contains("code_verifier="));
        assert_eq!(status(&p, &store).unwrap().logged_in, true);
        logout(&p, &store).unwrap();
        assert!(!status(&p, &store).unwrap().logged_in);
    }

    #[test]
    fn login_times_out_and_reports_provider_errors() {
        let (url, _) = idp(200, "{}");
        let (store, p) = (store(), profile(&url));
        assert!(login(&p, &store, |_| Ok(()), Duration::from_millis(300)).is_err());
        let result = login(
            &p,
            &store,
            |auth| {
                let q: std::collections::HashMap<_, _> = url::Url::parse(auth).unwrap().query_pairs().into_owned().collect();
                let callback = format!("{}?error=access_denied&state={}", q["redirect_uri"], q["state"]);
                std::thread::spawn(move || {
                    let _ = ureq::get(&callback).call();
                });
                Ok(())
            },
            Duration::from_secs(10),
        );
        assert!(matches!(result, Err(Error::Oidc(m)) if m.contains("access_denied")));
    }

    #[test]
    fn login_without_refresh_token_hints_offline_access() {
        let (url, _) = idp(200, r#"{"access_token":"A1","expires_in":600}"#);
        let (store, p) = (store(), profile(&url));
        let result = login(
            &p,
            &store,
            |auth| {
                let q: std::collections::HashMap<_, _> = url::Url::parse(auth).unwrap().query_pairs().into_owned().collect();
                let callback = format!("{}?code=C&state={}", q["redirect_uri"], q["state"]);
                std::thread::spawn(move || {
                    let _ = ureq::get(&callback).call();
                });
                Ok(())
            },
            Duration::from_secs(10),
        );
        assert!(matches!(result, Err(Error::Oidc(m)) if m.contains("offline_access")));
        assert!(store.read(&p.id).unwrap().is_none());
    }

    #[test]
    fn id_token_profile_refreshes_to_id_token_expiring_per_exp() {
        let id = jwt(r#"{"exp":5000,"oid":"o1"}"#);
        let (url, log) = idp(200, format!(r#"{{"access_token":"A2","id_token":"{id}","refresh_token":"R2","expires_in":3600}}"#));
        let (store, mut p) = (store(), profile(&url));
        p.oidc_token_type = OidcTokenType::Id;
        store.write(&p.id, &tokens(None, None)).unwrap();
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), id);
        let saved = store.read(&p.id).unwrap().unwrap();
        assert_eq!((saved.expires_at, saved.token_type), (Some(5_000), OidcTokenType::Id));
        assert_eq!(access_token(&p, &store, 1_001).unwrap(), id);
        assert_eq!(token_posts(&log), 1);
    }

    #[test]
    fn id_token_profile_fails_without_usable_id_token() {
        let (url, _) = idp(200, r#"{"access_token":"A2","refresh_token":"R2","expires_in":3600}"#);
        let (store, mut p) = (store(), profile(&url));
        p.oidc_token_type = OidcTokenType::Id;
        store.write(&p.id, &tokens(None, None)).unwrap();
        assert!(matches!(access_token(&p, &store, 0), Err(Error::Oidc(m)) if m.contains("openid")));
        let (url, _) = idp(200, format!(r#"{{"id_token":"{}","expires_in":3600}}"#, jwt(r#"{"oid":"o1"}"#)));
        p.oidc_issuer = Some(url);
        assert!(matches!(access_token(&p, &store, 0), Err(Error::Oidc(m)) if m.contains("exp")));
        assert_eq!(store.read(&p.id).unwrap().unwrap(), tokens(None, None));
    }

    #[test]
    fn changed_token_type_never_hands_out_the_old_kind() {
        let id = jwt(r#"{"exp":10000}"#);
        let (url, log) = idp(200, format!(r#"{{"access_token":"A2","id_token":"{id}","expires_in":3600}}"#));
        let (store, mut p) = (store(), profile(&url));
        store.write(&p.id, &tokens(Some("A1"), Some(10_000))).unwrap();
        p.oidc_token_type = OidcTokenType::Id;
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), id);
        p.oidc_token_type = OidcTokenType::Access;
        assert_eq!(access_token(&p, &store, 1_000).unwrap(), "A2");
        assert_eq!(token_posts(&log), 2);
    }

    #[test]
    fn login_stores_id_token_for_id_token_profile() {
        let id = jwt(r#"{"exp":4102444800,"oid":"o1"}"#);
        let (url, _) = idp(200, format!(r#"{{"access_token":"A1","id_token":"{id}","refresh_token":"R1","expires_in":600}}"#));
        let (store, mut p) = (store(), profile(&url));
        p.oidc_token_type = OidcTokenType::Id;
        login(
            &p,
            &store,
            |auth| {
                let q: std::collections::HashMap<_, _> = url::Url::parse(auth).unwrap().query_pairs().into_owned().collect();
                let callback = format!("{}?code=C&state={}", q["redirect_uri"], q["state"]);
                std::thread::spawn(move || {
                    let _ = ureq::get(&callback).call();
                });
                Ok(())
            },
            Duration::from_secs(10),
        )
        .unwrap();
        let saved = store.read(&p.id).unwrap().unwrap();
        assert_eq!((saved.access_token, saved.expires_at, saved.token_type), (Some(id), Some(4_102_444_800), OidcTokenType::Id));
    }
}
