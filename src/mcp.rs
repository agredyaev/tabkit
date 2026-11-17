//! MCP server integration built on the `rmcp` Rust implementation.
//!
//! This module owns workbook sessions and maps MCP tools to package, workbook,
//! lineage, edit, validation, and publish operations. MCP framing and transport
//! semantics are provided by `rmcp`, not reimplemented here.

use crate::{
    Error, Result as TapResult,
    edit::{self, Batch},
    lineage::{Direction, Kind, Lineage},
    package::{Kind as PackageKind, Limits, Package},
    rest::{self, Rest},
    workbook::{self, Workbook},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

struct SessionPermit {
    slots: Arc<AtomicUsize>,
}

impl Drop for SessionPermit {
    fn drop(&mut self) {
        let previous = self.slots.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "session permit counter underflow");
    }
}

struct Session {
    path: PathBuf,
    owned_path: bool,
    package: Package,
    workbook: Workbook,
    lineage: Lineage,
    _permit: SessionPermit,
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.owned_path {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct OwnedPathCleanup(Option<PathBuf>);

impl OwnedPathCleanup {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for OwnedPathCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

const MAX_SESSIONS: usize = 64;

#[derive(Clone)]
struct HttpAuth {
    token: Arc<str>,
}

async fn require_http_bearer(
    axum::extract::State(auth): axum::extract::State<HttpAuth>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> std::result::Result<axum::response::Response, axum::http::StatusCode> {
    let expected = format!("Bearer {}", auth.token);
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    if !authorized {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

struct State {
    limits: Limits,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    session_slots: Arc<AtomicUsize>,
    tableau: Mutex<Option<Arc<Rest>>>,
    tableau_config: Option<rest::Config>,
    workspace: tempfile::TempDir,
    allow_local_paths: bool,
}

/// MCP server implementation and shared session state.
#[derive(Clone)]
pub struct TapKitMcp {
    state: Arc<State>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchWorkbooksArgs {
    /// Exact or partial Tableau workbook name.
    query: String,
    /// Maximum number of matches to return. Defaults to 20, maximum 100.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct OpenWorkbookArgs {
    /// Tableau workbook LUID. Use this for Amazon Quick/server workflows.
    workbook_id: Option<String>,
    /// Local TWB/TWBX path. Disabled for remote HTTP unless explicitly allowed.
    path: Option<String>,
    /// Include extracts when downloading from Tableau. Defaults to false.
    include_extract: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct InspectArgs {
    /// Workbook handle returned by workbook_open or workbook_edit.
    handle: String,
    /// datasource, connection, relation, field, calculation, worksheet, or dashboard.
    kind: String,
    /// Optional exact/substring name filter.
    query: Option<String>,
    /// Maximum objects returned. Defaults to 100, maximum 500.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LineageArgs {
    /// Workbook handle returned by workbook_open or workbook_edit.
    handle: String,
    /// Object name/caption to trace.
    query: String,
    /// Optional object kind to disambiguate.
    kind: Option<String>,
    /// upstream or downstream. Defaults to downstream.
    direction: Option<String>,
    /// Maximum nodes returned. Defaults to 200, maximum 1000.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditArgs {
    /// Source workbook handle.
    handle: String,
    /// Semantic calculation/dashboard edit batch.
    batch: Batch,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct HandleArgs {
    /// Workbook/candidate handle.
    handle: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JobArgs {
    /// Tableau background job LUID returned by workbook_publish.
    job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PublishArgs {
    /// Candidate workbook handle.
    handle: String,
    /// Published workbook name.
    name: String,
    /// Target Tableau project LUID.
    project_id: String,
    /// Whether an existing workbook with the same name may be overwritten.
    #[serde(default)]
    overwrite: bool,
}

impl TapKitMcp {
    /// Creates an MCP server with optional Tableau REST configuration.
    pub fn new(tableau_config: Option<rest::Config>, allow_local_paths: bool) -> TapResult<Self> {
        if let Some(config) = &tableau_config {
            config.validate()?;
        }
        Ok(Self {
            state: Arc::new(State {
                limits: Limits::default(),
                sessions: RwLock::new(HashMap::new()),
                session_slots: Arc::new(AtomicUsize::new(0)),
                tableau: Mutex::new(None),
                tableau_config,
                workspace: tempfile::tempdir()?,
                allow_local_paths,
            }),
        })
    }

    fn with_rest<T>(&self, f: impl FnOnce(&Rest) -> TapResult<T>) -> TapResult<T> {
        let rest = {
            let mut slot = self
                .state
                .tableau
                .lock()
                .map_err(|_| Error::new("INTERNAL", "Tableau client lock is poisoned"))?;
            if slot.is_none() {
                let config = self.state.tableau_config.clone().ok_or_else(|| {
                    Error::new(
                        "TABLEAU_NOT_CONFIGURED",
                        "Tableau-backed tools are unavailable because this MCP server has no Tableau REST configuration",
                    )
                })?;
                *slot = Some(Arc::new(Rest::sign_in(config)?));
            }
            slot.as_ref()
                .cloned()
                .ok_or_else(|| Error::new("INTERNAL", "Tableau client was not initialized"))?
        };
        f(&rest)
    }

    async fn blocking<T, F>(&self, operation: F) -> std::result::Result<T, McpError>
    where
        T: Send + 'static,
        F: FnOnce(TapKitMcp) -> TapResult<T> + Send + 'static,
    {
        let server = self.clone();
        tokio::task::spawn_blocking(move || operation(server))
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?
            .map_err(tool_error)
    }

    fn session(&self, handle: &str) -> TapResult<Arc<Session>> {
        self.state
            .sessions
            .read()
            .map_err(|_| Error::new("INTERNAL", "Workbook cache lock is poisoned"))?
            .get(handle)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "HANDLE_NOT_FOUND",
                    format!("Unknown workbook handle {handle:?}"),
                )
            })
    }

    fn reserve_session(&self) -> TapResult<SessionPermit> {
        self.state
            .session_slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < MAX_SESSIONS).then_some(current + 1)
            })
            .map_err(|_| {
                Error::new(
                    "SESSION_LIMIT",
                    format!("Workbook handle limit {MAX_SESSIONS} reached; close unused handles"),
                )
            })?;
        Ok(SessionPermit {
            slots: Arc::clone(&self.state.session_slots),
        })
    }

    fn insert_path(&self, path: PathBuf, owned_path: bool) -> TapResult<Value> {
        let permit = self.reserve_session()?;
        self.insert_path_reserved(path, owned_path, permit)
    }

    fn insert_path_reserved(
        &self,
        path: PathBuf,
        owned_path: bool,
        permit: SessionPermit,
    ) -> TapResult<Value> {
        let mut cleanup = OwnedPathCleanup(owned_path.then(|| path.clone()));
        let (package, twb) = Package::open(&path, &self.state.limits)?;
        let workbook = Workbook::open(twb)?;
        cleanup.disarm();
        self.insert_session_reserved(path, owned_path, permit, package, workbook)
    }

    fn insert_session_reserved(
        &self,
        path: PathBuf,
        owned_path: bool,
        permit: SessionPermit,
        package: Package,
        workbook: Workbook,
    ) -> TapResult<Value> {
        let mut cleanup = OwnedPathCleanup(owned_path.then(|| path.clone()));
        let lineage = Lineage::build(&workbook)?;
        let summary = workbook.summary();
        let gaps = lineage.gaps.len();
        let handle = format!("wb_{}", uuid::Uuid::new_v4().simple());

        self.state
            .sessions
            .write()
            .map_err(|_| Error::new("INTERNAL", "Workbook cache lock is poisoned"))?
            .insert(
                handle.clone(),
                Arc::new(Session {
                    path,
                    owned_path,
                    package,
                    workbook,
                    lineage,
                    _permit: permit,
                }),
            );
        cleanup.disarm();

        Ok(json!({
            "handle": handle,
            "summary": summary,
            "lineage_gaps": gaps
        }))
    }

    fn open_workbook(&self, args: OpenWorkbookArgs) -> TapResult<Value> {
        match (args.workbook_id, args.path) {
            (Some(workbook_id), None) => {
                let permit = self.reserve_session()?;
                let (path, receipt) = self.with_rest(|rest| {
                    rest.download_workbook_auto(
                        &workbook_id,
                        args.include_extract.unwrap_or(false),
                        self.state.workspace.path(),
                        &self.state.limits,
                    )
                })?;
                let mut result = self.insert_path_reserved(path, true, permit)?;
                if let Value::Object(object) = &mut result {
                    object.insert("download".into(), serde_json::to_value(receipt)?);
                    object.insert("tableau_workbook_id".into(), Value::String(workbook_id));
                }
                Ok(result)
            }
            (None, Some(path)) => {
                if !self.state.allow_local_paths {
                    return Err(Error::new(
                        "LOCAL_PATHS_DISABLED",
                        "This MCP server does not accept local workbook paths; use workbook_id or start the server with local paths enabled",
                    ));
                }
                self.insert_path(PathBuf::from(path), false)
            }
            _ => Err(Error::new(
                "ARGUMENT",
                "Provide exactly one of workbook_id or path",
            )),
        }
    }

    fn close_workbook(&self, handle: &str) -> TapResult<Value> {
        let removed = self
            .state
            .sessions
            .write()
            .map_err(|_| Error::new("INTERNAL", "Workbook cache lock is poisoned"))?
            .remove(handle)
            .is_some();
        Ok(json!({ "handle": handle, "closed": removed }))
    }

    fn edit_workbook(&self, args: EditArgs) -> TapResult<Value> {
        let source = self.session(&args.handle)?;
        let permit = self.reserve_session()?;
        let candidate = edit::apply(&source.workbook, &args.batch, self.state.limits.xml_bytes)?;
        let extension = match source.package.kind {
            PackageKind::Twb => "twb",
            PackageKind::Twbx => "twbx",
        };
        let output = self.state.workspace.path().join(format!(
            "candidate-{}.{}",
            uuid::Uuid::new_v4().simple(),
            extension
        ));
        let edit::Candidate { workbook, receipt } = candidate;
        let package = source.package.write_candidate(
            &output,
            workbook.xml.source().as_bytes(),
            &self.state.limits,
        )?;
        let package_sha256 = package.sha256.clone();
        let mut result = self.insert_session_reserved(output, true, permit, package, workbook)?;
        if let Value::Object(object) = &mut result {
            object.insert("source_handle".into(), Value::String(args.handle));
            object.insert("package_sha256".into(), Value::String(package_sha256));
            object.insert("edit".into(), serde_json::to_value(receipt)?);
        }
        Ok(result)
    }
}

#[tool_router]
impl TapKitMcp {
    #[tool(
        description = "Find Tableau workbooks by name on the configured Tableau Server 2026.2.4 site."
    )]
    async fn tableau_search_workbooks(
        &self,
        Parameters(args): Parameters<SearchWorkbooksArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| {
                let limit = args.limit.unwrap_or(20);
                server
                    .with_rest(|rest| rest.search_workbooks(&args.query, limit))
                    .and_then(|items| serde_json::to_value(items).map_err(Into::into))
            })
            .await?;
        json_result(&value)
    }

    #[tool(
        description = "Open and index a Tableau workbook. Use workbook_id to download from Tableau Server or path for an explicitly enabled local file. Returns a reusable in-memory handle."
    )]
    async fn workbook_open(
        &self,
        Parameters(args): Parameters<OpenWorkbookArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| server.open_workbook(args))
            .await?;
        json_result(&value)
    }

    #[tool(
        description = "Inspect compact typed workbook objects without returning raw Tableau XML."
    )]
    fn workbook_inspect(
        &self,
        Parameters(args): Parameters<InspectArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let limit = args.limit.unwrap_or(100);
        if !(1..=500).contains(&limit) {
            return Err(McpError::invalid_params(
                "Inspect limit must be 1..500",
                None,
            ));
        }
        let session = self.session(&args.handle).map_err(tool_error)?;
        let value =
            workbook::inspect_limited(&session.workbook, &args.kind, args.query.as_deref(), limit)
                .map_err(tool_error)?;
        json_result(&value)
    }

    #[tool(
        description = "Trace bounded upstream or downstream Tableau workbook lineage from a named object."
    )]
    fn workbook_lineage(
        &self,
        Parameters(args): Parameters<LineageArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let session = self.session(&args.handle).map_err(tool_error)?;
        let kind = args
            .kind
            .as_deref()
            .map(|value| value.parse::<Kind>())
            .transpose()
            .map_err(tool_error)?;
        let matches = session.lineage.find_kind(&args.query, kind);
        if matches.len() != 1 {
            let candidates = serde_json::to_value(&matches)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            return Err(McpError::invalid_params(
                if matches.is_empty() {
                    format!("No lineage object matches {:?}", args.query)
                } else {
                    format!("Lineage object {:?} is ambiguous: {candidates}", args.query)
                },
                None,
            ));
        }

        let direction = match args.direction.as_deref().unwrap_or("downstream") {
            "upstream" => Direction::Upstream,
            "downstream" => Direction::Downstream,
            other => {
                return Err(McpError::invalid_params(
                    format!("Unknown lineage direction {other:?}"),
                    None,
                ));
            }
        };
        let limit = args.limit.unwrap_or(200);
        if !(1..=1000).contains(&limit) {
            return Err(McpError::invalid_params(
                "Lineage limit must be 1..1000",
                None,
            ));
        }
        let trace = session
            .lineage
            .trace_limited(matches[0].id, direction, limit)
            .ok_or_else(|| McpError::internal_error("Lineage node disappeared", None))?;
        json_result(&trace)
    }

    #[tool(
        description = "Apply a semantic calculation/dashboard edit batch locally and return a new candidate workbook handle. The source workbook is never overwritten."
    )]
    async fn workbook_edit(
        &self,
        Parameters(args): Parameters<EditArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| server.edit_workbook(args))
            .await?;
        json_result(&value)
    }

    #[tool(
        description = "Run Tableau Server 2026.2.4 syntactic and semantic validation for the workbook XML. For a TWBX handle, TapKit sends the embedded TWB to Tableau validateWorkbook; packaged resources are checked by the publish path."
    )]
    async fn workbook_validate(
        &self,
        Parameters(args): Parameters<HandleArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| {
                let session = server.session(&args.handle)?;
                server
                    .with_rest(|rest| rest.validate_workbook(&session.path, &server.state.limits))
                    .and_then(|validation| serde_json::to_value(validation).map_err(Into::into))
            })
            .await?;
        json_result(&value)
    }

    #[tool(
        description = "Publish a candidate workbook to Tableau Server. Set overwrite=true only when replacing an existing workbook is intended."
    )]
    async fn workbook_publish(
        &self,
        Parameters(args): Parameters<PublishArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| {
                let session = server.session(&args.handle)?;
                server.with_rest(|rest| {
                    rest.publish_workbook(
                        &session.path,
                        &args.name,
                        &args.project_id,
                        args.overwrite,
                        &server.state.limits,
                    )
                })
            })
            .await?;
        json_result(&value)
    }

    #[tool(description = "Get Tableau background job status by job LUID.")]
    async fn tableau_job(
        &self,
        Parameters(args): Parameters<JobArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self
            .blocking(move |server| server.with_rest(|rest| rest.job(&args.job_id)))
            .await?;
        json_result(&value)
    }

    #[tool(
        description = "Release an in-memory workbook handle when it is no longer needed. User-provided local source files are never deleted; TapKit-owned download/candidate files are removed after the final in-flight session reference is released."
    )]
    fn workbook_close(
        &self,
        Parameters(args): Parameters<HandleArgs>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let value = self.close_workbook(&args.handle).map_err(tool_error)?;
        json_result(&value)
    }
}

#[tool_handler]
impl ServerHandler for TapKitMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "TapKit is a headless Tableau Server 2026.2.4 workbook runtime. Open a workbook once, then use compact inspect/lineage/edit tools; do not request or generate raw Tableau XML.".to_owned(),
            )
    }
}

fn tool_error(error: Error) -> McpError {
    let message = format!("{}: {}", error.code, error.message);
    let data = Some(json!({ "tapkit_code": error.code }));

    if error.code == "HANDLE_NOT_FOUND" {
        return McpError::resource_not_found(message, data);
    }

    if matches!(
        error.code,
        "ARGUMENT"
            | "AMBIGUOUS_PACKAGE"
            | "CALCULATION"
            | "CALCULATION_EXISTS"
            | "DASHBOARD"
            | "DASHBOARD_EXISTS"
            | "DATASOURCE_NOT_FOUND"
            | "DUPLICATE_EDIT"
            | "DUPLICATE_STYLE"
            | "EMPTY_BATCH"
            | "ENCODING"
            | "FIELD_NOT_FOUND"
            | "FILE_TYPE"
            | "FILTER"
            | "FILTER_FIELD_NOT_FOUND"
            | "FILTER_INSTANCE_AMBIGUOUS"
            | "FILTER_INSTANCE_NOT_FOUND"
            | "FILTER_NOT_ON_SHEET"
            | "FORMAT"
            | "FORMULA"
            | "LAYOUT"
            | "LIMIT"
            | "LOCAL_PATHS_DISABLED"
            | "OUTPUT_EXISTS"
            | "PATH"
            | "REFERENCE"
            | "STALE_BASE"
            | "STALE_PRECONDITION"
            | "STYLE"
            | "UNSAFE_FORMULA"
            | "VERSION"
            | "WORKBOOK_SHAPE"
            | "WORKSHEET_NOT_FOUND"
            | "XML"
            | "XML_SHAPE"
            | "ZIP_DUPLICATE"
            | "ZIP_LIMIT"
            | "ZIP_PATH"
            | "ZIP_SYMLINK"
    ) {
        McpError::invalid_params(message, data)
    } else {
        McpError::internal_error(message, data)
    }
}

fn json_result(value: &impl serde::Serialize) -> std::result::Result<CallToolResult, McpError> {
    let text = serde_json::to_string(value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

pub async fn serve_stdio(server: TapKitMcp) -> TapResult<()> {
    use rmcp::{ServiceExt, transport::stdio};

    let service = server
        .serve(stdio())
        .await
        .map_err(|error| Error::new("MCP", error.to_string()))?;
    service
        .waiting()
        .await
        .map_err(|error| Error::new("MCP", error.to_string()))?;
    Ok(())
}

fn http_router(server: TapKitMcp, bearer_token: Option<String>) -> axum::Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };

    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    if let Some(token) = bearer_token {
        router.layer(axum::middleware::from_fn_with_state(
            HttpAuth {
                token: Arc::<str>::from(token),
            },
            require_http_bearer,
        ))
    } else {
        router
    }
}

pub async fn serve_http(
    server: TapKitMcp,
    bind: std::net::SocketAddr,
    bearer_token: Option<String>,
) -> TapResult<()> {
    let router = http_router(server, bearer_token);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| Error::new("MCP_BIND", error.to_string()))?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|error| Error::new("MCP", error.to_string()))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    fn request(body: Value, session: Option<&str>, bearer: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost:8000")
            .header("mcp-protocol-version", "2025-03-26");
        if let Some(session) = session {
            builder = builder.header("mcp-session-id", session);
        }
        if let Some(bearer) = bearer {
            builder = builder.header("authorization", format!("Bearer {bearer}"));
        }
        builder
            .body(Body::from(body.to_string()))
            .expect("HTTP request")
    }

    async fn json_response(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("response body");
        serde_json::from_slice(&bytes).expect("JSON response")
    }

    #[test]
    fn owned_session_file_lives_until_last_arc_and_then_is_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workbook_path = temp.path().join("owned.twb");
        std::fs::write(
            &workbook_path,
            br#"<workbook version="26.2" source-build="20262.test"/>"#,
        )
        .expect("fixture");

        let server = TapKitMcp::new(None, true).expect("MCP server");
        let opened = server
            .insert_path(workbook_path.clone(), true)
            .expect("owned workbook");
        let handle = opened["handle"].as_str().expect("handle").to_owned();
        let held = server.session(&handle).expect("session");

        assert_eq!(server.state.session_slots.load(Ordering::Acquire), 1);
        assert!(workbook_path.exists());
        let closed = server.close_workbook(&handle).expect("close");
        assert_eq!(closed["closed"], true);
        assert!(
            workbook_path.exists(),
            "in-flight Arc<Session> must keep the owned file alive"
        );
        assert_eq!(
            server.state.session_slots.load(Ordering::Acquire),
            1,
            "closing the handle must not release a slot held by an in-flight Arc"
        );

        drop(held);
        assert_eq!(server.state.session_slots.load(Ordering::Acquire), 0);
        assert!(
            !workbook_path.exists(),
            "owned file must be removed when the final Session Arc drops"
        );
    }

    #[test]
    fn failed_owned_session_insert_cleans_up_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workbook_path = temp.path().join("invalid-owned.twb");
        std::fs::write(
            &workbook_path,
            br#"<workbook version="26.3" source-build="20263.test"/>"#,
        )
        .expect("fixture");

        let server = TapKitMcp::new(None, true).expect("MCP server");
        let error = server
            .insert_path(workbook_path.clone(), true)
            .expect_err("26.3 must be rejected");
        assert_eq!(error.code, "VERSION");
        assert_eq!(server.state.session_slots.load(Ordering::Acquire), 0);
        assert!(
            !workbook_path.exists(),
            "failed owned insert must not leak a workspace file"
        );
    }

    #[test]
    fn session_permits_enforce_the_hard_limit_and_recover_on_drop() {
        let server = TapKitMcp::new(None, true).expect("MCP server");
        let mut permits = Vec::with_capacity(MAX_SESSIONS);
        for _ in 0..MAX_SESSIONS {
            permits.push(server.reserve_session().expect("session permit"));
        }

        assert_eq!(
            server.state.session_slots.load(Ordering::Acquire),
            MAX_SESSIONS
        );
        let error = match server.reserve_session() {
            Ok(_) => panic!("session limit was not enforced"),
            Err(error) => error,
        };
        assert_eq!(error.code, "SESSION_LIMIT");

        drop(permits.pop());
        assert_eq!(
            server.state.session_slots.load(Ordering::Acquire),
            MAX_SESSIONS - 1
        );
        permits.push(
            server
                .reserve_session()
                .expect("slot should be reusable after permit drop"),
        );
        assert_eq!(
            server.state.session_slots.load(Ordering::Acquire),
            MAX_SESSIONS
        );
    }

    #[tokio::test]
    async fn streamable_http_requires_bearer_and_preserves_handles_across_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workbook_path = temp.path().join("fixture.twb");
        std::fs::write(
            &workbook_path,
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Sales">
      <table><view><datasource-dependencies datasource="Orders">
        <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      </datasource-dependencies></view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
  </worksheets>
</workbook>"#,
        )
        .expect("fixture");

        let server = TapKitMcp::new(None, true).expect("MCP server");
        let router = http_router(server, Some("secret".into()));
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }
        });

        let unauthorized = router
            .clone()
            .oneshot(request(initialize.clone(), None, None))
            .await
            .expect("unauthorized response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = router
            .clone()
            .oneshot(request(initialize, None, Some("secret")))
            .await
            .expect("initialize response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers().get("mcp-session-id").is_none(),
            "stateless HTTP must not allocate transport sessions"
        );
        let initialized = json_response(response).await;
        assert_eq!(initialized["id"], 1);

        let notification = router
            .clone()
            .oneshot(request(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                None,
                Some("secret"),
            ))
            .await
            .expect("initialized notification");
        assert!(notification.status().is_success());

        let tools_response = router
            .clone()
            .oneshot(request(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
                None,
                Some("secret"),
            ))
            .await
            .expect("tools/list response");
        assert_eq!(tools_response.status(), StatusCode::OK);
        let tools_json = json_response(tools_response).await;
        let tools = tools_json
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tools array");
        assert_eq!(tools.len(), 9);
        for tool in tools {
            assert_eq!(
                tool.pointer("/inputSchema/type").and_then(Value::as_str),
                Some("object"),
                "tool has invalid input schema: {tool}"
            );
        }
        let edit_tool = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some("workbook_edit"))
            .expect("workbook_edit schema");
        let edit_schema = edit_tool["inputSchema"].to_string();
        for expected in [
            "view_styles",
            "fixed_size",
            "multiple_dropdown",
            "all_database",
            "fit_width",
        ] {
            assert!(
                edit_schema.contains(expected),
                "workbook_edit schema does not expose {expected}: {edit_schema}"
            );
        }

        let open_response = router
            .clone()
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":3,
                    "method":"tools/call",
                    "params":{
                        "name":"workbook_open",
                        "arguments":{"path":workbook_path.to_string_lossy()}
                    }
                }),
                None,
                Some("secret"),
            ))
            .await
            .expect("workbook_open response");
        let open_json = json_response(open_response).await;
        let open_text = open_json
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .expect("open text");
        let opened: Value = serde_json::from_str(open_text).expect("open JSON");
        let handle = opened["handle"].as_str().expect("handle").to_owned();

        let inspect_response = router
            .clone()
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":4,
                    "method":"tools/call",
                    "params":{
                        "name":"workbook_inspect",
                        "arguments":{"handle":handle,"kind":"worksheet","query":"Sales"}
                    }
                }),
                None,
                Some("secret"),
            ))
            .await
            .expect("workbook_inspect response");
        let inspect_json = json_response(inspect_response).await;
        let inspect_text = inspect_json
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .expect("inspect text");
        let inspected: Value = serde_json::from_str(inspect_text).expect("inspect JSON");
        assert_eq!(inspected["count"], 1);

        let close_response = router
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":5,
                    "method":"tools/call",
                    "params":{
                        "name":"workbook_close",
                        "arguments":{"handle":handle}
                    }
                }),
                None,
                Some("secret"),
            ))
            .await
            .expect("workbook_close response");
        let close_json = json_response(close_response).await;
        let close_text = close_json
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .expect("close text");
        let closed: Value = serde_json::from_str(close_text).expect("close JSON");
        assert_eq!(closed["closed"], true);
    }

    #[test]
    fn tool_errors_preserve_tapkit_code_and_use_json_rpc_categories() {
        let missing = tool_error(Error::new(
            "HANDLE_NOT_FOUND",
            "Unknown workbook handle \"wb_missing\"",
        ));
        assert_eq!(missing.code, rmcp::model::ErrorCode::RESOURCE_NOT_FOUND);
        assert_eq!(
            missing
                .data
                .as_ref()
                .and_then(|value| value.get("tapkit_code"))
                .and_then(Value::as_str),
            Some("HANDLE_NOT_FOUND")
        );

        let invalid = tool_error(Error::new(
            "WORKSHEET_NOT_FOUND",
            "Worksheet \"Missing\" does not exist in this workbook",
        ));
        assert_eq!(invalid.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(invalid.message.contains("WORKSHEET_NOT_FOUND"));

        let internal = tool_error(Error::new("INTERNAL", "Workbook cache lock is poisoned"));
        assert_eq!(internal.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(
            internal
                .data
                .as_ref()
                .and_then(|value| value.get("tapkit_code"))
                .and_then(Value::as_str),
            Some("INTERNAL")
        );
    }
}
