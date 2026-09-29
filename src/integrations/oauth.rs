//! Minimal native OAuth 2.0 Authorization Code + PKCE flow for Tableau connected-app/EAS JWT sign-in.
//! No token cache, client secret, embedded webview, or generic OAuth framework. PAT fallback is implemented separately in rest/config.
use crate::{
    config::OAuthConfig,
    error::{Error, Result, require},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{blocking::Client, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

fn discovery_url(issuer: &str) -> Result<Url> {
    let mut u = Url::parse(issuer).map_err(|_| Error::new("AUTH_CONFIG", "Invalid OAuth issuer"))?;
    {
        let mut p = u.path_segments_mut().map_err(|_| Error::new("AUTH_CONFIG", "Invalid OAuth issuer path"))?;
        p.pop_if_empty().push(".well-known").push("openid-configuration");
    }
    Ok(u)
}

fn fetch_json(client: &Client, url: Url, max: usize) -> Result<Vec<u8>> {
    let mut response = client.get(url).send().map_err(|_| Error::new("OAUTH_DISCOVERY", "OAuth discovery request failed"))?;
    require(response.status().is_success(), "OAUTH_DISCOVERY", format!("OAuth discovery returned HTTP {}", response.status().as_u16()))?;
    let mut data = Vec::new();
    response.by_ref().take(max as u64 + 1).read_to_end(&mut data)?;
    require(data.len() <= max, "OAUTH_DISCOVERY", "OAuth discovery response is too large")?;
    Ok(data)
}

fn pkce() -> (Zeroizing<String>, String, String) {
    let verifier = Zeroizing::new(format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().simple().to_string();
    (verifier, challenge, state)
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    // Browser launchers must never inherit MCP protocol input/output.
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|_| Error::new("OAUTH_BROWSER", "Could not open the system browser for SSO"))?;
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(())
}

fn callback_listener(redirect: &Url) -> Result<TcpListener> {
    let host = redirect.host_str().ok_or_else(|| Error::new("AUTH_CONFIG", "OAuth redirect has no host"))?;
    let port = redirect.port().ok_or_else(|| Error::new("AUTH_CONFIG", "OAuth redirect has no port"))?;
    let listener = TcpListener::bind((host, port)).map_err(|_| Error::new("OAUTH_CALLBACK", "OAuth loopback callback port is unavailable"))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn respond(mut stream: TcpStream, success: bool) {
    let body = if success {
        "<html><body>Authentication complete. You can return to Amazon Quick.</body></html>"
    } else {
        "<html><body>Authentication failed. Return to Amazon Quick for details.</body></html>"
    };
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
        if success { "200 OK" } else { "400 Bad Request" },
        body.len(),
        body
    );
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// Malformed/unrelated loopback requests are rejected locally, not treated as an
// authorization decision. Only a callback bound to our state can deny the flow.
pub(crate) fn callback_code(request: &str, redirect: &Url, expected_state: &str) -> Result<Zeroizing<String>> {
    require(request.ends_with("\r\n\r\n"), "OAUTH_CALLBACK", "Incomplete callback headers")?;
    let first = request.lines().next().unwrap_or("");
    let parts: Vec<_> = first.split_whitespace().collect();
    require(parts.len() == 3 && parts[0] == "GET" && matches!(parts[2], "HTTP/1.0" | "HTTP/1.1"),
        "OAUTH_CALLBACK", "Expected one HTTP GET request")?;
    let target = parts[1];
    require(target.starts_with('/') && !target.starts_with("//") && !target.contains('#'),
        "OAUTH_CALLBACK", "Invalid origin-form callback target")?;
    let mut base = redirect.clone();
    base.set_path("/"); base.set_query(None); base.set_fragment(None);
    let url = base.join(target).map_err(|_| Error::new("OAUTH_CALLBACK", "Invalid callback target"))?;
    require(url.origin() == redirect.origin() && url.path() == redirect.path(),
        "OAUTH_CALLBACK", "Unexpected callback path")?;
    let mut parameters = std::collections::BTreeMap::new();
    for (key, value) in url.query_pairs() {
        require(!parameters.contains_key(key.as_ref()), "OAUTH_CALLBACK", "Duplicate callback parameter")?;
        parameters.insert(key.into_owned(), value.into_owned());
    }
    require(parameters.get("state").map(String::as_str) == Some(expected_state),
        "OAUTH_STATE", "OAuth state mismatch")?;
    require(!(parameters.contains_key("error") && parameters.contains_key("code")),
        "OAUTH_CALLBACK", "Callback contains both code and error")?;
    if parameters.contains_key("error") {
        // Do not expose provider-controlled descriptions or URLs to the model.
        return Err(Error::new("OAUTH_DENIED", "The identity provider denied this authorization request"));
    }
    let code = parameters.remove("code").filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new("OAUTH_CALLBACK", "Authorization code is missing"))?;
    Ok(Zeroizing::new(code))
}

fn parse_callback(stream: &mut TcpStream, redirect: &Url, state: &str,
    deadline: Instant, ct: &CancellationToken) -> Result<Zeroizing<String>> {
    let connection_deadline = std::cmp::min(deadline, Instant::now() + Duration::from_secs(2));
    let mut bytes = Zeroizing::new(Vec::with_capacity(2048));
    let mut buf = [0u8; 1024];
    loop {
        require(!ct.is_cancelled(), "CANCELLED", "OAuth authentication cancelled")?;
        let now = Instant::now();
        require(now < deadline, "OAUTH_TIMEOUT", "Timed out waiting for browser SSO")?;
        require(now < connection_deadline, "OAUTH_CALLBACK", "Incomplete callback timed out")?;
        stream.set_read_timeout(Some(std::cmp::min(connection_deadline - now, Duration::from_millis(100))))?;
        match stream.read(&mut buf) {
            Ok(0) => return Err(Error::new("OAUTH_CALLBACK", "Truncated callback headers")),
            Ok(n) => {
                require(bytes.len() + n <= 16 * 1024, "OAUTH_CALLBACK", "Callback headers exceed limit")?;
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let text = std::str::from_utf8(&bytes[..end + 4])
                        .map_err(|_| Error::new("OAUTH_CALLBACK", "Callback headers are not UTF-8"))?;
                    return callback_code(text, redirect, state);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

fn wait_for_code(listener: &TcpListener, redirect: &Url, state: &str, timeout: Duration,
    ct: &CancellationToken) -> Result<Zeroizing<String>> {
    let deadline = Instant::now().checked_add(timeout)
        .ok_or_else(|| Error::new("AUTH_CONFIG", "OAuth timeout exceeds clock range"))?;
    loop {
        require(!ct.is_cancelled(), "CANCELLED", "OAuth authentication cancelled")?;
        require(Instant::now() < deadline, "OAUTH_TIMEOUT", "Timed out waiting for browser SSO")?;
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() { respond(stream, false); continue; }
                let result = parse_callback(&mut stream, redirect, state, deadline, ct);
                respond(stream, result.is_ok());
                match result {
                    Ok(code) => return Ok(code),
                    Err(e) if matches!(e.code, "CANCELLED" | "OAUTH_TIMEOUT" | "OAUTH_DENIED") => return Err(e),
                    Err(_) => continue,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.into()),
        }
    }
}

fn verified_endpoints(discovery: &Discovery, issuer: &str) -> Result<(Url, Url)> {
    require(discovery.issuer == issuer, "OAUTH_ISSUER", "Discovery issuer does not match the configured issuer")?;
    let endpoint = |raw: &str| -> Result<Url> {
        let u = Url::parse(raw).map_err(|_| Error::new("OAUTH_DISCOVERY", "Invalid OAuth endpoint"))?;
        require(u.scheme() == "https" && u.host_str().is_some() && u.username().is_empty()
            && u.password().is_none() && u.fragment().is_none(), "OAUTH_DISCOVERY", "OAuth endpoint requires HTTPS without credentials or fragments")?;
        Ok(u)
    };
    let authorize = endpoint(&discovery.authorization_endpoint)?;
    let token = endpoint(&discovery.token_endpoint)?;
    for (key, _) in authorize.query_pairs() {
        require(!["response_type", "client_id", "redirect_uri", "scope", "state", "code_challenge", "code_challenge_method"].contains(&key.as_ref()),
            "OAUTH_DISCOVERY", "Authorization endpoint predefines a reserved request parameter")?;
    }
    Ok((authorize, token))
}

/// Acquire one short-lived JWT from the corporate IdP using browser SSO + PKCE.
/// The token is returned only to the Tableau sign-in path and is never cached or exposed to MCP.
pub fn acquire_jwt(client: &Client, cfg: &OAuthConfig, timeout: Duration, ct: &CancellationToken) -> Result<Zeroizing<String>> {
    let discovery: Discovery = crate::wire::decode(&fetch_json(client, discovery_url(&cfg.issuer)?, 256 * 1024)?, 256 * 1024)?;
    let (authorize, token_endpoint) = verified_endpoints(&discovery, &cfg.issuer)?;

    let redirect = Url::parse(&cfg.redirect_uri).map_err(|_| Error::new("AUTH_CONFIG", "Invalid OAuth redirect URI"))?;
    let listener = callback_listener(&redirect)?;
    let (verifier, challenge, state) = pkce();

    let mut auth_url = authorize;
    {
        let mut q = auth_url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &cfg.client_id)
            .append_pair("redirect_uri", &cfg.redirect_uri)
            .append_pair("scope", &cfg.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
    }
    open_browser(auth_url.as_str())?;
    let code = wait_for_code(&listener, &redirect, &state, timeout, ct)?;

    let response = client.post(token_endpoint).form(&[
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ]).send().map_err(|_| Error::new("OAUTH_TOKEN", "OAuth token exchange failed"))?;
    require(response.status().is_success(), "OAUTH_TOKEN", format!("OAuth token endpoint returned HTTP {}", response.status().as_u16()))?;
    let mut reader = response;
    let mut data = Vec::new();
    reader.by_ref().take(256 * 1024 + 1).read_to_end(&mut data)?;
    require(data.len() <= 256 * 1024, "OAUTH_TOKEN", "OAuth token response too large")?;
    let token: TokenResponse = crate::wire::decode(&data, 256 * 1024)?;
    let jwt = Zeroizing::new(token.access_token);
    require(jwt.split('.').count() == 3, "OAUTH_TOKEN", "OAuth access token is not a JWT accepted by Tableau connected-app sign-in")?;
    Ok(jwt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn redirect() -> Url { Url::parse("http://127.0.0.1:8765/callback").unwrap() }
    fn request(query: &str) -> String { format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n") }
    #[test]
    fn callback_requires_state_on_success_and_error() {
        let uri = redirect();
        assert_eq!(callback_code(&request("code=ok&state=expected"), &uri, "expected").unwrap().as_str(), "ok");
        assert_eq!(callback_code(&request("error=access_denied&state=wrong"), &uri, "expected").unwrap_err().code, "OAUTH_STATE");
        assert_eq!(callback_code(&request("error=access_denied&state=expected"), &uri, "expected").unwrap_err().code, "OAUTH_DENIED");
        assert!(callback_code(&request("code=ok"), &uri, "expected").is_err());
    }
    #[test]
    fn callback_rejects_duplicate_or_ambiguous_parameters() {
        for query in ["state=expected&state=expected&code=x", "state=expected&code=a&code=b",
            "state=expected&%63ode=a&code=b", "state=expected&code=a&error=denied", "state=expected&code="] {
            assert!(callback_code(&request(query), &redirect(), "expected").is_err(), "{query}");
        }
    }
    #[test]
    fn callback_rejects_wrong_paths_and_framing() {
        for r in ["GET /favicon.ico HTTP/1.1\r\n\r\n", "POST /callback?state=expected&code=x HTTP/1.1\r\n\r\n",
            "GET //evil.invalid/callback?state=expected&code=x HTTP/1.1\r\n\r\n",
            "GET /callback?state=expected&code=x HTTP/1.1\r\n"] {
            assert!(callback_code(r, &redirect(), "expected").is_err());
        }
    }
    #[test]
    fn discovery_issuer_is_exact_and_endpoints_are_constrained() {
        let mut d = Discovery { issuer:"https://issuer.invalid/tenant".into(),
            authorization_endpoint:"https://login.invalid/authorize".into(), token_endpoint:"https://login.invalid/token".into() };
        assert!(verified_endpoints(&d, &d.issuer).is_ok());
        assert_eq!(verified_endpoints(&d, "https://issuer.invalid/other").unwrap_err().code, "OAUTH_ISSUER");
        d.token_endpoint = "http://login.invalid/token".into(); assert!(verified_endpoints(&d, &d.issuer).is_err());
        d.token_endpoint = "https://user:secret@login.invalid/token".into(); assert!(verified_endpoints(&d, &d.issuer).is_err());
        d.token_endpoint = "https://login.invalid/token".into();
        d.authorization_endpoint.push_str("?state=attacker"); assert!(verified_endpoints(&d, &d.issuer).is_err());
    }
    #[test]
    fn pkce_is_bounded_and_challenge_matches_verifier() {
        let (verifier, challenge, state) = pkce();
        assert!((43..=128).contains(&verifier.len()));
        assert!(verifier.bytes().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())));
        assert!(!challenge.contains('=')); assert!(state.len() >= 32);
    }
    #[test]
    fn unrelated_callback_does_not_abort_login() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let uri = Url::parse(&format!("http://{address}/callback")).unwrap();
        let sender = std::thread::spawn(move || {
            for r in ["GET /favicon.ico HTTP/1.1\r\n\r\n".to_owned(), request("state=correct&code=valid")] {
                let mut s = TcpStream::connect(address).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                s.write_all(r.as_bytes()).unwrap();
                let mut response = Vec::new(); let _ = s.read_to_end(&mut response);
            }
        });
        let result = wait_for_code(&listener, &uri, "correct", Duration::from_secs(3), &CancellationToken::new());
        sender.join().unwrap(); assert_eq!(result.unwrap().as_str(), "valid");
    }
    #[test]
    fn slow_callback_obeys_global_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        let start = Instant::now();
        let error = parse_callback(&mut s, &redirect(), "state", start + Duration::from_millis(150),
            &CancellationToken::new()).unwrap_err();
        assert_eq!(error.code, "OAUTH_TIMEOUT");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn callback_wait_observes_cancellation_without_network() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap();
        let ct = CancellationToken::new(); ct.cancel();
        assert_eq!(wait_for_code(&listener, &redirect(), "s", Duration::from_secs(1), &ct).unwrap_err().code, "CANCELLED");
    }
}
