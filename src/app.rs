//! Application boundary: every CLI/MCP tool reaches this single dispatcher.
use crate::{
    assertions,
    config::Config,
    edit,
    error::{
        Error,
        Result,
        require
    },
    fs::{
        self,
        Workspace
    },
    hyper,
    package::Package,
    rest::{
        self,
        Rest
    },
    validation,
    wire,
    workbook::{Workbook, lineage::{Direction, Graph, ImpactCursor, NodeKind, NodeRef, SearchBy}}
};
use schemars::JsonSchema;
use serde::{
    Serialize,
    Deserialize,
    de::DeserializeOwned
};
use serde_json::{
    Value,
    json
};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::Mutex,
    time::{
        SystemTime,
        UNIX_EPOCH
    }
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Empty{
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Input{
    pub input:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Inspect {
    pub input:String,
    #[serde(default)]pub section:InspectSection,
    #[serde(default)]pub offset:usize,
    #[serde(default="page_limit")]pub limit:usize,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct LineageFind {
    pub snapshot_id:String,
    pub prefix:String,
    #[serde(default)]pub by:SearchBy,
    pub kind:Option<NodeKind>,
    #[serde(default)]pub offset:usize,
    #[serde(default="page_limit")]pub limit:usize,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct LineageNeighbors {
    pub snapshot_id:String,
    pub node:NodeRef,
    pub direction:Direction,
    #[serde(default)]pub offset:usize,
    #[serde(default="page_limit")]pub limit:usize,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct LineageImpact {
    pub snapshot_id:String,
    pub from:Option<NodeRef>,
    pub direction:Option<Direction>,
    pub cursor:Option<String>,
    #[serde(default="page_limit")]pub limit:usize,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct LineageExport {
    pub snapshot_id:Option<String>,
    pub input:Option<String>,
    pub output:String,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct LineageGaps {
    pub snapshot_id:String,
    #[serde(default)]pub offset:usize,
    #[serde(default="page_limit")]pub limit:usize,
}
fn page_limit()->usize{
    100
}
#[derive(Clone,Copy,Default,Deserialize,JsonSchema)]#[serde(rename_all="snake_case")]
pub enum InspectSection{
    #[default]Overview,
    Fields,
    Calculations,
    Parameters,
    Filters,
    Datasources,
    Sheets,
    References,
    Diagnostics,
    LocalDefinitions,
    DependencyScopes,
    Uses,
    Package
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Id{
    pub id:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Download{
    pub workbook_id:String,
    pub output:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct PlanRequest{
    pub input:String,
    pub changes:edit::ChangeSet,
    pub output:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Apply{
    pub plan:String,
    pub expected_plan_sha256:String,
    pub output:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Diff{
    pub before:String,
    pub after:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Test{
    pub input:String,
    pub suite:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Extract{
    pub input:String,
    pub expected_sha256:String,
    pub entry:String,
    pub output:String
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Guide{
    pub topic:GuideTopic
}
#[derive(Deserialize,JsonSchema)]#[serde(rename_all="snake_case")]pub enum GuideTopic{
    Workflow,
    DashboardAdvisor,
    VizCritique,
    CalculationReview,
    DataQualitySentinel,
    About,
    ContentViewer
}
#[derive(Clone,Serialize,Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct PreparePublish{
    pub input:String,
    pub expected_sha256:String,
    pub name:String,
    pub project_id:String,
    /// None creates a new copy. Overwrite additionally requires operator policy.
    pub overwrite_workbook_id:Option<String>,
    /// Acknowledges that no local checker can replace Tableau 2025 execution.
    pub acknowledge_tableau_not_run:bool,
    /// Does NOT bypass known errors. Acknowledges preserved, unsupported constructs.
    #[serde(default)]pub acknowledge_unsupported_objects:bool,
    pub test_suite:Option<String>,
    pub expected_suite_sha256:Option<String>,
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct ConfirmPublish{
    pub approval_id:String,
    pub expected_approval_sha256:String,
    pub confirm:bool
}
#[derive(Deserialize,JsonSchema)]#[serde(deny_unknown_fields)]pub struct Receipt{
    pub approval_id:String
}
#[derive(Serialize,Deserialize)]#[serde(deny_unknown_fields)]struct Approval{
    schema_version:u32,
    engine_version:String,
    id:String,
    expires_at:u64,
    request:PreparePublish,
    server:Value,
    baseline:Option<Value>,
}
pub struct App{
    pub cfg:Config,
    pub ws:Workspace,
    rest:Mutex<Option<Rest>>,
    lineage:Mutex<Option<LineageSession>>,
}
struct LineageSession { id:String, graph:Graph, impact:Option<ImpactCursor> }
impl App{
    pub fn new(cfg:Config)->Result<Self>{
        Ok(Self{
            ws:Workspace::new(cfg.workspace.clone())?,
            cfg,
            rest:Mutex::new(None),
            lineage:Mutex::new(None),
        })
    }
    fn remote<F>(&self,ct:&CancellationToken,f:F)->Result<Value> where F:FnOnce(&mut Rest)->Result<Value>{
        let mut guard=self.rest.lock().map_err(|_|Error::new("INTERNAL","REST session mutex is poisoned"))?;
        if guard.is_none(){
            *guard=Some(Rest::new(&self.cfg)?);
        }
        let rest=guard.as_mut().ok_or_else(||Error::new("INTERNAL","REST session absent"))?;
        if !rest.authenticated(){
            rest.login(ct)?;
        }
        f(rest)
    }
    fn load(&self,input:&str)->Result<(Package,Workbook)>{
        let(pkg,xml)=Package::open(&self.ws.input(input)?,&self.cfg.limits)?;
        let book=Workbook::from_xml(xml)?;
        Ok((pkg,book))
    }
    /// dispatch is synchronous; the MCP boundary moves it to a blocking worker.
    pub fn dispatch(&self,name:&str,args:Value,ct:&CancellationToken)->Result<Value>{
        wire::encoded_len(&args,self.cfg.limits.input_json_bytes as usize)?;
        rest::cancelled(ct)?;
        let result=match name{
            "system_status"=>{
                let _:Empty=parse(args)?;
                Ok(json!({
                    "engine":env!("CARGO_PKG_VERSION"),
                    "tableau_api":self.cfg.tableau.as_ref().map(|t|&t.api_version),
                    "tableau_auth":self.cfg.tableau.as_ref().map(|t|t.auth.kind()),
                    "hyper_compiled":hyper::available(),
                    "hyper_configured":self.cfg.hyper.is_some(),
                    "publish_enabled":self.cfg.policy.publish_enabled,
                    "allow_overwrite":self.cfg.policy.allow_overwrite,
                    "allow_data_output":self.cfg.policy.allow_data_output,
                    "local_validation":"XML/known structures/references only",
                    "tableau_semantics":"not_run",
                    "supported_edit_profile":"2025.x with source-build; supported shapes only"
                }))
            },
            "guidance"=>{
                let a:Guide=parse(args)?;
                let text=match a.topic{
                    GuideTopic::Workflow=>include_str!("../resources/workflow.md"),
                    GuideTopic::DashboardAdvisor=>include_str!("../resources/dashboard-advisor.md"),
                    GuideTopic::VizCritique=>include_str!("../resources/viz-critique.md"),
                    GuideTopic::CalculationReview=>include_str!("../resources/calculation-review.md"),
                    GuideTopic::DataQualitySentinel=>include_str!("../resources/data-quality-sentinel.md"),
                    GuideTopic::About=>include_str!("../resources/about.md"),
                    GuideTopic::ContentViewer=>include_str!("../resources/content-viewer.md")
                };
                Ok(json!({
                    "guidance":text
                }))
            },
            "file_hash"=>{
                let a:Input=parse(args)?;
                Ok(json!({
                    "input":a.input,
                    "sha256":fs::hash_file(&self.ws.input(&a.input)?,self.cfg.limits.file_bytes)?
                }))
            },
            "tableau_login"=>{
                let _:Empty=parse(args)?;
                self.remote(ct,|r|Ok(json!({
                    "authenticated":true,
                    "identity":r.identity()
                })))
            },
            "tableau_logout"=>{
                let _:Empty=parse(args)?;
                let mut r=self.rest.lock().map_err(|_|Error::new("INTERNAL","Session mutex poisoned"))?;
                match r.as_mut(){
                    Some(r)=>r.logout(ct),
                    None=>Ok(json!({
                        "authenticated":false
                    }))
                }
            },
            "tableau_explore"=>{
                let a:rest::Explore=parse(args)?;
                self.remote(ct,|r|Ok(serde_json::to_value(r.explore(&a,ct)?)?))
            },
            "tableau_search"=>{
                let a:rest::Search=parse(args)?;
                self.remote(ct,|r|r.search(&a,self.cfg.limits.max_search_pages,ct))
            },
            "tableau_get_workbook"=>{
                let a:Id=parse(args)?;
                self.remote(ct,|r|r.workbook(&a.id,ct))
            },
            "tableau_get_job"=>{
                let a:Id=parse(args)?;
                self.remote(ct,|r|r.job(&a.id,ct))
            },
            "tableau_download_workbook"=>{
                let a:Download=parse(args)?;
                let path=self.ws.output(&a.output)?;
                let mut result=self.remote(ct,|r|r.download(&a.workbook_id,&path,&self.cfg,ct))?;
                result["output"]=json!(a.output);
                Ok(result)
            },
            "tableau_view_image"|"tableau_view_data"=>{
                let a:rest::ViewExport=parse(args)?;
                let path=self.ws.output(&a.output)?;
                let image=name=="tableau_view_image";
                let mut result=self.remote(ct,|r|r.view_export(&a,&path,image,&self.cfg,ct))?;
                result["output"]=json!(a.output);
                let preview=(||->Result<Value>{
                    if image{
                    use base64::Engine;
                    let bytes=fs::read_bounded(&path,self.cfg.limits.result_bytes as u64/2)?;
                    Ok(json!({"_mcp_image_png":base64::engine::general_purpose::STANDARD.encode(bytes)}))
                }
                else {
                    let bytes=fs::read_bounded(&path,self.cfg.limits.result_bytes as u64)?;
                    let text=std::str::from_utf8(&bytes).map_err(|_|Error::new("CSV_ENCODING","CSV export is not UTF-8; file is retained for inspection"))?;
                    let mut end=text.len().min(self.cfg.limits.result_bytes/8);
                    while !text.is_char_boundary(end){
                        end-=1;
                    }
                    if end<text.len(){
                        end=text[..end].rfind('\n').map(|n|n+1).unwrap_or(0);
                    }
                    Ok(json!({"csv_preview":&text[..end],"csv_truncated":end<text.len()}))
                }
                })();
                match preview {
                    Ok(Value::Object(fields))=>{for(k,v)in fields{result[k]=v;}},
                    Ok(_)=>{},
                    Err(e)=>result["preview_error"]=json!(e),
                }

                Ok(result)
            },
            "workbook_inspect"=>self.inspect(parse(args)?),
            "workbook_lineage_open"=>self.lineage_open(parse(args)?),
            "workbook_lineage_find"=>self.lineage_find(parse(args)?),
            "workbook_lineage_neighbors"=>self.lineage_neighbors(parse(args)?),
            "workbook_lineage_impact"=>self.lineage_impact(parse(args)?),
            "workbook_lineage_gaps"=>self.lineage_gaps(parse(args)?),
            "workbook_lineage_export"=>self.lineage_export(parse(args)?),
            "workbook_validate"=>{
                let a:Input=parse(args)?;
                let(pkg,book)=self.load(&a.input)?;
                let checked=validation::local(&book);
                Ok(json!({
                    "passed":checked.passed,"status":checked.status,"input_sha256":pkg.sha256,
                    "xml":"passed","scope":"Known local invariants only; partial means unsupported constructs remain",
                    "coverage":{"static_parameters":checked.static_parameters,"simple_filters":checked.simple_filters,
                        "unsupported_objects":checked.unsupported_objects},
                    "diagnostics":checked.diagnostics,"xsd":"not_run","tableau_semantics":"not_run","business_tests":"not_run"
                }))
            },
            "workbook_plan"=>{
                let a:PlanRequest=parse(args)?;
                let(pkg,book)=self.load(&a.input)?;
                let plan=edit::plan_product(&a.input,&pkg.sha256,book,a.changes,&self.cfg)?;
                let bytes=wire::encode(&plan,self.cfg.limits.plan_json_bytes as usize,true)?;
                let mut result=json!({"output":a.output,"plan_sha256":fs::sha256(&bytes),
                    "candidate_twb_sha256":plan.candidate_twb_sha256,"delta":plan.delta,
                    "patch_count":plan.patches.len(),"warnings":plan.warnings,"tableau_semantics":"not_run"});
                compact_details(&mut result,self.cfg.limits.result_bytes)?;
                rest::cancelled(ct)?;
                self.ws.write_new(&a.output,&bytes)?;
                Ok(result)
            },
            "workbook_apply"=>self.apply(parse(args)?,ct),
            "workbook_diff"=>{
                let a:Diff=parse(args)?;
                let(_,before)=self.load(&a.before)?;
                let(_,after)=self.load(&a.after)?;
                Ok(json!({
                    "delta":edit::diff(&before.snapshot()?,&after.snapshot()?),
                    "scope":"Known semantic properties. Use plan/apply for byte-preservation proof; IDs assume unchanged object ordering."
                }))
            },
            "workbook_test"=>{
                let a:Test=parse(args)?;
                let(pkg,book)=self.load(&a.input)?;
                let suite=wire::decode(&fs::read_bounded(&self.ws.input(&a.suite)?,self.cfg.limits.input_json_bytes)?,self.cfg.limits.input_json_bytes)?;
                Ok(serde_json::to_value(assertions::run(&self.cfg,&self.ws,&pkg,&book,&suite,ct)?)?)
            },
            "workbook_extract_hyper"=>{
                let a:Extract=parse(args)?;
                let(pkg,_)=self.load(&a.input)?;
                require(pkg.sha256==a.expected_sha256,"STALE_BASE","Package changed")?;
                let out=self.ws.output(&a.output)?;
                rest::cancelled(ct)?;
                let sha=pkg.extract_hyper(&a.entry,&out,&self.cfg.limits)?;
                Ok(json!({
                    "output":a.output,
                    "sha256":sha
                }))
            },
            "hyper_query"=>{
                let a:hyper::Request=parse(args)?;
                hyper::query(&self.cfg,&self.ws,&a,ct)
            },
            "tableau_prepare_publish"=>self.prepare_publish(parse(args)?,ct),
            "tableau_publish"=>self.confirm_publish(parse(args)?,ct),
            "tableau_publish_receipt"=>{
                let a:Receipt=parse(args)?;
                approval_id(&a.approval_id)?;
                let name=format!("{}.receipt.json",a.approval_id);
                match self.ws.internal_read(&name,self.cfg.limits.result_bytes as u64){
                    Ok(b)=>wire::decode(&b,self.cfg.limits.result_bytes as u64),
                    Err(_)=>{
                        let attempted=self.ws.state().join(format!("{}.attempt",a.approval_id)).exists();
                        Ok(json!({
                            "approval_id":a.approval_id,
                            "status":if attempted{
                                "outcome_unknown"
                            }else{
                                "not_attempted"
                            },
                            "instruction":"Never repeat an attempted publish automatically. Inspect server jobs/content."
                        }))
                    }
                }
            },
            _=>Err(Error::new("UNKNOWN_TOOL",format!("Unknown tool {name}"))),
        }?;
        if wire::encoded_len(&result,self.cfg.limits.result_bytes).is_ok(){return Ok(result);}
        // Never erase an existing artifact/remote receipt with a late generic error.
        if result.get("output").is_some() || result.get("approval_id").is_some(){
            let mut compact=json!({"report_truncated":true,"instruction":"Inspect the output file or tableau_publish_receipt; do not replay the side effect."});
            for key in ["output","sha256","plan_sha256","candidate_twb_sha256","approval_id","approval_sha256","status","passed"] {
                if let Some(v)=result.get(key){compact[key]=v.clone();}
            }
            wire::encoded_len(&compact,self.cfg.limits.result_bytes)?;
            return Ok(compact);
        }
        Err(Error::new("RESULT_LIMIT","Read-only report exceeds budget; request a narrower page or smaller result"))
    }
    fn lineage_open(&self,a:Input)->Result<Value>{
        let(pkg,book)=self.load(&a.input)?;
        let graph=Graph::build(book,pkg.sha256.clone())?;
        let id=Uuid::new_v4().to_string();
        let (nodes,edges)=graph.count();
        let mut gap_codes=BTreeMap::<&str,usize>::new();
        for gap in &graph.gaps {*gap_codes.entry(&gap.code).or_default()+=1;}
        let result=json!({"snapshot_id":id,"input_sha256":pkg.sha256,
            "source_build":graph.source_build.as_deref().map(|s|s.chars().take(128).collect::<String>()),
            "nodes":nodes,"edges":edges,"coverage":graph.coverage(),"gap_codes":gap_codes});
        let mut slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        *slot=Some(LineageSession{id,graph,impact:None});
        Ok(result)
    }
    fn lineage_find(&self,a:LineageFind)->Result<Value>{
        let slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        let session=active_lineage(&slot,&a.snapshot_id)?;
        let mut result=session.graph.find(&a.prefix,a.by,a.kind,a.offset,a.limit)?;
        result["snapshot_id"]=json!(a.snapshot_id);
        Ok(result)
    }
    fn lineage_neighbors(&self,a:LineageNeighbors)->Result<Value>{
        let slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        let session=active_lineage(&slot,&a.snapshot_id)?;
        let mut result=session.graph.neighbors(a.node,a.direction,a.offset,a.limit)?;
        result["snapshot_id"]=json!(a.snapshot_id);
        Ok(result)
    }
    fn lineage_impact(&self,a:LineageImpact)->Result<Value>{
        require((1..=100).contains(&a.limit),"LIMIT","Lineage page limit must be 1..100")?;
        let mut slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        let session=active_lineage_mut(&mut slot,&a.snapshot_id)?;
        match (a.from,a.cursor) {
            (Some(from),None)=>{
                session.impact=Some(ImpactCursor::new(&session.graph,from,
                    a.direction.unwrap_or(Direction::Downstream))?);
            },
            (None,Some(cursor))=>{
                require(a.direction.is_none(),"INPUT","Direction belongs to the first impact page")?;
                require(session.impact.as_ref().is_some_and(|c| c.id==cursor),"STALE_CURSOR",
                    "Impact cursor is absent or has been replaced")?;
            },
            _=>return Err(Error::new("INPUT","Provide from or cursor, but not both")),
        }
        let cursor=session.impact.as_mut().ok_or_else(||Error::new("INTERNAL","Impact cursor absent"))?;
        let mut result=cursor.page(&session.graph,a.limit)?;
        if result["done"]==true { session.impact=None; }
        result["snapshot_id"]=json!(a.snapshot_id);
        Ok(result)
    }
    fn lineage_export(&self,a:LineageExport)->Result<Value>{
        require(a.input.is_some() != a.snapshot_id.is_some(),"INPUT",
            "Provide exactly one of input or snapshot_id")?;
        if let Some(input)=a.input {
            let(pkg,book)=self.load(&input)?;
            let graph=Graph::build(book,pkg.sha256)?;
            return self.write_lineage_export(&graph,&a.output);
        }
        let slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        let session=active_lineage(&slot,a.snapshot_id.as_deref().unwrap_or(""))?;
        self.write_lineage_export(&session.graph,&a.output)
    }
    fn lineage_gaps(&self,a:LineageGaps)->Result<Value>{
        let slot=self.lineage.lock().map_err(|_|Error::new("INTERNAL","Lineage mutex is poisoned"))?;
        let session=active_lineage(&slot,&a.snapshot_id)?;
        let mut result=session.graph.gap_page(a.offset,a.limit)?;
        result["snapshot_id"]=json!(a.snapshot_id);
        Ok(result)
    }
    fn write_lineage_export(&self,graph:&Graph,output:&str)->Result<Value>{
        let(path,sha256,bytes)=self.ws.write_new_stream(output,self.cfg.limits.file_bytes,
            |writer|graph.export(writer))?;
        let (nodes,edges)=graph.count();
        Ok(json!({"output":path,"sha256":sha256,"bytes":bytes,
            "input_sha256":graph.input_sha256,"nodes":nodes,"edges":edges,
            "coverage":graph.coverage()}))
    }
    fn inspect(&self,a:Inspect)->Result<Value>{
        require((1..=1000).contains(&a.limit),"LIMIT","Inspection limit must be 1..1000")?;
        let(pkg,book)=self.load(&a.input)?;
        let selected_fields:Vec<usize>=match a.section {
            InspectSection::Fields=>(0..book.fields.len()).collect(),
            InspectSection::Calculations=>book.fields.iter().enumerate().filter(|(_,f)|f.formula.is_some()&&!f.parameter).map(|(i,_)|i).collect(),
            InspectSection::Parameters=>book.fields.iter().enumerate().filter(|(_,f)|f.parameter).map(|(i,_)|i).collect(),
            _=>Vec::new(),
        };
        let total=match a.section {
            InspectSection::Overview=>0,
            InspectSection::Fields|InspectSection::Calculations|InspectSection::Parameters=>selected_fields.len(),
            InspectSection::Filters=>book.filters.len(),
            InspectSection::Datasources=>book.datasources.len(),
            InspectSection::Sheets=>book.worksheets.len()+book.dashboards.len(),
            InspectSection::References=>book.edges.len(),
            InspectSection::Diagnostics=>book.diagnostics.len(),
            InspectSection::LocalDefinitions=>book.local_definitions.len(),
            InspectSection::DependencyScopes=>book.dependency_scopes.len(),
            InspectSection::Uses=>book.known_uses.len(),
            InspectSection::Package=>pkg.entries.len(),
        };
        let end=a.offset.saturating_add(a.limit).min(total);
        let mut items=Vec::new();
        for i in a.offset.min(total)..end {
            items.push(match a.section {
                InspectSection::Fields|InspectSection::Calculations|InspectSection::Parameters=>book.field_report(selected_fields[i]),
                InspectSection::Filters=>book.filter_report(i),
                InspectSection::Datasources=>book.datasource_report(i)?,
                InspectSection::References=>serde_json::to_value(&book.edges[i])?,
                InspectSection::Diagnostics=>serde_json::to_value(&book.diagnostics[i])?,
                InspectSection::LocalDefinitions=>serde_json::to_value(&book.local_definitions[i])?,
                InspectSection::DependencyScopes=>book.scope_report(i),
                InspectSection::Uses=>serde_json::to_value(&book.known_uses[i])?,
                InspectSection::Package=>serde_json::to_value(&pkg.entries[i])?,
                InspectSection::Sheets=>if i<book.worksheets.len(){
                    json!({
                        "kind":"worksheet",
                        "name":book.worksheets[i]
                    })
                }else{
                    json!({
                        "kind":"dashboard",
                        "name":book.dashboards[i-book.worksheets.len()]
                    })
                },
                InspectSection::Overview=>Value::Null,
            });
        }
        let mut result=book.overview();
        result["input"]=json!(a.input);
        result["input_sha256"]=json!(pkg.sha256);
        result["package"]=json!(pkg.kind);
        result["items"]=json!(items);
        result["offset"]=json!(a.offset);
        result["total"]=json!(total);
        result["next_offset"]=if end<total{
            json!(end)
        }else{
            Value::Null
        };
        Ok(result)
    }
    fn apply(&self,a:Apply,ct:&CancellationToken)->Result<Value>{
        let bytes=fs::read_bounded(&self.ws.input(&a.plan)?,self.cfg.limits.plan_json_bytes)?;
        require(fs::sha256(&bytes)==a.expected_plan_sha256,"STALE_PLAN","Plan file hash changed")?;
        let supplied:edit::Plan=wire::decode(&bytes,self.cfg.limits.plan_json_bytes)?;
        require(supplied.schema_version==1&&supplied.engine_version==env!("CARGO_PKG_VERSION"),"PLAN_VERSION","Plan was made with another engine/schema")?;
        let(pkg,book)=self.load(&supplied.input)?;
        // Rebuild, never execute caller-supplied byte patches as authority.
        let(rebuilt,candidate)=edit::plan_product_owned(&supplied.input,&pkg.sha256,book,supplied.changes.clone(),&self.cfg)?;
        require(supplied==rebuilt,"PLAN_TAMPERED","Recomputed plan differs; re-plan on this engine/config/input")?;
        let out=self.ws.output(&a.output)?;
        let mut result=json!({"output":a.output,"sha256":"0".repeat(64),
            "twb_sha256":rebuilt.candidate_twb_sha256,"delta":rebuilt.delta,
            "plan":a.plan,"plan_sha256":a.expected_plan_sha256,"byte_preservation":"passed",
            "tableau_semantics":"not_run","source_unchanged":true});
        compact_details(&mut result,self.cfg.limits.result_bytes)?;
        rest::cancelled(ct)?;
        let sha=pkg.write_planned_candidate(&out,&candidate,&self.cfg.limits)?;
        result["sha256"]=json!(sha);
        Ok(result)
    }
    /// The same hard gate is executed at prepare AND immediately before confirm's POST.
    fn publish_checks(&self,a:&PreparePublish,pkg:&Package,book:&Workbook,ct:&CancellationToken)->Result<validation::LocalValidation>{
        require(pkg.sha256==a.expected_sha256,"STALE_BASE","Candidate differs from expected hash")?;
        book.require_2025()?;
        let local=validation::local(book);
        local.require_passed()?;
        require(local.unsupported_objects.is_empty()||a.acknowledge_unsupported_objects,"UNSUPPORTED_ACK_REQUIRED","Review unsupported preserved constructs separately; this cannot override known errors")?;
        require(a.acknowledge_tableau_not_run,"ACK_REQUIRED","Acknowledge that Tableau 2025 execution is not locally validated")?;
        match (&a.test_suite,&a.expected_suite_sha256){
            (Some(path),Some(hash))=>{
                let bytes=fs::read_bounded(&self.ws.input(path)?,self.cfg.limits.input_json_bytes)?;
                require(fs::sha256(&bytes)==*hash,"STALE_SUITE","Publish suite changed")?;
                let suite=wire::decode(&bytes,self.cfg.limits.input_json_bytes)?;
                let report=assertions::run(&self.cfg,&self.ws,pkg,book,&suite,ct)?;
                require(report.passed,"PUBLISH_TEST_FAILED","Candidate assertion suite failed")?;
            }
            (None,None)=>require(!self.cfg.policy.require_publish_tests,"PUBLISH_TEST_REQUIRED","Operator policy requires a hash-bound assertion suite")?,
            _=>return Err(Error::new("ARGUMENT","test_suite and expected_suite_sha256 must be supplied together")),
        }
        Ok(local)
    }

    fn prepare_publish(&self,a:PreparePublish,ct:&CancellationToken)->Result<Value>{
        self.publish_policy(&a)?;
        let(pkg,book)=self.load(&a.input)?;
        let local=self.publish_checks(&a,&pkg,&book,ct)?;
        let mut baseline=None;
        let server=self.remote(ct,|r|{
            if let Some(id)=&a.overwrite_workbook_id{
                let b=r.workbook(id,ct)?;
                check_destination(&b,&a)?;
                baseline=Some(b);
            }
            Ok(r.identity())
        })?;
        let id=uuid::Uuid::new_v4().to_string();
        let expires_at=now()?.checked_add(600).ok_or_else(||Error::new("CLOCK","Timestamp overflow"))?;
        let approval=Approval{
            schema_version:1,
            engine_version:env!("CARGO_PKG_VERSION").into(),
            id:id.clone(),
            expires_at,
            request:a.clone(),
            server,
            baseline
        };
        let bytes=wire::encode(&approval,self.cfg.limits.input_json_bytes as usize,false)?;
        let mut result=json!({
            "approval_id":id,
            "approval_sha256":fs::sha256(&bytes),
            "expires_at":expires_at,
            "destination":{
                "name":a.name,
                "project_id":a.project_id,
                "overwrite_id":a.overwrite_workbook_id
            },
            "candidate_sha256":pkg.sha256,
            "local_validation":local,
            "tests":if a.test_suite.is_some(){"passed"}else{"not_run"},
            "suite_sha256":a.expected_suite_sha256,
            "tableau_semantics":"not_run",
            "warning":"Confirm only after reviewing destination and candidate. Overwrite freshness check is not a server-side atomic CAS."
        });
        compact_details(&mut result,self.cfg.limits.result_bytes)?;
        rest::cancelled(ct)?;
        self.ws.internal_write(&format!("{id}.approval.json"),&bytes)?;
        Ok(result)
    }
    fn publish_policy(&self,a:&PreparePublish)->Result<()>{
        require(self.cfg.policy.publish_enabled,"PUBLISH_DISABLED","Publishing is disabled by operator configuration")?;
        let t=self.cfg.tableau.as_ref().ok_or_else(||Error::new("CAPABILITY_UNAVAILABLE","REST is not configured"))?;
        rest::luid(&a.project_id)?;
        require(t.publish_projects.contains(&a.project_id),"PROJECT_POLICY","Destination is outside the configured publish project allowlist")?;
        require(!a.name.trim().is_empty()&&a.name.len()<=256,"ARGUMENT","Publish name must be 1..256 bytes")?;
        if let Some(id)=&a.overwrite_workbook_id{
            rest::luid(id)?;
            require(self.cfg.policy.allow_overwrite,"OVERWRITE_DISABLED","Overwrite is disabled by operator policy")?;
        }
        Ok(())
    }
    fn confirm_publish(&self,a:ConfirmPublish,ct:&CancellationToken)->Result<Value>{
        approval_id(&a.approval_id)?;
        require(a.confirm,"CONFIRMATION_REQUIRED","Explicit confirmation is required")?;
        let bytes=self.ws.internal_read(&format!("{}.approval.json",a.approval_id),self.cfg.limits.input_json_bytes)?;
        require(fs::sha256(&bytes)==a.expected_approval_sha256,"STALE_APPROVAL","Approval differs from reviewed content")?;
        let p:Approval=wire::decode(&bytes,self.cfg.limits.input_json_bytes)?;
        require(p.schema_version==1&&p.engine_version==env!("CARGO_PKG_VERSION")&&p.id==a.approval_id,"APPROVAL_VERSION","Invalid approval contract")?;
        require(now()?<=p.expires_at,"APPROVAL_EXPIRED","Approval expired; prepare again")?;
        self.publish_policy(&p.request)?;
        let (pkg,book)=self.load(&p.request.input)?;
        self.publish_checks(&p.request,&pkg,&book,ct)?;
        let path=self.ws.input(&p.request.input)?;
        require(fs::hash_file(&path,self.cfg.limits.file_bytes)?==p.request.expected_sha256,"STALE_BASE","Candidate changed since approval")?;
        self.remote(ct,|r|{
            require(r.identity()==p.server,"DESTINATION_CHANGED","REST identity differs from approval")?;
            if let Some(expected)=&p.baseline{
                let id=p.request.overwrite_workbook_id.as_deref().ok_or_else(||Error::new("APPROVAL","Missing overwrite id"))?;
                let current=r.workbook(id,ct)?;
                check_destination(&current,&p.request)?;
                require(current==*expected,"REMOTE_CHANGED","Remote workbook changed since approval")?;
            }
            rest::cancelled(ct)?;
            let mut marker=fs::claim(&self.ws.state().join(format!("{}.attempt",p.id)))?;
            marker.write_all(b"publish attempt started; no automatic replay\n")?;
            marker.sync_all()?;
            fs::sync_dir(self.ws.state())?;
            let outcome=r.publish(&path,&p.request.expected_sha256,&p.request.name,&p.request.project_id,p.request.overwrite_workbook_id.is_some(),p.baseline.as_ref(),&self.cfg,ct);
            let receipt=match outcome{
                Ok(v)=>json!({
                    "approval_id":p.id,
                    "outcome":v
                }),
                Err(e)=>json!({
                    "approval_id":p.id,
                    "status":"failed_or_unknown",
                    "error":e,
                    "instruction":"Inspect remote state before creating a new approval. No automatic retry."
                }),
            };
            let receipt_bytes=wire::encode(&receipt,self.cfg.limits.result_bytes,false)
                .map_err(|_|Error::new("RECEIPT_LIMIT","Remote attempt completed but receipt exceeds budget; inspect remote state before retrying").details(json!({"approval_id":p.id,"status":"outcome_unknown"})))?;
            if self.ws.internal_write(&format!("{}.receipt.json",p.id),&receipt_bytes).is_err(){
                return Err(Error::new("RECEIPT_PERSIST_FAILED","Remote outcome exists but local receipt could not be saved; inspect server before retrying").details(receipt));
            }
            Ok(receipt)
        })
    }
}
fn active_lineage<'a>(slot:&'a Option<LineageSession>,id:&str)->Result<&'a LineageSession>{
    slot.as_ref().filter(|s|s.id==id).ok_or_else(||Error::new("STALE_SNAPSHOT",
        "Lineage snapshot is absent or has been replaced; reopen the workbook"))
}
fn active_lineage_mut<'a>(slot:&'a mut Option<LineageSession>,id:&str)->Result<&'a mut LineageSession>{
    slot.as_mut().filter(|s|s.id==id).ok_or_else(||Error::new("STALE_SNAPSHOT",
        "Lineage snapshot is absent or has been replaced; reopen the workbook"))
}
fn parse<T:DeserializeOwned>(args:Value)->Result<T>{
    serde_json::from_value(args).map_err(|e|Error::new("ARGUMENT",e.to_string()))
}
fn now()->Result<u64>{
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|Error::new("CLOCK","System clock precedes epoch"))?.as_secs())
}
fn approval_id(s:&str)->Result<()>{
    rest::luid(s)
}
fn check_destination(v:&Value,a:&PreparePublish)->Result<()>{
    require(v["name"].as_str()==Some(a.name.as_str())&&v.pointer("/project/id").and_then(Value::as_str)==Some(a.project_id.as_str()),"DESTINATION_CHANGED","Overwrite target name/project differs")?;
    require(v["updatedAt"].as_str().is_some(),"REST_SHAPE","Overwrite requires an updatedAt baseline")
}

/// Side-effect replies are budgeted before local commit. Full deltas live in the plan.
fn compact_details(result:&mut Value,max:usize)->Result<()> {
    if wire::encoded_len(result,max).is_ok(){return Ok(());}
    if let Some(object)=result.as_object_mut(){
        for key in ["delta","warnings"] {
            if let Some(v)=object.remove(key){
                object.insert(format!("{key}_count"),json!(v.as_array().map(Vec::len)));
            }
        }
        if let Some(v)=object.get_mut("local_validation") {
            *v=json!({"passed":v["passed"],"status":v["status"],"unsupported_count":v["unsupported_objects"].as_array().map(Vec::len)});
        }
        object.insert("details_truncated".into(),json!(true));
    }
    wire::encoded_len(result,max)?; Ok(())
}
