//! Additional observable-contract tests; no external accounts or native runtime.
use crate::{app::App, config::{Config, Limits, Policy}, fs};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
const GOOD: &str = include_str!("../examples/synthetic.twb");
fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.twb"), GOOD).unwrap();
    std::fs::write(dir.path().join("baseline.twb"), GOOD).unwrap();
    let app = App::new(Config {
        workspace: dir.path().canonicalize().unwrap(), limits: Limits::default(),
        policy: Policy::default(), tableau: None, hyper: None,
    }).unwrap();
    (dir, app)
}
fn run_suite(dir: &std::path::Path, app: &App, assertions: Vec<Value>) -> Value {
    let suite = json!({"schema_version":1,"input_sha256":fs::sha256(GOOD.as_bytes()),"assertions":assertions});
    std::fs::write(dir.join("suite.json"), serde_json::to_vec(&suite).unwrap()).unwrap();
    app.dispatch("workbook_test", json!({"input":"in.twb","suite":"suite.json"}), &CancellationToken::new()).unwrap()
}
fn call(app: &App, name: &str, value: Value) -> crate::error::Result<Value> {
    app.dispatch(name, value, &CancellationToken::new())
}
#[test]
fn every_local_assertion_reports_both_matches_and_mismatches() {
    let (dir, app) = setup();
    let mut checks = vec![
        json!({"assert":"calculation","field_id":3,"formula":"[Profit] / [Sales]"}),
        json!({"assert":"parameter","field_id":4,"current":{"kind":"integer","value":10},"domain":null}),
        json!({"assert":"filter_values","filter_id":0,"values":["West"]}),
        json!({"assert":"file_hash","input":"in.twb","sha256":fs::sha256(GOOD.as_bytes())}),
        json!({"assert":"inventory_unchanged","baseline":"baseline.twb"}),
        json!({"assert":"no_diagnostics"}),
    ];
    let filters = call(&app,"workbook_inspect",json!({"input":"in.twb","section":"filters"})).unwrap();
    let state = &filters["items"][1]["state"];
    checks.push(json!({"assert":"filter_range","filter_id":1,"min":state["min"],"max":state["max"]}));
    let passed = run_suite(dir.path(), &app, checks.clone());
    assert_eq!(passed["passed"], true, "{passed}");
    checks[0]["formula"] = json!("0");
    checks[1]["current"]["value"] = json!(11);
    checks[2]["values"] = json!(["East"]);
    checks[3]["sha256"] = json!("0".repeat(64));
    checks[6]["max"] = json!({"kind":"real","value":"99999"});
    let report = run_suite(dir.path(), &app, checks);
    assert_eq!(report["passed"], false);
    for i in [0, 1, 2, 3, 6] { assert_eq!(report["assertions"][i]["status"], "failed"); }
    for i in [4, 5] { assert_eq!(report["assertions"][i]["status"], "passed"); }
}
#[test]
fn subtree_checks_cover_present_absent_and_changed_containers() {
    let (dir, app) = setup();
    let sections = ["datasources", "worksheets", "dashboards", "windows"];
    let checks: Vec<_> = sections.iter().map(|s| json!({"assert":"subtree_unchanged","baseline":"baseline.twb","section":s})).collect();
    assert_eq!(run_suite(dir.path(), &app, checks.clone())["passed"], true);
    let changed = GOOD.replace("x='0'", "x='17'");
    assert_ne!(changed, GOOD);
    std::fs::write(dir.path().join("baseline.twb"), changed).unwrap();
    let report = run_suite(dir.path(), &app, checks);
    assert_eq!(report["assertions"][2]["status"], "failed");
    for i in [0,1,3] { assert_eq!(report["assertions"][i]["status"], "passed"); }
}
#[cfg(not(feature="hyper"))]
#[test]
fn native_assertion_unavailable_is_failure_not_skipped_success() {
    let (dir, app) = setup();
    let r = run_suite(dir.path(), &app, vec![json!({"assert":"hyper_scalar","input":"sample.hyper",
        "input_sha256":"0","sql":"SELECT SUM(\"Sales\") FROM \"Extract\".\"Extract\"","expected":"1"})]);
    assert_eq!(r["passed"], false);
    assert_eq!(r["assertions"][0]["status"], "failed");
    assert_eq!(r["assertions"][0]["error"]["code"], "CAPABILITY_UNAVAILABLE");
}
#[test]
fn missing_baseline_does_not_hide_other_assertion_results() {
    let (dir, app) = setup();
    let r = run_suite(dir.path(), &app, vec![json!({"assert":"inventory_unchanged","baseline":"missing.twb"}),json!({"assert":"no_diagnostics"})]);
    assert_eq!(r["passed"], false); assert_eq!(r["assertions"][1]["status"], "passed");
}
#[test]
fn inspection_pagination_reassembles_all_sections_without_losing_ids() {
    let (_dir, app) = setup();
    for section in ["fields","calculations","parameters","filters","datasources","sheets","references","diagnostics","local_definitions","dependency_scopes","uses","package"] {
        let all=call(&app,"workbook_inspect",json!({"input":"in.twb","section":section,"limit":1000})).unwrap();
        let mut offset=0; let mut actual=Vec::new();
        loop {
            let page=call(&app,"workbook_inspect",json!({"input":"in.twb","section":section,"offset":offset,"limit":1})).unwrap();
            actual.extend(page["items"].as_array().unwrap().iter().cloned());
            match page["next_offset"].as_u64() { Some(next)=>{assert!(next>offset); offset=next;},None=>break }
        }
        assert_eq!(actual, *all["items"].as_array().unwrap(), "{section}");
        let beyond=call(&app,"workbook_inspect",json!({"input":"in.twb","section":section,"offset":usize::MAX,"limit":1})).unwrap();
        assert_eq!(beyond["items"], json!([])); assert!(beyond["next_offset"].is_null());
    }
}
#[test]
fn stale_or_future_suite_is_rejected_before_any_assertion_is_evaluated() {
    let (dir, app) = setup();
    for (version, hash, expected) in [(2,fs::sha256(GOOD.as_bytes()),"SCHEMA_VERSION"),(1,"different".into(),"STALE_BASE")] {
        let suite=json!({"schema_version":version,"input_sha256":hash,"assertions":[{"assert":"file_hash","input":"missing","sha256":"x"}]});
        std::fs::write(dir.path().join("suite.json"),serde_json::to_vec(&suite).unwrap()).unwrap();
        assert_eq!(call(&app,"workbook_test",json!({"input":"in.twb","suite":"suite.json"})).unwrap_err().code,expected);
    }
}
