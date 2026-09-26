//! Optional native Hyper capability, isolated in a bounded same-executable worker.
//! Read-only here means admitted SELECT against a private copy, not an invented
//! Hyper Connection "read_only" option. No source extract is opened by the SDK.
use crate::{config::Config, error::{Error, Result}, fs::Workspace};
#[cfg(any(feature = "hyper", test))]
use crate::{error::require, sql};
#[cfg(feature = "hyper")]
use crate::{fs::{hash_file, read_bounded}, rest::cancelled};
use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use serde_json::Value;
#[cfg(feature = "hyper")]
use serde_json::json;
#[cfg(feature = "hyper")]
use std::{fs::File, io::{Read, Write}, process::{Command, Stdio}, time::{Duration, Instant}};
use tokio_util::sync::CancellationToken;
#[derive(Clone,Serialize,Deserialize,JsonSchema)]
#[serde(tag="operation",rename_all="snake_case",deny_unknown_fields)]
pub enum Operation {
    Tables,
    Columns{
        schema:String,
        table:String
    },
    Query{
        sql:String
    },
    Sample{
        schema:String,
        table:String
    },
    Distinct{
        schema:String,
        table:String,
        column:String
    },
    MinMax{
        schema:String,
        table:String,
        column:String
    },
}
#[derive(Serialize,Deserialize,JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub input:String,
    pub expected_sha256:String,
    pub operation:Operation,
    pub max_rows:Option<usize>
}
#[cfg(feature = "hyper")]
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Worker {
    runtime:String,
    memory:String,
    snapshot:String,
    operation:Operation,
    max_rows:usize,
    max_bytes:usize,
}
pub fn available()->bool{
    cfg!(feature="hyper")
}
#[cfg(not(feature = "hyper"))]
pub fn query(_: &Config, _: &Workspace, _: &Request, _: &CancellationToken) -> Result<Value> {
    Err(Error::new("CAPABILITY_UNAVAILABLE", "Hyper requires the hyper build profile and official native runtime"))
}
#[cfg(feature = "hyper")]
pub fn query(cfg:&Config,ws:&Workspace,a:&Request,ct:&CancellationToken)->Result<Value>{
    require(available(),"CAPABILITY_UNAVAILABLE","Build with --features hyper and install the official native SDK runtime")?;
    require(cfg.policy.allow_data_output,"DATA_POLICY","Hyper output is disabled by operator configuration")?;
    let h=cfg.hyper.as_ref().ok_or_else(||Error::new("CAPABILITY_UNAVAILABLE","Hyper runtime is not configured"))?;
    let rows=a.max_rows.unwrap_or(cfg.limits.rows);
    require(rows>0&&rows<=cfg.limits.rows,"LIMIT","Requested rows exceed policy")?;
    // Reject invalid requests before filesystem I/O or copying a large extract.
    operation_query(&a.operation,rows)?;
    let path=ws.input(&a.input)?;
    require(path.extension().and_then(|s|s.to_str())==Some("hyper"),"FILE_TYPE","Expected a .hyper file")?;
    require(hash_file(&path,cfg.limits.file_bytes)?==a.expected_sha256,"STALE_BASE","Hyper input changed")?;
    cancelled(ct)?;
    let directory=tempfile::Builder::new().prefix("hyper-").tempdir_in(ws.state())?;
    let snapshot=directory.path().join("snapshot.hyper");
    {
        let mut source=File::open(&path)?;
        let mut output=File::create(&snapshot)?;
        let mut count=0u64;
        let mut b=[0u8;65536];
        loop{
            cancelled(ct)?;
            let n=source.read(&mut b)?;
            if n==0{
                break;
            }
            count+=n as u64;
            require(count<=cfg.limits.file_bytes,"LIMIT","Extract grew while copying")?;
            output.write_all(&b[..n])?;
        }
        output.sync_all()?;
    }
    require(hash_file(&snapshot,cfg.limits.file_bytes)?==a.expected_sha256&&hash_file(&path,cfg.limits.file_bytes)?==a.expected_sha256,"STALE_BASE","Extract changed during snapshot")?;
    let worker=Worker{
        runtime:h.runtime_directory.to_str().ok_or_else(||Error::new("PATH","Hyper path must be UTF-8"))?.into(),
        memory:h.memory_limit.clone(),
        snapshot:snapshot.to_str().ok_or_else(||Error::new("PATH","Snapshot path must be UTF-8"))?.into(),
        operation:a.operation.clone(),
        max_rows:rows,
        max_bytes:cfg.limits.result_bytes/2
    };
    // Admission also occurs in the child, so no unchecked SQL bypass exists through worker input.
    let payload=crate::wire::encode(&worker,128*1024,false)?;
    let response=directory.path().join("response.json");
    let out=File::create(&response)?;
    let mut command=Command::new(std::env::current_exe()?);
    command.arg("__hyper-worker").env_clear().current_dir(directory.path()).stdout(Stdio::from(out)).stderr(Stdio::null());
    for name in["SystemRoot","WINDIR","PATH","LD_LIBRARY_PATH","DYLD_LIBRARY_PATH"]{
        if let Some(v)=std::env::var_os(name){
            command.env(name,v);
        }
    }
    cancelled(ct)?;
    let mut guard=crate::process_tree::ProcessTree::spawn(&mut command,&payload)?;
    let started=Instant::now();
    loop{
        if ct.is_cancelled()||started.elapsed()>Duration::from_secs(cfg.limits.hyper_seconds){
            guard.stop();
            return Err(Error::new(if ct.is_cancelled(){
                "CANCELLED"
            }else{
                "HYPER_TIMEOUT"
            },"Hyper worker stopped; source extract was not connected"));
        }
        if let Some(status)=guard.child.try_wait()?{
            require(status.success(),"HYPER_WORKER","Native worker failed; verify SDK installation and feature build")?;
            break;
        }
        if std::fs::metadata(&response)?.len()>cfg.limits.result_bytes as u64{
            guard.stop();
            return Err(Error::new("HYPER_RESULT_LIMIT","Worker output limit exceeded"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    guard.stop(); // Release any runtime descendants before snapshot cleanup.
    let bytes=read_bounded(&response,cfg.limits.result_bytes as u64)?;
    let mut result:Value=crate::wire::decode(&bytes,cfg.limits.result_bytes as u64).map_err(|_|Error::new("HYPER_WORKER","Worker returned invalid JSON"))?;
    if let Some(e)=result.get("error"){
        return Err(Error::new("HYPER_QUERY",e.as_str().unwrap_or("Hyper failed")));
    }
    require(hash_file(&path,cfg.limits.file_bytes)?==a.expected_sha256,"STALE_BASE","Source changed during query; discard result")?;
    result["source_sha256"]=json!(a.expected_sha256);
    result["tableau_calculations_evaluated"]=json!(false);
    result["ordering_note"]=json!("Query rows are deterministic only with an explicit complete ORDER BY; samples are unordered");
    Ok(result)
}
#[cfg(any(feature = "hyper", test))]
fn operation_query(operation:&Operation,max_rows:usize)->Result<sql::ReadQuery>{
    let literal=|s:&str|->Result<String>{
        require(!s.contains('\0')&&s.len()<=512,"SQL_POLICY","Invalid metadata selector")?;
        Ok(format!("'{}'",s.replace('\'',"''")))
    };
    let relation=|s:&str,
    t:&str|->Result<String>{
        Ok(format!("{}.{}",sql::identifier(s)?,sql::identifier(t)?))
    };
    match operation{
        Operation::Tables=>Ok(sql::ReadQuery{
            sql:format!("SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_type='BASE TABLE' AND table_schema NOT IN ('pg_catalog','information_schema') ORDER BY table_schema,table_name LIMIT {}",max_rows+1),
            tables:Vec::new()
        }),
        Operation::Columns{
            schema,
            table
        }
        =>Ok(sql::ReadQuery{
            sql:format!("SELECT column_name,data_type,is_nullable,ordinal_position FROM information_schema.columns WHERE table_schema={} AND table_name={} ORDER BY ordinal_position LIMIT {}",literal(schema)?,literal(table)?,max_rows+1),
            tables:Vec::new()
        }),
        Operation::Query{
            sql:s
        }
        =>sql::admit(s,max_rows),
        Operation::Sample{
            schema,
            table
        }
        =>sql::admit(&format!("SELECT * FROM {}",relation(schema,table)?),max_rows),
        Operation::Distinct{
            schema,
            table,
            column
        }
        =>{
            let c=sql::identifier(column)?;
            sql::admit(&format!("SELECT DISTINCT {c} FROM {} ORDER BY {c}",relation(schema,table)?),max_rows)
        },
        Operation::MinMax{
            schema,
            table,
            column
        }
        =>{
            let c=sql::identifier(column)?;
            sql::admit(&format!("SELECT MIN({c}) AS \"min\",MAX({c}) AS \"max\",COUNT(*) AS \"rows\",COUNT({c}) AS \"non_null\" FROM {}",relation(schema,table)?),max_rows)
        },
    }
}
#[cfg(not(feature = "hyper"))]
pub fn worker_main() -> Result<()> {
    Err(Error::new("CAPABILITY_UNAVAILABLE", "Native Hyper feature is not compiled in"))
}
#[cfg(feature = "hyper")]
pub fn worker_main()->Result<()>{
    let mut bytes=Vec::new();
    std::io::stdin().take(128*1024+1).read_to_end(&mut bytes)?;
    require(bytes.len()<=128*1024,"LIMIT","Worker request too large")?;
    let w:Worker=crate::wire::decode(&bytes,128*1024)?;
    require(w.max_rows>0&&w.max_rows<=100000&&w.max_bytes>=4096&&w.max_bytes<=64<<20,"LIMIT","Invalid worker budgets")?;
    let admitted=operation_query(&w.operation,w.max_rows)?;
    let result=native(&w,&admitted)?;
    std::io::stdout().write_all(&result)?;
    Ok(())
}
#[cfg(feature="hyper")]
fn native(w:&Worker,q:&sql::ReadQuery)->Result<Vec<u8>>{
    use std::ffi::{
        CStr,
        CString,
        c_char
    };
    unsafe extern "C"{
        fn tabkit_hyper_query(runtime:*const c_char,memory:*const c_char,snapshot:*const c_char,sql:*const c_char,
        schemas:*const *const c_char,tables:*const *const c_char,count:usize,rows:usize,bytes:usize)->*mut c_char;
        fn tabkit_hyper_free(p:*mut c_char);
    }
    let c=|s:&str|CString::new(s).map_err(|_|Error::new("HYPER_ARGUMENT","NUL in native argument"));
    let runtime=c(&w.runtime)?;
    let memory=c(&w.memory)?;
    let snapshot=c(&w.snapshot)?;
    let query=c(&q.sql)?;
    let schemas=q.tables.iter().map(|(s,_)|c(s)).collect::<Result<Vec<_>>>()?;
    let tables=q.tables.iter().map(|(_,t)|c(t)).collect::<Result<Vec<_>>>()?;
    let sp:Vec<_>=schemas.iter().map(|s|s.as_ptr()).collect();
    let tp:Vec<_>=tables.iter().map(|s|s.as_ptr()).collect();
    // SAFETY: all C strings/arrays live across this synchronous call. C++ catches
    // every exception, returns a malloc-owned NUL-terminated buffer, and provides
    // the matching deallocator. No SDK object crosses the ABI.
    unsafe{
        let p=tabkit_hyper_query(runtime.as_ptr(),memory.as_ptr(),snapshot.as_ptr(),query.as_ptr(),sp.as_ptr(),tp.as_ptr(),sp.len(),w.max_rows,w.max_bytes);
        require(!p.is_null(),"HYPER_NATIVE","Native allocation failed")?;
        let raw=CStr::from_ptr(p).to_bytes();
        if raw.len()>w.max_bytes {
            tabkit_hyper_free(p);
            return Err(Error::new("HYPER_RESULT_LIMIT","Native result too large"));
        }
        let result=raw.to_vec();
        tabkit_hyper_free(p);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_and_typed_queries_remain_available_to_hyper_build() {
        let cases = [Operation::Tables, Operation::Columns { schema:"Extract".into(), table:"Extract".into() },
            Operation::Sample { schema:"Extract".into(), table:"Extract".into() },
            Operation::Distinct { schema:"Extract".into(), table:"Extract".into(), column:"Region".into() },
            Operation::MinMax { schema:"Extract".into(), table:"Extract".into(), column:"Sales".into() }];
        for op in &cases { assert!(operation_query(op, 10).is_ok()); }
    }
    #[test]
    fn generic_query_is_not_removed_or_admitted_without_policy() {
        assert!(operation_query(&Operation::Query { sql:"SELECT SUM(\"Sales\") FROM \"Extract\".\"Extract\"".into() }, 10).is_ok());
        assert!(operation_query(&Operation::Query { sql:"DELETE FROM \"Extract\".\"Extract\"".into() }, 10).is_err());
    }
}
