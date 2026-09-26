use crate::error::{Error, Result, require};
use clap::{Args, ValueEnum};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    /// All agent file paths are relative to this pre-existing directory.
    pub workspace: PathBuf,
    pub limits: Limits,
    pub policy: Policy,
    pub tableau: Option<TableauConfig>,
    pub hyper: Option<HyperConfig>,
}

#[derive(Clone)]
pub struct Limits {
    pub xml_bytes: u64,
    pub file_bytes: u64,
    pub zip_entries: usize,
    pub zip_expanded_bytes: u64,
    pub xml_nodes: u32,
    pub input_json_bytes: u64,
    pub plan_json_bytes: u64,
    pub result_bytes: usize,
    pub rows: usize,
    pub request_seconds: u64,
    pub hyper_seconds: u64,
    pub max_search_pages: u32,
    pub max_operations: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            xml_bytes: 64 << 20,
            file_bytes: 2 << 30,
            zip_entries: 4096,
            zip_expanded_bytes: 8 << 30,
            xml_nodes: 1_000_000,
            input_json_bytes: 2 << 20,
            plan_json_bytes: 32 << 20,
            result_bytes: 4 << 20,
            rows: 1000,
            request_seconds: 120,
            hyper_seconds: 30,
            max_search_pages: 20,
            max_operations: 100,
        }
    }
}

#[derive(Clone, Default)]
pub struct Policy {
    pub publish_enabled: bool,
    pub allow_overwrite: bool,
    pub require_publish_tests: bool,
    pub allow_data_output: bool,
    pub allow_unverified_formula_edits: bool,
}

#[derive(Clone)]
pub struct TableauConfig {
    pub server: String,
    pub site: String,
    pub api_version: String,
    pub auth: TableauAuth,
    pub ca_certificate: Option<PathBuf>,
    pub publish_projects: Vec<String>,
}

#[derive(Clone)]
pub enum TableauAuth {
    OAuth(OAuthConfig),
    Pat(PatConfig),
}

impl TableauAuth {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::OAuth(_) => "oauth_pkce_connected_app",
            Self::Pat(_) => "pat",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TableauAuthMode {
    Oauth,
    Pat,
}

#[derive(Clone)]
pub struct OAuthConfig {
    /// OIDC issuer trusted by Tableau's OAuth 2.0 connected-app/EAS configuration.
    pub issuer: String,
    /// Public/native OAuth client. tabkit deliberately does not accept a client secret.
    pub client_id: String,
    /// Fixed loopback callback registered for the OAuth client.
    pub redirect_uri: String,
    /// Requested scopes. They must be accepted by the IdP and by Tableau's connected-app policy.
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub struct PatConfig {
    /// PAT display/name configured in Tableau for the user.
    pub name: String,
    /// Name of the environment variable holding the PAT secret. The secret itself is never accepted as an MCP argument.
    pub secret_env: String,
}

// Retained in startup configuration; fields are read by the optional native profile.
#[cfg_attr(not(feature = "hyper"), allow(dead_code))]
#[derive(Clone)]
pub struct HyperConfig {
    pub runtime_directory: PathBuf,
    pub memory_limit: String,
}

/// All operator configuration is startup configuration. Amazon Quick passes these values
/// in the local MCP command's args. PAT mode reads the secret from the named environment variable.
#[derive(Clone, Args)]
pub struct StartupArgs {
    #[arg(long)]
    pub workspace: PathBuf,

    #[arg(long, global = true)]
    pub tableau_server: Option<String>,
    #[arg(long, global = true, default_value = "")]
    pub tableau_site: String,
    #[arg(long, global = true, default_value = "3.25")]
    pub tableau_api_version: String,
    #[arg(long, global = true, value_enum, default_value = "oauth")]
    pub tableau_auth: TableauAuthMode,
    #[arg(long, global = true)]
    pub oauth_issuer: Option<String>,
    #[arg(long, global = true)]
    pub oauth_client_id: Option<String>,
    #[arg(long, global = true, default_value = "http://127.0.0.1:8765/callback")]
    pub oauth_redirect_uri: String,
    /// Repeat or comma-separate. If omitted, tabkit requests the scopes needed by its v1 REST surface.
    #[arg(long = "oauth-scope", global = true, value_delimiter = ',')]
    pub oauth_scopes: Vec<String>,
    #[arg(long, global = true)]
    pub tableau_pat_name: Option<String>,
    /// Name of the PAT secret environment variable, not its value. An env value stored in Quick config is still a stored secret.
    #[arg(long, global = true, default_value = "TABLEAU_PAT_SECRET")]
    pub tableau_pat_secret_env: String,
    #[arg(long, global = true)]
    pub ca_certificate: Option<PathBuf>,
    #[arg(long = "publish-project", global = true, value_delimiter = ',')]
    pub publish_projects: Vec<String>,

    #[arg(long, global = true)]
    pub publish_enabled: bool,
    #[arg(long, global = true)]
    pub allow_overwrite: bool,
    #[arg(long, global = true)]
    pub require_publish_tests: bool,
    #[arg(long, global = true)]
    pub allow_data_output: bool,
    #[arg(long, global = true)]
    pub allow_unverified_formula_edits: bool,

    #[arg(long, global = true)]
    pub hyper_runtime_directory: Option<PathBuf>,
    #[arg(long, global = true, default_value = "512M")]
    pub hyper_memory_limit: String,

    #[arg(long, global = true, default_value_t = 64u64 << 20)]
    pub xml_bytes: u64,
    #[arg(long, global = true, default_value_t = 2u64 << 30)]
    pub file_bytes: u64,
    #[arg(long, global = true, default_value_t = 4096)]
    pub zip_entries: usize,
    #[arg(long, global = true, default_value_t = 8u64 << 30)]
    pub zip_expanded_bytes: u64,
    #[arg(long, global = true, default_value_t = 1_000_000)]
    pub xml_nodes: u32,
    #[arg(long, global = true, default_value_t = 2u64 << 20)]
    pub input_json_bytes: u64,
    #[arg(long, global = true, default_value_t = 32u64 << 20)]
    pub plan_json_bytes: u64,
    #[arg(long, global = true, default_value_t = 4usize << 20)]
    pub result_bytes: usize,
    #[arg(long, global = true, default_value_t = 1000)]
    pub rows: usize,
    #[arg(long, global = true, default_value_t = 120)]
    pub request_seconds: u64,
    #[arg(long, global = true, default_value_t = 30)]
    pub hyper_seconds: u64,
    #[arg(long, global = true, default_value_t = 20)]
    pub max_search_pages: u32,
    #[arg(long, global = true, default_value_t = 100)]
    pub max_operations: usize,
}

fn default_oauth_scopes() -> Vec<String> {
    [
        "openid",
        "profile",
        "email",
        "tableau:content:read",
        "tableau:workbooks:create",
        "tableau:workbooks:update",
        "tableau:workbooks:download",
        "tableau:views:download",
        "tableau:file_uploads:create",
        "tableau:jobs:read",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn https_url(value: &str, what: &str) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(value).map_err(|_| Error::new("CONFIG", format!("Invalid {what} URL")))?;
    require(u.scheme() == "https" && u.host_str().is_some(), "CONFIG", format!("{what} requires HTTPS"))?;
    require(u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none(), "CONFIG", format!("Credentials/query/fragment in {what} URL are forbidden"))?;
    Ok(u)
}

impl Config {
    pub fn from_startup(a: StartupArgs) -> Result<Self> {
        require(a.workspace.is_absolute(), "CONFIG", "workspace must be an absolute path")?;
        let workspace = a.workspace.canonicalize()?;
        require(workspace.is_dir(), "CONFIG", "workspace is not a directory")?;

        let limits = Limits {
            xml_bytes: a.xml_bytes,
            file_bytes: a.file_bytes,
            zip_entries: a.zip_entries,
            zip_expanded_bytes: a.zip_expanded_bytes,
            xml_nodes: a.xml_nodes,
            input_json_bytes: a.input_json_bytes,
            plan_json_bytes: a.plan_json_bytes,
            result_bytes: a.result_bytes,
            rows: a.rows,
            request_seconds: a.request_seconds,
            hyper_seconds: a.hyper_seconds,
            max_search_pages: a.max_search_pages,
            max_operations: a.max_operations,
        };
        require(limits.xml_bytes > 0 && limits.xml_bytes <= u32::MAX as u64, "CONFIG", "xml_bytes must fit u32")?;
        require(limits.file_bytes >= limits.xml_bytes && limits.file_bytes <= 16u64 << 30, "CONFIG", "file_bytes must be between xml_bytes and 16 GiB")?;
        require(limits.xml_nodes > 0 && limits.zip_entries > 0 && limits.max_operations > 0, "CONFIG", "zero structural limit")?;
        require(limits.rows > 0 && limits.rows <= 100_000 && (8192..=64 * 1024 * 1024).contains(&limits.result_bytes), "CONFIG", "invalid result limits")?;
        require((1..=3600).contains(&limits.request_seconds) && (1..=300).contains(&limits.hyper_seconds), "CONFIG", "invalid timeout")?;
        require((128..=8 * 1024 * 1024).contains(&limits.input_json_bytes) && (8192..=256 * 1024 * 1024).contains(&limits.plan_json_bytes), "CONFIG", "invalid JSON limits")?;
        require(limits.xml_nodes <= 5_000_000 && limits.zip_entries <= 10_000 && limits.max_operations <= 1000, "CONFIG", "structural limits are too high")?;
        require((1..=1000).contains(&limits.max_search_pages), "CONFIG", "invalid max_search_pages")?;

        let policy = Policy {
            publish_enabled: a.publish_enabled,
            allow_overwrite: a.allow_overwrite,
            require_publish_tests: a.require_publish_tests,
            allow_data_output: a.allow_data_output,
            allow_unverified_formula_edits: a.allow_unverified_formula_edits,
        };

        let tableau = match a.tableau_server {
            None => {
                require(a.oauth_issuer.is_none() && a.oauth_client_id.is_none() && a.tableau_pat_name.is_none(), "CONFIG", "Tableau auth args require --tableau-server")?;
                None
            }
            Some(server) => {
                https_url(&server, "Tableau server")?;
                require(matches!(a.tableau_api_version.as_str(), "3.25" | "3.27"), "CONFIG", "Choose Tableau Server 2025.1 (3.25) or 2025.3 (3.27)")?;
                let auth = match a.tableau_auth {
                    TableauAuthMode::Oauth => {
                        require(a.tableau_pat_name.is_none(), "CONFIG", "PAT args are not valid with --tableau-auth oauth")?;
                        let issuer = a.oauth_issuer.ok_or_else(|| Error::new("CONFIG", "--oauth-issuer is required with --tableau-auth oauth"))?;
                        https_url(&issuer, "OAuth issuer")?;
                        let client_id = a.oauth_client_id.ok_or_else(|| Error::new("CONFIG", "--oauth-client-id is required with --tableau-auth oauth"))?;
                        require(!client_id.trim().is_empty(), "CONFIG", "OAuth client id must not be empty")?;
                        let redirect = reqwest::Url::parse(&a.oauth_redirect_uri).map_err(|_| Error::new("CONFIG", "Invalid OAuth redirect URI"))?;
                        require(redirect.scheme() == "http", "CONFIG", "OAuth native callback must use loopback HTTP")?;
                        require(matches!(redirect.host_str(), Some("127.0.0.1") | Some("localhost")), "CONFIG", "OAuth redirect must use a loopback host")?;
                        require(redirect.port().is_some(), "CONFIG", "OAuth redirect URI must contain an explicit port")?;
                        require(redirect.query().is_none() && redirect.fragment().is_none(), "CONFIG", "OAuth redirect URI must not contain query/fragment")?;
                        let scopes = if a.oauth_scopes.is_empty() { default_oauth_scopes() } else { a.oauth_scopes };
                        require(scopes.iter().all(|s| !s.trim().is_empty() && !s.chars().any(char::is_whitespace)), "CONFIG", "OAuth scopes must be non-empty tokens")?;
                        TableauAuth::OAuth(OAuthConfig { issuer, client_id, redirect_uri: a.oauth_redirect_uri, scopes })
                    }
                    TableauAuthMode::Pat => {
                        require(a.oauth_issuer.is_none() && a.oauth_client_id.is_none() && a.oauth_scopes.is_empty(), "CONFIG", "OAuth args are not valid with --tableau-auth pat")?;
                        let name = a.tableau_pat_name.ok_or_else(|| Error::new("CONFIG", "--tableau-pat-name is required with --tableau-auth pat"))?;
                        require(!name.trim().is_empty(), "CONFIG", "PAT name must not be empty")?;
                        require(!a.tableau_pat_secret_env.trim().is_empty() && a.tableau_pat_secret_env.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()), "CONFIG", "PAT secret env name must be an ASCII environment variable name")?;
                        TableauAuth::Pat(PatConfig { name, secret_env: a.tableau_pat_secret_env })
                    }
                };
                Some(TableauConfig {
                    server,
                    site: a.tableau_site,
                    api_version: a.tableau_api_version,
                    auth,
                    ca_certificate: a.ca_certificate,
                    publish_projects: a.publish_projects,
                })
            }
        };

        let hyper = match a.hyper_runtime_directory {
            None => None,
            Some(runtime_directory) => {
                require(runtime_directory.is_absolute(), "CONFIG", "Hyper runtime path must be absolute")?;
                require(!a.hyper_memory_limit.is_empty() && a.hyper_memory_limit.len() < 32, "CONFIG", "Invalid Hyper memory limit")?;
                Some(HyperConfig { runtime_directory, memory_limit: a.hyper_memory_limit })
            }
        };

        Ok(Self { workspace, limits, policy, tableau, hyper })
    }
}
