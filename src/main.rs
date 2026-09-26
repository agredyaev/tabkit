mod app;
mod assertions;
mod catalog;
mod config;
mod edit;
mod error;
mod formula;
mod fs;
mod hyper;
mod mcp;
mod oauth;
mod package;
mod patch;
#[cfg(any(feature = "hyper", test))]
mod process_tree;
mod rest;
mod scalar;
#[cfg(any(feature = "hyper", test))]
mod sql;
mod validation;
mod wire;
mod workbook;
mod xml;
use clap::{
    Parser,
    Subcommand
};
use error::{
    Error,
    Result,
    require
};
use serde_json::{
    Value,
    json
};
use std::{
    io::{
        Read,
        Write
    },
    path::PathBuf,
    sync::Arc
};
use tokio_util::sync::CancellationToken;
#[derive(Parser)]
#[command(name="tabkit",version,about="Focused Tableau 2025 tools for Amazon Quick Desktop")]
struct Cli{
    #[command(flatten)]
    startup:config::StartupArgs,
    #[command(subcommand)]command:Command,
}
#[derive(Subcommand)]
enum Command{
    /// Serve the same tools over local MCP stdio; stdout is exclusively protocol.
    Mcp,
    /// Emit actual machine-readable tool names, descriptions and JSON Schemas.
    Tools,
    /// Call a tool with JSON arguments. '-' reads stdin; file paths here are operator inputs.
    Call{
        tool:String,
        #[arg(long,default_value="-")]args:PathBuf
    },
    /// Convenience aliases; they dispatch into the same application functions.
    Inspect{
        input:String
    },
    Validate{
        input:String
    },
    Test{
        input:String,
        suite:String
    },
}
fn main(){
    // Same-executable native worker. It never hosts MCP or receives OAuth/Tableau session tokens.
    if std::env::args().nth(1).as_deref()==Some("__hyper-worker"){
        if let Err(e)=hyper::worker_main(){
            let _=writeln!(std::io::stdout(),"{}",json!({
                "error":e
            }));
            std::process::exit(1);
        }
        return;
    }
    let exit=match run(){
        Ok(code)=>code,
        Err(e)=>{
            let _=writeln!(std::io::stderr(),"{}",json!({
                "error":e
            }));
            2
        }
    };
    std::process::exit(exit);
}
fn run()->Result<i32>{
    let cli=Cli::parse();
    if matches!(&cli.command,Command::Tools){
        print_json(&serde_json::to_value(catalog::tools()?)?)?;
        return Ok(0);
    }
    // Blocking reqwest client is constructed outside a Tokio runtime for CLI calls.
    let app=Arc::new(app::App::new(config::Config::from_startup(cli.startup)?)?);
    let (tool,args)=match cli.command{
        Command::Mcp=>{
            let runtime=tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            runtime.block_on(mcp::serve(app))?;
            return Ok(0);
        },
        Command::Tools=>return Err(Error::new("INTERNAL","Unreachable command route")),
        Command::Call{
            tool,
            args
        }
        =>{
            let max=app.cfg.limits.input_json_bytes;
            let bytes=if args.as_os_str()=="-"{
                let mut b=Vec::new();
                std::io::stdin().take(max+1).read_to_end(&mut b)?;
                require(b.len()as u64<=max,"LIMIT","Arguments exceed limit")?;
                b
            }else{
                fs::read_bounded(&args,max)?
            };
            (tool,wire::decode(&bytes,max)?)
        },
        Command::Inspect{
            input
        }
        =>("workbook_inspect".into(),json!({
            "input":input
        })),
        Command::Validate{
            input
        }
        =>("workbook_validate".into(),json!({
            "input":input
        })),
        Command::Test{
            input,
            suite
        }
        =>("workbook_test".into(),json!({
            "input":input,
            "suite":suite
        })),
    };
    let result=app.dispatch(&tool,args,&CancellationToken::new())?;
    let failed=mcp::result_failed(&result);
    print_json(&result)?;
    Ok(if failed{
        1
    }else{
        0
    })
}
fn print_json(v:&Value)->Result<()>{
    let stdout=std::io::stdout();
    let mut out=stdout.lock();
    serde_json::to_writer_pretty(&mut out,v)?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod regression;

#[cfg(test)]
mod properties;
#[cfg(test)]
mod behavioral;

#[cfg(test)]
mod edge_cases;

#[cfg(test)]
mod local_core_tests;

#[cfg(test)]
mod xml_index_tests;
