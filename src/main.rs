use clap::{Args, Parser, Subcommand};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tabkit::{
    Error,
    edit::{self, Batch},
    lineage::{Direction, Kind, Lineage},
    package::{Limits, Package},
    rest,
    workbook::{self, Workbook},
};

#[derive(Parser)]
#[command(name = "tabkit")]
#[command(about = "Inspect, edit, validate, and publish Tableau 2026.2.4 workbooks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a local TWB/TWBX and print the indexed workbook summary.
    Open {
        /// Local TWB/TWBX path.
        input: PathBuf,
    },
    /// Inspect indexed workbook objects without returning raw Tableau XML.
    Inspect {
        /// Local TWB/TWBX path.
        input: PathBuf,
        /// datasource, connection, relation, field, calculation, parameter, worksheet, or dashboard.
        #[arg(long)]
        kind: String,
        /// Optional exact/substring object-name filter.
        #[arg(long)]
        query: Option<String>,
    },
    /// Build lineage or trace one named object.
    Lineage {
        /// Local TWB/TWBX path.
        input: PathBuf,
        /// Object name/caption to trace. Omit to return the complete lineage graph.
        #[arg(long)]
        from: Option<String>,
        /// Optional lineage object kind used to disambiguate --from.
        #[arg(long)]
        kind: Option<String>,
        /// Trace dependencies upstream instead of consumers downstream.
        #[arg(long)]
        upstream: bool,
    },
    /// Apply a JSON semantic edit batch and write a new TWB/TWBX candidate.
    Edit {
        /// Source TWB/TWBX path.
        input: PathBuf,
        /// JSON file containing a TabKit edit batch.
        spec: PathBuf,
        /// Candidate output path. Existing files are not overwritten.
        output: PathBuf,
    },
    /// Download a workbook from Tableau by LUID.
    Download {
        #[command(flatten)]
        tableau: TableauArgs,
        /// Tableau workbook LUID.
        workbook_id: String,
        /// Local output path.
        output: PathBuf,
        /// Include workbook extracts in the download.
        #[arg(long, default_value_t = true)]
        include_extract: bool,
    },
    /// Submit a local workbook to Tableau REST validateWorkbook.
    Validate {
        #[command(flatten)]
        tableau: TableauArgs,
        /// Local TWB/TWBX path.
        input: PathBuf,
    },
    /// Publish a local workbook to a Tableau project.
    Publish {
        #[command(flatten)]
        tableau: TableauArgs,
        /// Local TWB/TWBX path.
        input: PathBuf,
        /// Published workbook name.
        #[arg(long)]
        name: String,
        /// Target Tableau project LUID.
        #[arg(long)]
        project_id: String,
        /// Allow overwrite of an existing workbook with the same name.
        #[arg(long)]
        overwrite: bool,
    },
    /// Get Tableau background job status by job LUID.
    Job {
        #[command(flatten)]
        tableau: TableauArgs,
        /// Tableau background job LUID.
        job_id: String,
    },
}

#[derive(Clone, Args)]
struct TableauArgs {
    /// Tableau Server/Cloud base URL.
    #[arg(long, env = "TABLEAU_SERVER")]
    server: String,
    /// Tableau site content URL. Empty selects the default site.
    #[arg(long, env = "TABLEAU_SITE", default_value = "")]
    site: String,
    /// Tableau REST API version. The 2026.2.4 target uses 3.29.
    #[arg(long, env = "TABLEAU_API_VERSION", default_value = "3.29")]
    api_version: String,
    /// Tableau Personal Access Token name.
    #[arg(long, env = "TABLEAU_PAT_NAME")]
    pat_name: String,
    /// Environment variable containing the Tableau PAT secret.
    #[arg(
        long,
        env = "TABLEAU_PAT_SECRET_ENV",
        default_value = "TABLEAU_PAT_SECRET"
    )]
    pat_secret_env: String,
    /// Tableau HTTP request timeout in seconds.
    #[arg(long, default_value_t = 120)]
    timeout_seconds: u64,
}

impl TableauArgs {
    fn config(self) -> rest::Config {
        rest::Config {
            server: self.server,
            site: self.site,
            api_version: self.api_version,
            pat_name: self.pat_name,
            pat_secret_env: self.pat_secret_env,
            timeout: Duration::from_secs(self.timeout_seconds),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{}: {}", error.code, error.message);
        std::process::exit(1);
    }
}

fn run() -> tabkit::Result<()> {
    let limits = Limits::default();

    match Cli::parse().command {
        Command::Open { input } => {
            let (_package, workbook) = open(&input, &limits)?;
            print_json(&workbook.summary())
        }
        Command::Inspect { input, kind, query } => {
            let (_package, workbook) = open(&input, &limits)?;
            print_json(&workbook::inspect(&workbook, &kind, query.as_deref())?)
        }
        Command::Lineage {
            input,
            from,
            kind,
            upstream,
        } => {
            let (_package, workbook) = open(&input, &limits)?;
            let lineage = Lineage::build(&workbook)?;
            if let Some(query) = from {
                let kind = kind
                    .as_deref()
                    .map(|value| value.parse::<Kind>())
                    .transpose()?;
                let matches = lineage.find_kind(&query, kind);
                if matches.len() != 1 {
                    return print_json(&serde_json::json!({
                        "query": query,
                        "matches": matches,
                        "error": if matches.is_empty() { "not_found" } else { "ambiguous" }
                    }));
                }
                let node = matches[0];
                let direction = if upstream {
                    Direction::Upstream
                } else {
                    Direction::Downstream
                };
                let trace = lineage
                    .trace(node.id, direction)
                    .ok_or_else(|| Error::new("INTERNAL", "Lineage node disappeared"))?;
                print_json(&trace)
            } else {
                print_json(&serde_json::json!({
                    "nodes": lineage.nodes,
                    "edges": lineage.edges,
                    "gaps": lineage.gaps
                }))
            }
        }
        Command::Edit {
            input,
            spec,
            output,
        } => {
            let (package, workbook) = open(&input, &limits)?;
            let batch: Batch = serde_json::from_slice(&fs::read(spec)?)
                .map_err(|error| Error::new("SPEC_JSON", error.to_string()))?;
            let candidate = edit::apply(&workbook, &batch, limits.xml_bytes)?;
            let candidate_package =
                package.write_candidate(&output, candidate.text().as_bytes(), &limits)?;
            print_json(&serde_json::json!({
                "output": output,
                "package_sha256": candidate_package.sha256,
                "twb_sha256": candidate.receipt.sha256,
                "calculations": candidate.receipt.calculations,
                "dashboards": candidate.receipt.dashboards
            }))
        }
        Command::Download {
            tableau,
            workbook_id,
            output,
            include_extract,
        } => {
            let rest = rest::Rest::sign_in(tableau.config())?;
            let receipt =
                rest.download_workbook(&workbook_id, include_extract, &output, &limits)?;
            print_json(&receipt)
        }
        Command::Validate { tableau, input } => {
            let rest = rest::Rest::sign_in(tableau.config())?;
            print_json(&rest.validate_workbook(&input, &limits)?)
        }
        Command::Publish {
            tableau,
            input,
            name,
            project_id,
            overwrite,
        } => {
            let rest = rest::Rest::sign_in(tableau.config())?;
            print_json(&rest.publish_workbook(&input, &name, &project_id, overwrite, &limits)?)
        }
        Command::Job { tableau, job_id } => {
            let rest = rest::Rest::sign_in(tableau.config())?;
            print_json(&rest.job(&job_id)?)
        }
    }
}

fn open(path: &Path, limits: &Limits) -> tabkit::Result<(Package, Workbook)> {
    let (package, twb) = Package::open(path, limits)?;
    let workbook = Workbook::open(twb)?;
    Ok((package, workbook))
}

fn print_json(value: &impl serde::Serialize) -> tabkit::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
