use crate::api::{AccessTokens, ApiError, agent, percent_encode};
use crate::keychain;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ClientSecrets {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
}

#[derive(Deserialize)]
struct ClientFile {
    installed: ClientSecrets,
}

pub struct Grant {
    pub refresh_token: String,
    pub scope: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub fn load_client(path: &Path) -> Result<ClientSecrets> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read OAuth client file {}", path.display()))?;
    warn_if_readable_by_others(path);
    let file: ClientFile = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} isn't a Desktop OAuth client file (expected an \"installed\" key)",
            path.display()
        )
    })?;
    Ok(file.installed)
}

#[cfg(unix)]
fn warn_if_readable_by_others(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        eprintln!(
            "warning: {} is readable by other users; run: chmod 600 {}",
            path.display(),
            path.display()
        );
    }
}

#[cfg(not(unix))]
fn warn_if_readable_by_others(_path: &Path) {}

pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_b64<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("no system randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_pair() -> (String, String) {
    let verifier = random_b64::<48>();
    let challenge = challenge_for(&verifier);
    (verifier, challenge)
}

pub fn authorize_url(
    c: &ClientSecrets,
    redirect: &str,
    scopes: &[&str],
    challenge: &str,
    state: &str,
    login_hint: Option<&str>,
) -> String {
    let scope = scopes.join(" ");
    let mut params = vec![
        ("client_id", c.client_id.as_str()),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", scope.as_str()),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    if let Some(hint) = login_hint {
        params.push(("login_hint", hint));
    }
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect();
    format!("{}?{}", c.auth_uri, query.join("&"))
}

pub fn missing_scopes(granted: &str, required: &[&str]) -> Vec<String> {
    let granted: Vec<&str> = granted.split_whitespace().collect();
    required
        .iter()
        .filter(|s| !granted.contains(s))
        .map(|s| s.to_string())
        .collect()
}

pub fn run_loopback_flow(
    c: &ClientSecrets,
    scopes: &[&str],
    login_hint: Option<&str>,
) -> Result<Grant> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let (verifier, challenge) = pkce_pair();
    let state = random_b64::<16>();
    let url = authorize_url(c, &redirect, scopes, &challenge, &state, login_hint);

    eprintln!("Opening your browser to sign in. If it doesn't open, visit:\n{url}\n");
    let _ = std::process::Command::new("open").arg(&url).status();

    let params = wait_for_redirect(&listener)?;
    if let Some(err) = params.get("error") {
        bail!("Google sign-in failed: {err}");
    }
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        bail!("sign-in response didn't match this request; try again");
    }
    let code = params
        .get("code")
        .ok_or_else(|| anyhow!("Google didn't return a code"))?;

    let resp = exchange(
        c,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
        ],
    )
    .map_err(|e| anyhow!("{e}"))?;
    Ok(Grant {
        refresh_token: resp
            .refresh_token
            .ok_or_else(|| anyhow!("Google didn't return a refresh token"))?,
        scope: resp.scope.unwrap_or_default(),
    })
}

fn wait_for_redirect(listener: &TcpListener) -> Result<HashMap<String, String>> {
    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let path = line.split_whitespace().nth(1).unwrap_or("");
        let params = parse_query(path.split_once('?').map(|(_, q)| q).unwrap_or(""));
        if params.contains_key("code") || params.contains_key("error") {
            let body = "<html><body>calendar-autoblock: you can close this tab.</body></html>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )?;
            return Ok(params);
        }
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
    }
    bail!("sign-in listener closed")
}

fn parse_query(q: &str) -> HashMap<String, String> {
    form_urlencoded::parse(q.as_bytes()).into_owned().collect()
}

fn exchange(c: &ClientSecrets, extra: &[(&str, &str)]) -> Result<TokenResponse, ApiError> {
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", &c.client_id),
        ("client_secret", &c.client_secret),
    ];
    form.extend_from_slice(extra);
    let mut resp = agent()
        .post(&c.token_uri)
        .send_form(form)
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let body: TokenResponse = resp
        .body_mut()
        .read_json()
        .map_err(|e| ApiError::Other(format!("unreadable token response ({status}): {e}")))?;
    if status >= 500 {
        return Err(ApiError::Other(format!(
            "Google token endpoint returned {status}"
        )));
    }
    if let Some(err) = &body.error {
        let detail = body.error_description.clone().unwrap_or_default();
        return Err(ApiError::Other(format!("{err}: {detail}")));
    }
    Ok(body)
}

pub struct TokenStore {
    client: ClientSecrets,
    cache: HashMap<String, (String, Instant)>,
}

impl TokenStore {
    pub fn new(client: ClientSecrets) -> Self {
        Self {
            client,
            cache: HashMap::new(),
        }
    }
}

impl AccessTokens for TokenStore {
    fn access_token(&mut self, account: &str) -> Result<String, ApiError> {
        if let Some((token, expires)) = self.cache.get(account)
            && *expires > Instant::now() + Duration::from_secs(60)
        {
            return Ok(token.clone());
        }
        let auth_err = |message: String| ApiError::Auth {
            account: account.to_string(),
            message,
        };
        let refresh = keychain::load_refresh_token(account)
            .map_err(|e| auth_err(format!("couldn't read Keychain: {e}")))?
            .ok_or_else(|| auth_err("not signed in".to_string()))?;
        let resp = exchange(
            &self.client,
            &[("grant_type", "refresh_token"), ("refresh_token", &refresh)],
        )
        .map_err(|e| match e {
            ApiError::Other(m) if m.starts_with("invalid_grant") => auth_err(m),
            other => other,
        })?;
        let token = resp
            .access_token
            .ok_or_else(|| ApiError::Other("token response had no access_token".into()))?;
        let expires = Instant::now() + Duration::from_secs(resp.expires_in.unwrap_or(3600));
        self.cache
            .insert(account.to_string(), (token.clone(), expires));
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn client() -> ClientSecrets {
        load_client(Path::new("tests/fixtures/client_secret_desktop.json")).unwrap()
    }

    #[test]
    fn loads_desktop_client_file() {
        let c = client();
        assert_eq!(c.client_id, "1234-fake.apps.googleusercontent.com");
        assert_eq!(c.client_secret, "GOCSPX-fake");
        assert_eq!(c.auth_uri, "https://accounts.google.com/o/oauth2/auth");
        assert_eq!(c.token_uri, "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (verifier, challenge) = pkce_pair();
        assert_eq!(verifier.len(), 64);
        assert_eq!(challenge, challenge_for(&verifier));
    }

    #[test]
    fn authorize_url_has_pkce_and_offline() {
        let url = authorize_url(
            &client(),
            "http://127.0.0.1:5555",
            &crate::config::DEST_SCOPES,
            "CHAL",
            "STATE",
            Some("person@example.org"),
        );
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/auth?"));
        for part in [
            "response_type=code",
            "access_type=offline",
            "prompt=consent",
            "code_challenge=CHAL",
            "code_challenge_method=S256",
            "state=STATE",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A5555",
            "client_id=1234-fake.apps.googleusercontent.com",
            "login_hint=person%40example.org",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.events.owned",
        ] {
            assert!(url.contains(part), "missing {part} in {url}");
        }
    }

    #[test]
    fn missing_scopes_reports_unchecked_scope() {
        let granted = "https://www.googleapis.com/auth/calendar.events.owned.readonly";
        assert_eq!(
            missing_scopes(granted, &crate::config::SOURCE_SCOPES),
            vec!["https://www.googleapis.com/auth/calendar.calendarlist.readonly".to_string()]
        );
        let all = crate::config::SOURCE_SCOPES.join(" ");
        assert!(missing_scopes(&all, &crate::config::SOURCE_SCOPES).is_empty());
    }
}
