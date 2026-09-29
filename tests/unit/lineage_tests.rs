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
    assert_eq!(result["coverage"]["status"], "partial");
    let graph: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("graph.json")).unwrap()).unwrap();
    assert!(
        graph["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "EXTRACT_FILTER")
    );
    assert!(
        !graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["reference"]["kind"] == "datasource_filter")
    );
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
