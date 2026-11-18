use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clap::Parser;
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};
use tabkit::{
    Error,
    extension_security::{EXTENSION_SIGNING_KEY_ENV, signing_key_from_env, verify_bundle},
};

const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;
const MAX_VERIFY_BODY_BYTES: usize = 3 * 1024 * 1024;

const HOST_HTML: &str = include_str!("../extension/host.html");
const HOST_CSS: &str = include_str!("../extension/host.css");
const HOST_JS: &str = include_str!("../extension/host.js");
const TABLEAU_JS: &str = include_str!("../extension/vendor/tableau.extensions.1.17.0.min.js");

#[derive(Parser)]
#[command(name = "tabkit-extension-host")]
#[command(about = "HTTP host and bundle verifier for TabKit Dashboard Extensions")]
struct Args {
    /// HTTP bind address. Put TLS in front of this listener for production.
    #[arg(long, default_value = "127.0.0.1:8765")]
    bind: SocketAddr,

    /// URL prefix used by a reverse proxy, for example /tabkit.
    #[arg(long, default_value = "")]
    base_path: String,

    /// Environment variable containing a base64 32-byte dashboard signing key.
    #[arg(long, default_value = EXTENSION_SIGNING_KEY_ENV)]
    signing_key_env: String,
}

#[derive(Clone)]
struct HostState {
    signing_key: Option<Arc<Vec<u8>>>,
    unsigned_localhost: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyRequest {
    bundle_b64: String,
    mac: String,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{}: {}", error.code, error.message);
        std::process::exit(1);
    }
}

async fn run() -> tabkit::Result<()> {
    let args = Args::parse();
    let base_path = normalize_base_path(&args.base_path)?;
    let signing_key = signing_key_from_env(&args.signing_key_env)?;
    if !args.bind.ip().is_loopback() && signing_key.is_none() {
        return Err(Error::new(
            "EXTENSION_HOST_AUTH",
            format!(
                "{} is required when the extension host binds to a non-loopback address",
                args.signing_key_env
            ),
        ));
    }

    let state = HostState {
        signing_key: signing_key.map(Arc::new),
        unsigned_localhost: args.bind.ip().is_loopback(),
    };

    let app = host_router(state, &base_path);

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .map_err(|error| {
            Error::new(
                "EXTENSION_HOST_BIND",
                format!("Failed to bind extension host to {}: {error}", args.bind),
            )
        })?;

    println!(
        "TabKit extension host listening on http://{}{}",
        args.bind, base_path
    );
    axum::serve(listener, app)
        .await
        .map_err(|error| Error::new("EXTENSION_HOST", error.to_string()))
}

fn host_router(state: HostState, base_path: &str) -> Router {
    let app = Router::new()
        .route("/", get(host_html))
        .route("/host.html", get(host_html))
        .route("/host.css", get(host_css))
        .route("/host.js", get(host_js))
        .route("/vendor/tableau.extensions.1.17.0.min.js", get(tableau_js))
        .route("/verify", post(verify))
        .route("/healthz", get(health))
        .layer(DefaultBodyLimit::max(MAX_VERIFY_BODY_BYTES))
        .with_state(state);

    if base_path.is_empty() {
        app
    } else {
        Router::new().nest(base_path, app)
    }
}

fn normalize_base_path(value: &str) -> tabkit::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(String::new());
    }
    if !trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.contains('?')
        || trimmed.contains('#')
    {
        return Err(Error::new(
            "CONFIG",
            "base_path must start with '/', must not end with '/', and must not contain '?' or '#'",
        ));
    }
    if trimmed.split('/').any(|part| part == "." || part == "..") {
        return Err(Error::new(
            "CONFIG",
            "base_path must not contain dot segments",
        ));
    }
    Ok(trimmed.to_owned())
}

async fn host_html() -> Response<Body> {
    static_response("text/html; charset=utf-8", HOST_HTML, true)
}

async fn host_css() -> Response<Body> {
    static_response("text/css; charset=utf-8", HOST_CSS, false)
}

async fn host_js() -> Response<Body> {
    static_response("text/javascript; charset=utf-8", HOST_JS, false)
}

async fn tableau_js() -> Response<Body> {
    static_response("text/javascript; charset=utf-8", TABLEAU_JS, false)
}

async fn health() -> Response<Body> {
    static_response("text/plain; charset=utf-8", "ok\n", false)
}

async fn verify(
    State(state): State<HostState>,
    Json(request): Json<VerifyRequest>,
) -> impl IntoResponse {
    if request.bundle_b64.len() > MAX_VERIFY_BODY_BYTES {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    let bundle = match BASE64.decode(&request.bundle_b64) {
        Ok(bundle) if bundle.len() <= MAX_BUNDLE_BYTES => bundle,
        Ok(_) => return StatusCode::PAYLOAD_TOO_LARGE,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    match state.signing_key.as_deref() {
        Some(key) => match verify_bundle(&bundle, &request.mac, key) {
            Ok(true) => StatusCode::NO_CONTENT,
            Ok(false) | Err(_) => StatusCode::UNAUTHORIZED,
        },
        None if state.unsigned_localhost && request.mac.is_empty() => StatusCode::NO_CONTENT,
        None => StatusCode::UNAUTHORIZED,
    }
}

fn static_response(content_type: &'static str, body: &'static str, html: bool) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=()",
        ),
    );
    if html {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-src 'self' blob: data: about:; img-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
            ),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_path_is_bounded() -> tabkit::Result<()> {
        assert_eq!(normalize_base_path("")?, "");
        assert_eq!(normalize_base_path("/")?, "");
        assert_eq!(normalize_base_path("/tabkit")?, "/tabkit");
        assert!(normalize_base_path("tabkit").is_err());
        assert!(normalize_base_path("/tabkit/").is_err());
        assert!(normalize_base_path("/../tabkit").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn verify_enforces_payload_encoding_size_and_unsigned_policy() {
        let signed = HostState {
            signing_key: Some(Arc::new(vec![7_u8; 32])),
            unsigned_localhost: false,
        };
        let malformed = verify(
            State(signed.clone()),
            Json(VerifyRequest {
                bundle_b64: "%%%".into(),
                mac: String::new(),
            }),
        )
        .await
        .into_response();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

        let encoded_too_large = verify(
            State(signed.clone()),
            Json(VerifyRequest {
                bundle_b64: "A".repeat(MAX_VERIFY_BODY_BYTES + 1),
                mac: String::new(),
            }),
        )
        .await
        .into_response();
        assert_eq!(encoded_too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let decoded_too_large = verify(
            State(signed),
            Json(VerifyRequest {
                bundle_b64: BASE64.encode(vec![0_u8; MAX_BUNDLE_BYTES + 1]),
                mac: String::new(),
            }),
        )
        .await
        .into_response();
        assert_eq!(decoded_too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let bundle = BASE64.encode(br#"{"runtime_version":1}"#);
        let unsigned_allowed = verify(
            State(HostState {
                signing_key: None,
                unsigned_localhost: true,
            }),
            Json(VerifyRequest {
                bundle_b64: bundle.clone(),
                mac: String::new(),
            }),
        )
        .await
        .into_response();
        assert_eq!(unsigned_allowed.status(), StatusCode::NO_CONTENT);

        let unsigned_with_mac = verify(
            State(HostState {
                signing_key: None,
                unsigned_localhost: true,
            }),
            Json(VerifyRequest {
                bundle_b64: bundle.clone(),
                mac: "unexpected".into(),
            }),
        )
        .await
        .into_response();
        assert_eq!(unsigned_with_mac.status(), StatusCode::UNAUTHORIZED);

        let unsigned_disabled = verify(
            State(HostState {
                signing_key: None,
                unsigned_localhost: false,
            }),
            Json(VerifyRequest {
                bundle_b64: bundle,
                mac: String::new(),
            }),
        )
        .await
        .into_response();
        assert_eq!(unsigned_disabled.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn base_path_rejects_query_fragment_and_dot_segments() -> tabkit::Result<()> {
        for value in [
            "tabkit",
            "/tabkit/",
            "/a/../b",
            "/a/./b",
            "/tabkit?x=1",
            "/tabkit#fragment",
        ] {
            let error = match normalize_base_path(value) {
                Ok(_) => {
                    return Err(Error::new(
                        "TEST",
                        format!("invalid base path accepted: {value:?}"),
                    ));
                }
                Err(error) => error,
            };
            assert_eq!(error.code, "CONFIG", "{value:?}");
        }
        assert_eq!(normalize_base_path(" /tabkit ")?, "/tabkit");
        Ok(())
    }

    #[tokio::test]
    async fn router_enforces_security_headers_content_types_and_base_path() -> tabkit::Result<()> {
        use axum::{body::to_bytes, http::Request};
        use tower::ServiceExt;

        let app = host_router(
            HostState {
                signing_key: None,
                unsigned_localhost: true,
            },
            "/tabkit",
        );

        let outside = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(outside.status(), StatusCode::NOT_FOUND);

        for (path, content_type, expect_csp) in [
            ("/tabkit/host.html", "text/html; charset=utf-8", true),
            ("/tabkit/host.css", "text/css; charset=utf-8", false),
            ("/tabkit/host.js", "text/javascript; charset=utf-8", false),
            (
                "/tabkit/vendor/tableau.extensions.1.17.0.min.js",
                "text/javascript; charset=utf-8",
                false,
            ),
            ("/tabkit/healthz", "text/plain; charset=utf-8", false),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .map_err(|error| Error::new("TEST", error.to_string()))?,
                )
                .await
                .map_err(|never| -> Error { match never {} })?;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some(content_type),
                "{path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::X_CONTENT_TYPE_OPTIONS)
                    .and_then(|value| value.to_str().ok()),
                Some("nosniff"),
                "{path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::REFERRER_POLICY)
                    .and_then(|value| value.to_str().ok()),
                Some("no-referrer"),
                "{path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("no-store"),
                "{path}"
            );
            assert_eq!(
                response
                    .headers()
                    .contains_key(header::CONTENT_SECURITY_POLICY),
                expect_csp,
                "{path}"
            );
        }

        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/tabkit/missing")
                    .body(Body::empty())
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(missing.into_body(), 1024)
            .await
            .map_err(|error| Error::new("TEST", error.to_string()))?;
        assert!(body.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn router_rejects_malformed_verify_requests_and_wrong_method() -> tabkit::Result<()> {
        use axum::http::Request;
        use tower::ServiceExt;

        let app = host_router(
            HostState {
                signing_key: None,
                unsigned_localhost: true,
            },
            "",
        );

        let malformed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

        let missing_content_type = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/verify")
                    .body(Body::from(r#"{"bundle_b64":"","mac":""}"#))
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(
            missing_content_type.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );

        let wrong_method = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/verify")
                    .body(Body::empty())
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
        Ok(())
    }

    #[tokio::test]
    async fn router_rejects_oversized_verify_http_body_before_handler() -> tabkit::Result<()> {
        use axum::http::Request;
        use tower::ServiceExt;

        let app = host_router(
            HostState {
                signing_key: None,
                unsigned_localhost: true,
            },
            "",
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("x".repeat(MAX_VERIFY_BODY_BYTES + 1024)))
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        Ok(())
    }

    #[tokio::test]
    async fn router_serves_assets_and_verifies_signed_bundle() -> tabkit::Result<()> {
        use axum::{body::to_bytes, http::Request};
        use tabkit::extension_security::sign_bundle;
        use tower::ServiceExt;

        let key = vec![7_u8; 32];
        let app = host_router(
            HostState {
                signing_key: Some(Arc::new(key.clone())),
                unsigned_localhost: false,
            },
            "/tabkit",
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tabkit/healthz")
                    .body(Body::empty())
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16)
            .await
            .map_err(|error| Error::new("TEST", error.to_string()))?;
        assert_eq!(body.as_ref(), b"ok\n");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tabkit/host.html")
                    .body(Body::empty())
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert!(
            response
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY)
        );
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .map_err(|error| Error::new("TEST", error.to_string()))?;
        assert!(
            std::str::from_utf8(&body)
                .map_err(|error| Error::new("TEST", error.to_string()))?
                .contains("tableau.extensions.1.17.0.min.js")
        );

        let bundle = br#"{"runtime_version":1,"html":"<main/>","css":"","javascript":""}"#;
        let mac = sign_bundle(bundle, &key)?;
        let good = serde_json::to_vec(&serde_json::json!({
            "bundle_b64": BASE64.encode(bundle),
            "mac": mac,
        }))?;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tabkit/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(good))
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let bad = serde_json::to_vec(&serde_json::json!({
            "bundle_b64": BASE64.encode(bundle),
            "mac": "AAAA",
        }))?;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tabkit/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(bad))
                    .map_err(|error| Error::new("TEST", error.to_string()))?,
            )
            .await
            .map_err(|never| -> Error { match never {} })?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        Ok(())
    }
}
