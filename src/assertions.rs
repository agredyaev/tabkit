//! Declarative product checks. This shipped feature is separate from developer regressions.
use crate::workbook::{FieldId, FilterId};
use crate::{
    config::Config,
    error::{
        Result,
        require,
        value
    },
    fs::{
        Workspace,
        hash_file
    },
    hyper,
    package::Package,
    scalar::{
        Domain,
        Scalar
    },
    workbook::Workbook
};
use serde::{
    Serialize,
    Deserialize
};
use schemars::JsonSchema;
use serde_json::{
    Value,
    json
};
use tokio_util::sync::CancellationToken;
#[derive(Serialize,Deserialize,JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema_version:u32,
    /// IDs in this suite are valid only for this exact artifact.
    pub input_sha256:String,
    pub assertions:Vec<Assertion>,
}
#[derive(Serialize,Deserialize,JsonSchema)]
#[serde(tag="assert",rename_all="snake_case",deny_unknown_fields)]
pub enum Assertion {
    Calculation {
        field_id:FieldId,
        formula:String
    },
    Parameter {
        field_id:FieldId,
        current:Scalar,
        domain:Option<Domain>
    },
    FilterValues {
        filter_id:FilterId,
        values:Vec<String>
    },
    FilterRange {
        filter_id:FilterId,
        min:Scalar,
        max:Scalar
    },
    NoDiagnostics,
    InventoryUnchanged {
        baseline:String
    },
    /// Byte-exact preservation of an explicitly selected root container.
    SubtreeUnchanged { baseline:String, section:ProtectedSection },
    FileHash {
        input:String,
        sha256:String
    },
    /// Expected is SDK text representation, or null for SQL NULL. It is not Tableau evaluation.
    HyperScalar {
        input:String,
        input_sha256:String,
        sql:String,
        expected:Option<String>
    },
}
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="snake_case")]
pub enum ProtectedSection { Datasources, Worksheets, Dashboards, Windows }
impl ProtectedSection {
    fn tag(self)->&'static str { match self { Self::Datasources=>"datasources",Self::Worksheets=>"worksheets",Self::Dashboards=>"dashboards",Self::Windows=>"windows" } }
}
#[derive(Serialize)]
pub struct Report {
    pub passed:bool,
    pub assertions:Vec<Value>,
    pub tableau_execution:&'static str,
    pub input_sha256:String,
    pub local_validation:crate::validation::LocalValidation,
}
struct Baseline {
    inventory:std::collections::BTreeMap<String,Value>,
    subtrees:std::collections::BTreeMap<ProtectedSection,String>,
}
fn inventory(book:&Workbook)->Result<std::collections::BTreeMap<String,Value>> {
    Ok(book.snapshot()?.into_iter().filter(|(k,_)|k.ends_with("/identity")||k.starts_with("workbook/")).collect())
}
fn subtree(book:&Workbook,section:ProtectedSection)->Result<String> {
    let mut nodes=book.xml.named_children(crate::xml::NodeId(0),section.tag());
    let first=nodes.next();
    require(nodes.next().is_none(),"AMBIGUOUS_TARGET","Multiple protected root containers")?;
    Ok(match first { Some(n)=>crate::fs::sha256(book.xml.text[book.xml.node(n).span.range()].as_bytes()),None=>"absent".into() })
}
fn baseline(book:&Workbook)->Result<Baseline> {
    let mut subtrees=std::collections::BTreeMap::new();
    for s in [ProtectedSection::Datasources,ProtectedSection::Worksheets,ProtectedSection::Dashboards,ProtectedSection::Windows] {
        subtrees.insert(s,subtree(book,s)?);
    }
    Ok(Baseline{inventory:inventory(book)?,subtrees})
}
pub fn run(cfg:&Config,ws:&Workspace,pkg:&Package,book:&Workbook,suite:&Suite,ct:&CancellationToken)->Result<Report>{
    require(suite.schema_version==1,"SCHEMA_VERSION","Unknown assertion schema")?;
    require(suite.input_sha256==pkg.sha256,"STALE_BASE","Assertion IDs are bound to another workbook artifact")?;
    require(!suite.assertions.is_empty()&&suite.assertions.len()<=1000,"LIMIT","Assertion count must be 1..1000")?;
    let mut baselines=std::collections::BTreeMap::<String,Baseline>::new();
    let mut current_baseline=None;
    let local=crate::validation::local(book);
    let mut results=Vec::new();
    let mut passed=true;
    for(index,a)in suite.assertions.iter().enumerate(){
        crate::rest::cancelled(ct)?;
        match check(cfg,ws,book,a,&local,&mut baselines,&mut current_baseline,ct){
            Ok(())=>results.push(json!({
                "index":index,
                "status":"passed"
            })),
            Err(e)=>{
                passed=false;
                results.push(json!({
                    "index":index,
                    "status":"failed",
                    "error":e
                }));
            }
        }
    }
    Ok(Report { passed,assertions:results,tableau_execution:"not_run",input_sha256:pkg.sha256.clone(),local_validation:local })
}

fn check(cfg:&Config,ws:&Workspace,book:&Workbook,a:&Assertion,local:&crate::validation::LocalValidation,
    baselines:&mut std::collections::BTreeMap<String,Baseline>,current:&mut Option<Baseline>,ct:&CancellationToken)->Result<()>{
    match a{
        Assertion::Calculation{
            field_id,
            formula
        }
        =>require(book.field(*field_id)?.formula.as_deref()==Some(formula.as_str()),"ASSERT_CALCULATION","Calculation differs"),
        Assertion::Parameter{
            field_id,
            current,
            domain
        }
        =>{
            let(c,d)=book.parameter_state(*field_id)?;
            require(c==*current && domain.as_ref().map(|expected|expected==&d).unwrap_or(true),"ASSERT_PARAMETER","Parameter differs")
        },
        Assertion::FilterValues{
            filter_id,
            values
        }
        =>require(book.filter_state(*filter_id)?==json!({
            "kind":"categorical",
            "values":values
        }),"ASSERT_FILTER","Categorical filter differs"),
        Assertion::FilterRange{
            filter_id,
            min,
            max
        }
        =>require(book.filter_state(*filter_id)?==json!({
            "kind":"range",
            "min":min,
            "max":max
        }),"ASSERT_FILTER","Range filter differs"),
        Assertion::NoDiagnostics=>require(local.passed,"ASSERT_DIAGNOSTICS","Workbook has known local errors"),
        Assertion::InventoryUnchanged{baseline:path}|Assertion::SubtreeUnchanged{baseline:path,..}=>{
            if !baselines.contains_key(path) {
                let (_,xml)=Package::open(&ws.input(path)?,&cfg.limits)?;
                let original=Workbook::from_xml(xml)?;
                baselines.insert(path.clone(),baseline(&original)?);
            }
            if current.is_none(){*current=Some(baseline(book)?);}
            let original=baselines.get(path).ok_or_else(||crate::error::Error::new("INTERNAL","Baseline projection missing"))?;
            let now=current.as_ref().ok_or_else(||crate::error::Error::new("INTERNAL","Current projection missing"))?;
            match a {
                Assertion::InventoryUnchanged{..}=>require(original.inventory==now.inventory,"ASSERT_INVENTORY","Object identities or sheet/datasource inventory changed"),
                Assertion::SubtreeUnchanged{section,..}=>require(original.subtrees.get(section)==now.subtrees.get(section),"ASSERT_SUBTREE","Protected XML container bytes changed"),
                _=>Err(crate::error::Error::new("INTERNAL","Unexpected baseline assertion")),
            }
        },
        Assertion::FileHash{
            input,
            sha256
        }
        =>require(hash_file(&ws.input(input)?,cfg.limits.file_bytes)?==*sha256,"ASSERT_HASH","File hash differs"),
        Assertion::HyperScalar{
            input,
            input_sha256,
            sql,
            expected
        }
        =>{
            let result=hyper::query(cfg,ws,&hyper::Request{
                input:input.clone(),
                expected_sha256:input_sha256.clone(),
                operation:hyper::Operation::Query{
                    sql:sql.clone()
                },
                max_rows:Some(2)
            },ct)?;
            require(result["truncated"]==false&&result["rows"].as_array().map(|v|v.len())==Some(1)&&result["rows"][0].as_array().map(|v|v.len())==Some(1),"ASSERT_CARDINALITY","Hyper scalar must return exactly one row and one column")?;
            require(result["rows"][0][0]==value(expected)?,"ASSERT_HYPER_SCALAR","Hyper scalar differs")
        },
    }
}
