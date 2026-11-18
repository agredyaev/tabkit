use clap::{Parser, ValueEnum};
use std::{env, net::SocketAddr, time::Duration};
use tabkit::{
    Error,
    mcp::{self, TabKitMcp},
    rest,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

#[derive(Parser)]
#[command(name = "tabkit-mcp")]
#[command(about = "MCP server for Tableau 2026.2.4 workbooks")]
struct Args {
    /// MCP transport. HTTP serves the MCP endpoint at /mcp.
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    transport: Transport,

    /// HTTP bind address. Used only with --transport http.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,

    /// Allow workbook_open(path=...) for this server. Stdio always allows local paths.
    #[arg(long)]
    allow_local_paths: bool,

    /// Environment variable containing an optional HTTP bearer token.
    #[arg(long, default_value = "TABKIT_MCP_TOKEN")]
    http_bearer_token_env: String,

    /// Explicitly allow unauthenticated HTTP on a non-loopback address.
    #[arg(long)]
    allow_unauthenticated_http: bool,

    /// Tableau Server/Cloud base URL. Omit with --tableau-pat-name for local-only tools.
    #[arg(long, env = "TABLEAU_SERVER")]
    tableau_server: Option<String>,

    /// Tableau site content URL. Empty selects the default site.
    #[arg(long, env = "TABLEAU_SITE", default_value = "")]
    tableau_site: String,

    /// Tableau Personal Access Token name. Must be supplied with --tableau-server.
    #[arg(long, env = "TABLEAU_PAT_NAME")]
    tableau_pat_name: Option<String>,

    /// Environment variable containing the Tableau PAT secret.
    #[arg(
        long,
        env = "TABLEAU_PAT_SECRET_ENV",
        default_value = "TABLEAU_PAT_SECRET"
    )]
    tableau_pat_secret_env: String,

    /// Tableau HTTP request timeout in seconds.
    #[arg(long, default_value_t = 120)]
    tableau_timeout_seconds: u64,
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
    let tableau_config = match (args.tableau_server, args.tableau_pat_name) {
        (Some(server), Some(pat_name)) => Some(rest::Config {
            server,
            site: args.tableau_site,
            api_version: tabkit::REST_API_VERSION.into(),
            pat_name,
            pat_secret_env: args.tableau_pat_secret_env,
            timeout: Duration::from_secs(args.tableau_timeout_seconds),
        }),
        (None, None) => None,
        _ => {
            return Err(Error::new(
                "CONFIG",
                "TABLEAU_SERVER and TABLEAU_PAT_NAME must be provided together",
            ));
        }
    };

    let allow_local_paths = args.allow_local_paths || matches!(args.transport, Transport::Stdio);
    let server = TabKitMcp::new(tableau_config, allow_local_paths)?;

    let bearer_token = match env::var(&args.http_bearer_token_env) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        Ok(_) | Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(Error::new("CONFIG", error.to_string())),
    };

    match args.transport {
        Transport::Stdio => mcp::serve_stdio(server).await,
        Transport::Http => {
            if !args.bind.ip().is_loopback()
                && bearer_token.is_none()
                && !args.allow_unauthenticated_http
            {
                return Err(Error::new(
                    "MCP_AUTH",
                    "Refusing unauthenticated HTTP on a non-loopback address; configure TABKIT_MCP_TOKEN or pass --allow-unauthenticated-http only when access is restricted outside TabKit",
                ));
            }
            mcp::serve_http(server, args.bind, bearer_token).await
        }
    }
}
