//! Tool metadata only. Semantics live in App/core, never in transport handlers.
use crate::{
    app,
    hyper,
    rest,
    error::{
        Error,
        Result
    }
};
use rmcp::model::{
    Tool,
    ToolAnnotations
};
use schemars::JsonSchema;
use std::sync::Arc;
fn tool<T:JsonSchema>(name:&'static str,description:&'static str,read:bool,destructive:bool,world:bool)->Result<Tool>{
    let schema=serde_json::to_value(schemars::schema_for!(T))?;
    let object=schema.as_object().cloned().ok_or_else(||Error::new("SCHEMA","Tool input schema is not an object"))?;
    Ok(Tool::new(name,description,Arc::new(object)).with_annotations(
    ToolAnnotations::new().read_only(read).destructive(destructive).idempotent(read).open_world(world)))
}
pub fn tools()->Result<Vec<Tool>>{
    Ok(vec![
    tool::<app::Empty>("system_status","Capabilities, policies and validation limitations. No secret values.",true,false,false)?,
    tool::<app::Guide>("guidance","Embedded workflow, advisor, critique, calculation review, read-only DQ, about and viewer instructions. Not a remote plugin installation.",true,false,false)?,
    tool::<app::Input>("file_hash","SHA-256 of one workspace file. Use it for snapshot-bound Hyper or edit requests.",true,false,false)?,
    tool::<app::Empty>("tableau_login","Establish the process-local Tableau REST session using the startup-selected OAuth PKCE/EAS or PAT mode. Secrets are never tool arguments or results; rejected sessions require sign-in on the next operation, without replaying mutations.",false,false,true)?,
    tool::<app::Empty>("tableau_logout","Invalidate this process's Tableau session and clear its token.",false,false,true)?,
    tool::<rest::Explore>("tableau_explore","Page through projects/workbooks/views/datasources. Explicit IDs and pagination; no arbitrary endpoint access.",true,false,true)?,
    tool::<rest::Search>("tableau_search","Bounded name-substring search using Tableau 2025 list APIs. Reports incomplete scans.",true,false,true)?,
    tool::<app::Id>("tableau_get_workbook","Get selected workbook metadata, project, owner and modification timestamp.",true,false,true)?,
    tool::<app::Download>("tableau_download_workbook","Download a TWB/TWBX including extracts to a NEW workspace file; verify package format before saving.",false,false,true)?,
    tool::<app::PreparePublish>("tableau_prepare_publish","Prepare a hash-bound, expiring publish approval. Does not publish. Review target and local-validation limits before confirmation.",false,false,true)?,
    tool::<app::ConfirmPublish>("tableau_publish","Use a reviewed single-use approval to stage and publish a workbook. No retries. Default new copy; overwrite requires operator policy.",false,true,true)?,
    tool::<app::Receipt>("tableau_publish_receipt","Read a publish attempt receipt. Missing receipt after an attempt means unknown outcome, not permission to retry.",true,false,false)?,
    tool::<app::Id>("tableau_get_job","Read asynchronous Tableau job status. Submitted is not completed or verified.",true,false,true)?,
    tool::<rest::ViewExport>("tableau_view_image","Export a PNG of a specific Tableau view and return image content. Requires operator data-output permission.",false,false,true)?,
    tool::<rest::ViewExport>("tableau_view_data","Export bounded CSV to a new workspace file. Tableau 2025 dashboard CSV can cover only the first sheet.",false,false,true)?,
    tool::<app::Inspect>("workbook_inspect","Page through overview, fields, calculations, parameters, filters, sheets, references, local_definitions, dependency_scopes, uses or package entries using section/offset/limit. Returned IDs are valid only with this input SHA-256.",true,false,false)?,
    tool::<app::Input>("workbook_lineage_open","Build one read-only lineage snapshot for this MCP session; replaces the previous snapshot. Returns snapshot_id, graph counts and coverage gaps.",true,false,false)?,
    tool::<app::LineageFind>("workbook_lineage_find","Search the open lineage graph by caption or internal-name prefix. IDs are scoped to snapshot_id.",true,false,false)?,
    tool::<app::LineageNeighbors>("workbook_lineage_neighbors","Page through direct upstream or downstream field, filter, sheet and dashboard links in the open graph.",true,false,false)?,
    tool::<app::LineageImpact>("workbook_lineage_impact","Traverse all reachable links in bounded pages. Start with from and direction, then continue with cursor; each new start replaces the prior traversal.",true,false,false)?,
    tool::<app::LineageExport>("workbook_lineage_export","Write a complete deterministic lineage JSON to a NEW workspace file from snapshot_id or input; includes coverage gaps.",false,false,false)?,
    tool::<app::PlanRequest>("workbook_plan","Plan typed formula/parameter/filter edits. Creates a reviewable plan file; does not change the source. No raw XML operation.",false,false,false)?,
    tool::<app::Apply>("workbook_apply","Recompute and verify an approved plan, then create a new TWB/TWBX candidate. Never trusts external raw patches or overwrites the source.",false,false,false)?,
    tool::<app::Input>("workbook_validate","Check XML and known invariants; distinguish valid, partial and invalid. Known local errors block publish. XSD/Tableau execution/business tests are not_run.",true,false,false)?,
    tool::<app::Diff>("workbook_diff","Compare known semantic properties of two books. Not a full Tableau semantic equivalence proof.",true,false,false)?,
    tool::<app::Test>("workbook_test","Run a declarative assertion suite pinned to a workbook hash. Optional Hyper scalar checks evaluate extract SQL, not Tableau calculations.",true,false,false)?,
    tool::<app::Extract>("workbook_extract_hyper","Copy one selected Hyper member from a hash-pinned TWBX to a new file. Does not mutate the package.",false,false,false)?,
    tool::<hyper::Request>("hyper_query","Read-only extract inspection: tables, columns, sample, distinct, min/max, or a conservative SELECT. Native feature/runtime and data-output permission required.",true,false,false)?,
    ])
}
