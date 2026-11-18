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
    Open {
        input: PathBuf,
    },
    Inspect {
        input: PathBuf,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        query: Option<String>,
    },
    Lineage {
        input: PathBuf,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        upstream: bool,
    },
    Edit {
        input: PathBuf,
        spec: PathBuf,
        output: PathBuf,
    },
    Download {
        #[command(flatten)]
        tableau: TableauArgs,
        workbook_id: String,
        output: PathBuf,
        #[arg(long, default_value_t = true)]
        include_extract: bool,
    },
    Validate {
        #[command(flatten)]
        tableau: TableauArgs,
        input: PathBuf,
    },
    Publish {
        #[command(flatten)]
        tableau: TableauArgs,
        input: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        overwrite: bool,
    },
    Job {
        #[command(flatten)]
        tableau: TableauArgs,
        job_id: String,
    },
}

#[derive(Clone, Args)]
struct TableauArgs {
    #[arg(long, env = "TABLEAU_SERVER")]
    server: String,
    #[arg(long, env = "TABLEAU_SITE", default_value = "")]
    site: String,
    #[arg(long, env = "TABLEAU_API_VERSION", default_value = "3.29")]
    api_version: String,
    #[arg(long, env = "TABLEAU_PAT_NAME")]
    pat_name: String,
    #[arg(
        long,
        env = "TABLEAU_PAT_SECRET_ENV",
        default_value = "TABLEAU_PAT_SECRET"
    )]
    pat_secret_env: String,
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
