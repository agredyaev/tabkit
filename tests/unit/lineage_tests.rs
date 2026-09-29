use crate::{
    app::App,
    config::{Config, Limits, Policy},
};
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

fn source() -> String {
    let base = include_str!("../fixtures/new-reference.before.twb").replace("\r\n", "\n");
    let with_filter = base.replace("    </datasource>\n    <datasource name='Parameters'",
        "      <column name='[Ratio2]' caption='Ratio 2' datatype='real' role='measure'><calculation class='tableau' formula='[Calculation_Ratio] * 2'/></column>\n      <filter class='categorical' column='[Region]'/>\n    </datasource>\n    <datasource name='Parameters'");
    assert_ne!(with_filter, base);
    let with_shelves = with_filter.replace("      </table>",
        "        <rows>[ds_orders].[none:Region:nk]</rows>\n        <cols>[ds_orders].[Ratio2]</cols>\n      </table>");
    assert_ne!(with_shelves, with_filter);
    let with_other = with_shelves.replace("  </worksheets>",
        "    <worksheet name='Parameter only'><table><view><datasources><datasource name='Parameters'/></datasources></view></table></worksheet>\n  </worksheets>");
    assert_ne!(with_other, with_shelves);
    let nested = with_other
        .replace(
            "<zones><zone id='1'",
            "<zones><zone id='0' type-v2='layout-basic'><zone id='1'",
        )
        .replace("</zones></dashboard>", "</zone></zones></dashboard>");
    assert_ne!(nested, with_other);
    nested
}
fn setup(source: &str, limits: Limits) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.twb"), source).unwrap();
    let app = App::new(Config {
        workspace: dir.path().canonicalize().unwrap(),
        limits,
        policy: Policy::default(),
        tableau: None,
        hyper: None,
    })
    .unwrap();
    (dir, app)
}
fn call(app: &App, name: &str, args: Value) -> crate::error::Result<Value> {
    app.dispatch(name, args, &CancellationToken::new())
}
fn has(edges: &[Value], from: (&str, u64), to: (&str, u64), kind: &str) -> bool {
    edges.iter().any(|e| {
        e["from"]["kind"] == from.0
            && e["from"]["id"] == from.1
            && e["to"]["kind"] == to.0
            && e["to"]["id"] == to.1
            && e["kind"] == kind
    })
}

#[test]
fn structured_source_and_ui_routes_have_details_without_xml() {
    let source=r#"<workbook source-build='2025.3.1' xmlns:user='http://www.tableausoftware.com/xml/user'>
      <datasources>
        <datasource name='d'>
          <named-connections><named-connection name='c'><connection class='postgres' one-time-sql='SELECT id FROM audit.events'/></named-connection></named-connections>
          <column name='[id]' datatype='integer' role='dimension'/>
          <column name='[calc]' datatype='integer' role='measure'><calculation class='tableau' formula='[id]'><table-calc ordering-type='Field' ordering-field='[d].[id]'><order field='[d].[id]'/><order field='[d].[id set]'/></table-calc></calculation></column>
          <column name='[setcalc]' datatype='boolean' role='dimension'><calculation class='tableau' formula='[id set]'/></column>
          <metadata-records><metadata-record class='column'><local-name>[id]</local-name><parent-name>[q]</parent-name><local-type>integer</local-type></metadata-record><metadata-record class='column'><local-name>[id]</local-name><parent-name>[ExtractAlias]</parent-name><local-type>integer</local-type></metadata-record></metadata-records>
          <group name='[id set]' user:ui-builder='filter-group'><groupfilter function='level-members' level='[id]'/><groupfilter function='except' level='[id]'/></group>
          <object-graph><objects>
            <object id='orders' caption='Orders'><properties context=''><relation type='join' join='left'><clause type='join'><expression op='='><expression op='[id]'/><expression op='[id]'/></expression></clause><clause type='join'><expression op='&lt;'><expression op='[id]'/><expression op='[id]'/></expression></clause><relation type='text' name='q' connection='c'>WITH cte AS (SELECT id FROM public.orders) SELECT id FROM cte</relation><relation type='table' name='labels' table='[public].[labels]' connection='c'/></relation></properties><properties context='extract'><relation type='table' name='ExtractAlias' table='[Extract].[Extract]'/></properties></object>
            <object id='lookup' caption='Lookup'><properties context=''><relation type='table' name='lookup' table='[public].[lookup]' connection='c'/></properties><properties context='extract'><relation type='table' name='ExtractAlias' table='[Extract].[Extract]'/></properties></object>
          </objects><relationships><relationship><expression op='='><expression op='[id]'/><expression op='[id]'/></expression><first-end-point object-id='orders'/><second-end-point object-id='lookup'/></relationship></relationships></object-graph>
        </datasource>
        <datasource name='Parameters'><column name='[P]' datatype='integer' role='measure' param-domain-type='list' value='1'><calculation class='tableau' formula='1'/></column></datasource>
      </datasources>
      <worksheets><worksheet name='S'><table><view><filter class='categorical' column='[d].[id set]'/><manual-sort column='[d].[yr:id:ok]'/></view><panes><pane><add-in><type-settings><worksheet/></type-settings></add-in><encodings><color column='[d].[io:id set:nk]'/><custom custom-type-name='target' column='[d].[sum:id:qk]'/></encodings><manual-sort column='[d].[io:id set:nk]'/><customized-tooltip><formatted-text><run>&lt;[d].[id]&gt;</run><run>&lt;Sheet name=&quot;S&quot; filter=&quot;&lt;All Fields&gt;&quot;&gt;</run></formatted-text></customized-tooltip></pane></panes><rows>[d].[io:id set:nk]</rows></table></worksheet></worksheets>
      <dashboards><dashboard name='D'><zones><zone name='S'/><zone type-v2='paramctrl' param='[Parameters].[P]' mode='compact'/><zone><flipboard><story-points><story-point id='1' caption='First' captured-sheet='S'/></story-points></flipboard></zone></zones></dashboard></dashboards>
      <actions><edit-parameter-action name='a'><activation type='on-select'/><source type='sheet' worksheet='S'/><params><param name='source-field' value='[d].[id]'/><param name='target-parameter' value='[Parameters].[P]'/></params></edit-parameter-action><edit-group-action name='b'><activation type='on-select'/><source type='sheet' worksheet='S'/><params><param name='target-group' value='[d].[id set]'/></params></edit-group-action></actions>
    </workbook>"#;
    let (dir, app)=setup(source,Limits::default());
    let opened=call(&app,"workbook_lineage_open",json!({"input":"in.twb"})).unwrap();
    let snapshot=opened["snapshot_id"].as_str().unwrap();
    let found=call(&app,"workbook_lineage_find",json!({"snapshot_id":snapshot,"prefix":"orders","kind":"logical_table"})).unwrap();
    let node=found["items"][0]["reference"].clone();
    let detail=call(&app,"workbook_lineage_details",json!({"snapshot_id":snapshot,"node":node})).unwrap();
    assert_eq!(detail["details"]["object_id"],"orders");
    let impact=call(&app,"workbook_lineage_impact",json!({"snapshot_id":snapshot,"from":node,"limit":100})).unwrap();
    assert!(!impact["edges"].as_array().unwrap().iter().any(|e|e["kind"]=="relationship_end"));
    let exported=call(&app,"workbook_lineage_export",json!({"snapshot_id":snapshot,"output":"graph.json"})).unwrap();
    assert_eq!(exported["input_sha256"],opened["input_sha256"]);
    let graph:Value=serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert_eq!(graph["schema_version"],2);
    let kinds=graph["nodes"].as_array().unwrap().iter().map(|n|n["reference"]["kind"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
    for kind in ["connection","physical_table","logical_table","custom_sql","initial_sql","join","relationship","set","action","tooltip","parameter_control","story","story_point","table_calculation","custom_encoding"] {
        assert!(kinds.contains(kind),"missing {kind}");
    }
    let edges=graph["edges"].as_array().unwrap();
    for kind in ["sql_read","join_input","join_output","relationship_end","field_origin","extract_field_origin","extract_in_logical","group_input","action_input","action_target","tooltip_input","tooltip_sheet","control_parameter","story_contains","table_calculation_input","table_calculation_order","custom_encoding_field","custom_encoding_on_sheet"] {
        assert!(edges.iter().any(|e|e["kind"]==kind),"missing {kind}");
    }
    assert_eq!(edges.iter().filter(|e|e["kind"]=="extract_in_logical").count(),2);
    assert!(has(edges,("field",0),("worksheet",0),"sort_field"));
    for kind in ["rows","mark_color","sort_field","worksheet_filter_field","group_use","table_calculation_order"] {
        assert!(edges.iter().any(|e|e["from"]["kind"]=="set" && e["kind"]==kind),"set missing {kind}");
    }
    let join=graph["nodes"].as_array().unwrap().iter().position(|n|n["reference"]["kind"]=="join").unwrap();
    assert_eq!(graph["details"][join]["condition"]["op"],"=");
    assert_eq!(graph["details"][join]["conditions"].as_array().unwrap().len(),2);
    let set=graph["nodes"].as_array().unwrap().iter().position(|n|n["reference"]["kind"]=="set").unwrap();
    assert_eq!(graph["details"][set]["definition"].as_array().unwrap().len(),2);
    let custom=graph["nodes"].as_array().unwrap().iter().position(|n|n["reference"]["kind"]=="custom_encoding").unwrap();
    assert_eq!(graph["details"][custom]["custom_type"],"target");
    assert!(!graph["gaps"].as_array().unwrap().iter().any(|g|["UNRESOLVED_REFERENCE","SHELF_REFERENCE","MARK_REFERENCE","FILTER_REFERENCE","COLUMN_BINDING","TABLE_CALC_ORDER","SORT_REFERENCE","CUSTOM_ENCODING_FIELD"].contains(&g["code"].as_str().unwrap_or(""))));
    assert!(!graph["nodes"].as_array().unwrap().iter().any(|n|n["reference"]["kind"]=="physical_table" && n["name"]=="cte"));
}

#[test]
fn duplicate_set_names_are_ambiguous_and_do_not_create_view_links() {
    let source="<workbook xmlns:user='http://www.tableausoftware.com/xml/user'><datasources><datasource name='d'><column name='[id]'/><group name='[S]' user:ui-builder='filter-group'/><group name='[S]' user:ui-builder='filter-group'/></datasource></datasources><worksheets><worksheet name='W'><table><view><datasources><datasource name='d'/></datasources></view><rows>[d].[io:S:nk]</rows></table></worksheet></worksheets></workbook>";
    let (dir,app)=setup(source,Limits::default());
    call(&app,"workbook_lineage_export",json!({"input":"in.twb","output":"graph.json"})).unwrap();
    let graph:Value=serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert!(graph["gaps"].as_array().unwrap().iter().any(|g|g["code"]=="GROUP_REFERENCE"));
    assert!(graph["gaps"].as_array().unwrap().iter().any(|g|g["code"]=="SHELF_REFERENCE"));
    assert!(!graph["edges"].as_array().unwrap().iter().any(|e|e["from"]["kind"]=="set" && e["kind"]=="rows"));
}

#[test]
fn overlapping_source_and_extract_aliases_do_not_claim_field_origin() {
    let source="<workbook><datasources><datasource name='d'><column name='[id]'/><metadata-records><metadata-record class='column'><local-name>[id]</local-name><parent-name>[t]</parent-name></metadata-record></metadata-records><object-graph><objects><object id='o'><properties context=''><relation type='table' name='t' table='[public].[t]'/></properties><properties context='extract'><relation type='table' name='t' table='[Extract].[t]'/></properties></object></objects></object-graph></datasource></datasources></workbook>";
    let (dir,app)=setup(source,Limits::default());
    call(&app,"workbook_lineage_export",json!({"input":"in.twb","output":"graph.json"})).unwrap();
    let graph:Value=serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert!(graph["gaps"].as_array().unwrap().iter().any(|g|g["code"]=="FIELD_ORIGIN"));
    assert!(!graph["edges"].as_array().unwrap().iter().any(|e|matches!(e["kind"].as_str(),Some("field_origin"|"extract_field_origin"))));
}

#[test]
fn lineage_api_rejects_unknown_nodes_and_stale_cursors() {
    let (_dir,app)=setup(&source(),Limits::default());
    let opened=call(&app,"workbook_lineage_open",json!({"input":"in.twb"})).unwrap();
    let id=&opened["snapshot_id"];
    assert_eq!(call(&app,"workbook_lineage_details",json!({"snapshot_id":id,"node":{"kind":"field","id":999999}})).unwrap_err().code,"TARGET_NOT_FOUND");
    assert_eq!(call(&app,"workbook_lineage_neighbors",json!({"snapshot_id":id,"node":{"kind":"field","id":999999},"direction":"downstream"})).unwrap_err().code,"TARGET_NOT_FOUND");
    assert_eq!(call(&app,"workbook_lineage_impact",json!({"snapshot_id":id,"from":{"kind":"field","id":0},"cursor":"bogus"})).unwrap_err().code,"INPUT");
    assert_eq!(call(&app,"workbook_lineage_impact",json!({"snapshot_id":id,"cursor":"bogus"})).unwrap_err().code,"STALE_CURSOR");
}

#[test]
fn lineage_routes_impact_and_snapshot_lifecycle() {
    let source = source();
    let (dir, app) = setup(&source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    let snapshot = opened["snapshot_id"].as_str().unwrap();
    let found = call(
        &app,
        "workbook_lineage_find",
        json!({"snapshot_id":snapshot,"prefix":"ratio"}),
    )
    .unwrap();
    assert_eq!(found["total"], 1);
    assert_eq!(
        found["items"][0]["reference"],
        json!({"kind":"field","id":5})
    );
    let by_name = call(
        &app,
        "workbook_lineage_find",
        json!({"snapshot_id":snapshot,"prefix":"[Ratio2]","by":"name"}),
    )
    .unwrap();
    assert_eq!(
        by_name["items"][0]["reference"],
        json!({"kind":"field","id":5})
    );
    let neighbors = call(
        &app,
        "workbook_lineage_neighbors",
        json!({"snapshot_id":snapshot,
        "node":{"kind":"field","id":1},"direction":"downstream"}),
    )
    .unwrap();
    assert!(has(
        &neighbors["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["edge"].clone())
            .collect::<Vec<_>>(),
        ("field", 1),
        ("field", 3),
        "calculation"
    ));
    let exported = call(
        &app,
        "workbook_lineage_export",
        json!({"snapshot_id":snapshot,"output":"graph.json"}),
    )
    .unwrap();
    let graph: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert_eq!(graph["input_sha256"], opened["input_sha256"]);
    assert_eq!(
        exported["sha256"],
        crate::fs::hash_file(&dir.path().join("graph.json"), u64::MAX).unwrap()
    );
    let edges = graph["edges"].as_array().unwrap();
    for (from, to, kind) in [
        (("field", 1), ("field", 3), "calculation"),
        (("field", 3), ("field", 5), "calculation"),
        (("field", 2), ("worksheet", 0), "rows"),
        (("field", 5), ("worksheet", 0), "cols"),
        (("field", 2), ("worksheet", 0), "mark_color"),
        (
            ("field", 2),
            ("worksheet_filter", 0),
            "worksheet_filter_field",
        ),
        (
            ("worksheet_filter", 0),
            ("worksheet", 0),
            "worksheet_filtered",
        ),
        (
            ("field", 2),
            ("datasource_filter", 0),
            "datasource_filter_field",
        ),
        (
            ("datasource_filter", 0),
            ("datasource", 0),
            "datasource_filtered",
        ),
        (("datasource", 0), ("worksheet", 0), "datasource_used"),
        (("worksheet", 0), ("dashboard", 0), "sheet_in_dashboard"),
    ] {
        assert!(
            has(edges, from, to, kind),
            "missing {from:?} -> {to:?}: {kind}"
        );
    }
    assert!(!has(
        edges,
        ("datasource", 0),
        ("worksheet", 1),
        "datasource_used"
    ));
    let mut cursor = None;
    let mut visited = Vec::new();
    loop {
        let args = if let Some(cursor) = cursor {
            json!({"snapshot_id":snapshot,"cursor":cursor,"limit":1})
        } else {
            json!({"snapshot_id":snapshot,"from":{"kind":"field","id":1},
                "direction":"downstream","limit":1})
        };
        let page = call(&app, "workbook_lineage_impact", args).unwrap();
        visited.extend(page["edges"].as_array().unwrap().iter().cloned());
        if page["done"] == true {
            break;
        }
        cursor = Some(page["cursor"].as_str().unwrap().to_owned());
    }
    assert!(has(&visited, ("field", 1), ("field", 3), "calculation"));
    assert!(has(&visited, ("field", 3), ("field", 5), "calculation"));
    assert!(has(
        &visited,
        ("worksheet", 0),
        ("dashboard", 0),
        "sheet_in_dashboard"
    ));
    let reopened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    assert_ne!(reopened["snapshot_id"], opened["snapshot_id"]);
    assert_eq!(
        call(
            &app,
            "workbook_lineage_find",
            json!({"snapshot_id":snapshot,"prefix":"ratio"})
        )
        .unwrap_err()
        .code,
        "STALE_SNAPSHOT"
    );
}

#[test]
fn cycles_terminate_and_unresolved_references_remain_visible() {
    let source = "<workbook><datasources><datasource name='d'>\
        <column name='[A]'><calculation class='tableau' formula='[B]'/></column>\
        <column name='[B]'><calculation class='tableau' formula='[A]'/></column>\
        <column name='[C]'><calculation class='tableau' formula='[Missing]'/></column>\
        </datasource></datasources></workbook>";
    let (_dir, app) = setup(source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    assert_eq!(opened["coverage"]["status"], "partial");
    assert_eq!(opened["gap_codes"]["UNRESOLVED_REFERENCE"], 1);
    let snapshot = opened["snapshot_id"].as_str().unwrap();
    let page = call(
        &app,
        "workbook_lineage_impact",
        json!({"snapshot_id":snapshot,
        "from":{"kind":"field","id":0},"limit":100}),
    )
    .unwrap();
    assert_eq!(page["done"], true);
    assert_eq!(page["edges"].as_array().unwrap().len(), 2);
    assert_eq!(page["nodes"].as_array().unwrap().len(), 2);
}

#[test]
fn extract_filter_is_not_reported_as_datasource_filter() {
    let source = source().replace(
        "<filter class='categorical' column='[Region]'/>",
        "<extract><filter class='categorical' column='[Region]'/></extract>",
    );
    let (dir, app) = setup(&source, Limits::default());
    let result = call(
        &app,
        "workbook_lineage_export",
        json!({"input":"in.twb","output":"graph.json"}),
    )
    .unwrap();
    assert_eq!(result["coverage"]["status"], "complete_for_v2_routes");
    let graph: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["reference"]["kind"] == "extract_filter")
    );
    let edges = graph["edges"].as_array().unwrap();
    assert!(has(
        edges,
        ("field", 2),
        ("extract_filter", 0),
        "extract_filter_field"
    ));
    assert!(has(
        edges,
        ("extract_filter", 0),
        ("datasource", 0),
        "extract_filtered"
    ));
    assert!(
        !graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["reference"]["kind"] == "datasource_filter")
    );
}

#[test]
fn shared_view_filter_has_a_field_but_no_assumed_worksheet_effect() {
    let source = source().replacen(
        "</datasources>",
        "</datasources><shared-views><shared-view name='ds_orders'><filter class='categorical' column='[ds_orders].[Region]'/></shared-view></shared-views>",
        1,
    );
    let (dir, app) = setup(&source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    assert_eq!(opened["gap_codes"]["SHARED_VIEW_SCOPE"], 1);
    call(
        &app,
        "workbook_lineage_export",
        json!({
            "snapshot_id":opened["snapshot_id"],"output":"graph.json"
        }),
    )
    .unwrap();
    let graph: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    let edges = graph["edges"].as_array().unwrap();
    assert!(has(
        edges,
        ("field", 2),
        ("shared_view_filter", 0),
        "shared_view_filter_field"
    ));
    assert!(
        !edges
            .iter()
            .any(|e| e["from"] == json!({"kind":"shared_view_filter","id":0}))
    );
}

#[test]
fn local_calculation_connects_source_to_shelf_and_dashboard() {
    let source = source().replacen(
        "</datasource-dependencies>",
        "<column name='[Tmp]' caption='Local total'><calculation class='tableau' formula='[Sales] * 2'/></column><column-instance name='[usr:Tmp:qk]' column='[Tmp]'/></datasource-dependencies>",
        1,
    )
    .replace("<cols>[ds_orders].[Ratio2]</cols>", "<cols>[ds_orders].[usr:Tmp:qk]</cols>")
    .replace("</view>", "<computed-sort column='[ds_orders].[usr:Tmp:qk]' using='[ds_orders].[none:Sales:qk]'/></view>")
    .replace("<color column='[ds_orders].[none:Region:nk]'/>", "<color column='[ds_orders].[none:Region:nk]'/><wedge-size column='[ds_orders].[usr:Tmp:qk]'/>");
    let (dir, app) = setup(&source, Limits::default());
    call(
        &app,
        "workbook_lineage_export",
        json!({"input":"in.twb","output":"graph.json"}),
    )
    .unwrap();
    let graph: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    let local = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["reference"]["kind"] == "local_field" && n["name"] == "[Tmp]")
        .unwrap();
    let id = local["reference"]["id"].as_u64().unwrap();
    assert_eq!(local["formula"], "[Sales] * 2");
    assert_eq!(local["worksheet"], json!({"kind":"worksheet","id":0}));
    let edges = graph["edges"].as_array().unwrap();
    assert!(has(edges, ("field", 0), ("local_field", id), "calculation"));
    assert!(has(edges, ("local_field", id), ("worksheet", 0), "cols"));
    assert!(has(
        edges,
        ("local_field", id),
        ("worksheet", 0),
        "sort_field"
    ));
    assert!(has(edges, ("field", 0), ("worksheet", 0), "sort_using"));
    assert!(has(
        edges,
        ("local_field", id),
        ("worksheet", 0),
        "mark_wedge_size"
    ));
    assert!(has(
        edges,
        ("worksheet", 0),
        ("dashboard", 0),
        "sheet_in_dashboard"
    ));
    assert!(
        !graph["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "LOCAL_DEFINITION")
    );
}

#[test]
fn lineage_gaps_identify_the_affected_worksheet() {
    let source = source().replace(
        "<color column='[ds_orders].[none:Region:nk]'/>",
        "<color column='[ds_orders].[none:Region:nk]'/><unknown-binding column='[ds_orders].[Sales]'/>",
    );
    let (_dir, app) = setup(&source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    let page = call(
        &app,
        "workbook_lineage_gaps",
        json!({
            "snapshot_id":opened["snapshot_id"],"limit":1
        }),
    )
    .unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["code"], "COLUMN_BINDING");
    assert!(
        page["items"][0]["context"]
            .as_str()
            .unwrap()
            .contains("worksheet=Sales by Region")
    );
}

#[test]
fn datasource_filter_details_keep_the_member_condition() {
    let source = "<workbook><datasources><datasource name='d'><column name='[id]'/><filter class='categorical' column='[id]'><groupfilter function='member' level='[id]' member='1'/></filter></datasource></datasources><worksheets><worksheet name='S'><table><view><datasources><datasource name='d'/></datasources></view></table></worksheet></worksheets></workbook>";
    let (dir, app) = setup(source, Limits::default());
    call(&app, "workbook_lineage_export", json!({"input":"in.twb","output":"graph.json"})).unwrap();
    let graph: Value = serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    let filter = graph["nodes"].as_array().unwrap().iter().position(|n|n["reference"]["kind"]=="datasource_filter").unwrap();
    assert_eq!(graph["details"][filter]["attributes"]["class"], "categorical");
    assert_eq!(graph["details"][filter]["children"][0]["attributes"]["member"], "1");
}

#[test]
fn unresolved_custom_encoding_and_manual_sort_are_reported() {
    let source = "<workbook><datasources><datasource name='d'><column name='[id]'/></datasource></datasources><worksheets><worksheet name='S'><table><view><datasources><datasource name='d'/></datasources><manual-sort column='[d].[missing]'/></view><panes><pane><encodings><custom custom-type-name='target' column='[d].[missing]'/></encodings></pane></panes></table></worksheet></worksheets></workbook>";
    let (_dir, app) = setup(source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    assert_eq!(opened["gap_codes"]["SORT_REFERENCE"], 1);
    assert_eq!(opened["gap_codes"]["CUSTOM_ENCODING_FIELD"], 1);
    assert!(opened["gap_codes"].get("COLUMN_BINDING").is_none());
}

#[test]
fn exact_field_name_precedes_generated_wrapper_fallback() {
    let source = "<workbook><datasources><datasource name='d'><column name='[id]'/><column name='[sum:id:qk]'/></datasource></datasources><worksheets><worksheet name='S'><table><view><datasources><datasource name='d'/></datasources></view><panes><pane><encodings><custom custom-type-name='target' column='[d].[sum:id:qk]'/></encodings></pane></panes></table></worksheet></worksheets></workbook>";
    let (dir, app) = setup(source, Limits::default());
    call(&app, "workbook_lineage_export", json!({"input":"in.twb","output":"graph.json"})).unwrap();
    let graph: Value = serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    let edges = graph["edges"].as_array().unwrap();
    assert!(has(edges, ("field", 1), ("custom_encoding", 0), "custom_encoding_field"));
    assert!(!has(edges, ("field", 0), ("custom_encoding", 0), "custom_encoding_field"));
}

#[test]
fn differing_worksheet_formula_copy_is_a_coverage_gap() {
    let source = source();
    let marker = "<datasource-dependencies datasource='ds_orders'>";
    let (before, after) = source.split_once(marker).unwrap();
    let changed = after.replacen(
        "formula='[Profit] / [Sales]'",
        "formula='[Profit] + [Sales]'",
        1,
    );
    assert_ne!(changed, after);
    let source = format!("{before}{marker}{changed}");
    let (_dir, app) = setup(&source, Limits::default());
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    assert_eq!(opened["gap_codes"]["INCONSISTENT_DEFINITION"], 1);
}

#[test]
#[ignore = "release-only performance acceptance on the local host"]
fn lineage_navigation_p95_on_large_synthetic_book() {
    let mut source = String::with_capacity(16_000_000);
    source.push_str("<workbook><datasources><datasource name='d'><column name='[Base]'/>");
    for i in 0..100_000 {
        source.push_str(&format!("<column name='[C{i}]' caption='Calc {i}'><calculation class='tableau' formula='[Base]+1'/></column>"));
    }
    source.push_str("</datasource></datasources><worksheets>");
    for i in 0..1_000 {
        source.push_str(&format!("<worksheet name='Sheet {i}'><table><view><datasources><datasource name='d'/></datasources></view><rows>[d].[Base]</rows></table></worksheet>"));
    }
    source.push_str("</worksheets></workbook>");
    let (_dir, app) = setup(&source, Limits::default());
    let start = Instant::now();
    let opened = call(&app, "workbook_lineage_open", json!({"input":"in.twb"})).unwrap();
    let build = start.elapsed();
    let snapshot = opened["snapshot_id"].as_str().unwrap();
    let mut measurements = [Vec::new(), Vec::new(), Vec::new()];
    for i in 0..500 {
        let offset = i * 100;
        let queries = [
            (
                "workbook_lineage_find",
                json!({"snapshot_id":snapshot,"prefix":"calc 9","limit":100}),
            ),
            (
                "workbook_lineage_neighbors",
                json!({"snapshot_id":snapshot,"node":{"kind":"field","id":0},"direction":"downstream","offset":offset,"limit":100}),
            ),
            (
                "workbook_lineage_impact",
                json!({"snapshot_id":snapshot,"from":{"kind":"field","id":0},"limit":100}),
            ),
        ];
        for (j, (name, args)) in queries.into_iter().enumerate() {
            let start = Instant::now();
            let value = call(&app, name, args).unwrap();
            let _wire = value.to_string();
            measurements[j].push(start.elapsed().as_micros());
        }
    }
    for (name, samples) in ["find", "neighbors", "impact"]
        .into_iter()
        .zip(&mut measurements)
    {
        samples.sort_unstable();
        let p95 = samples[samples.len() * 95 / 100];
        eprintln!(
            "lineage {name} p95={p95}us; cold build={build:?}; xml_bytes={}",
            source.len()
        );
        assert!(p95 <= 10_000, "{name} p95 exceeds 10 ms");
    }
}

#[test]
fn streaming_export_limit_does_not_leave_a_file() {
    let source = source();
    let (dir, app) = setup(&source, Limits::default());
    let error = app
        .ws
        .write_new_stream("too-large.json", 3, |writer| {
            writer.write_all(b"four")?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "LIMIT");
    assert!(!dir.path().join("too-large.json").exists());
}
