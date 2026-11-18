#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;
use std::process::Command;
use tempfile::tempdir;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().expect("temp dir");
    let workbook = dir.path().join("fixture.twb");
    std::fs::write(
        &workbook,
        br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Sales">
      <table><view><datasource-dependencies datasource="Orders">
        <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      </datasource-dependencies></view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
  </worksheets>
</workbook>"#,
    )
    .expect("write fixture");
    (dir, workbook)
}

#[test]
fn cli_open_and_inspect_emit_json() {
    let (_dir, workbook) = fixture();

    let open = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("open")
        .arg(&workbook)
        .output()
        .expect("run tabkit open");
    assert!(
        open.status.success(),
        "{}",
        String::from_utf8_lossy(&open.stderr)
    );
    let summary: Value = serde_json::from_slice(&open.stdout).expect("parse open JSON");
    assert_eq!(summary["version"], "26.2");
    assert_eq!(summary["datasources"], 1);
    assert_eq!(summary["worksheets"], 1);

    let inspect = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("inspect")
        .arg(&workbook)
        .arg("--kind")
        .arg("field")
        .arg("--query")
        .arg("Sales")
        .output()
        .expect("run tabkit inspect");
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let result: Value = serde_json::from_slice(&inspect.stdout).expect("parse inspect JSON");
    assert_eq!(result["count"], 1);
}

#[test]
fn cli_reports_invalid_spec_and_inspect_kind() {
    let (dir, workbook) = fixture();
    let spec = dir.path().join("invalid.json");
    let output = dir.path().join("candidate.twb");
    std::fs::write(&spec, b"{not-json").expect("write invalid spec");

    let edit = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("edit")
        .arg(&workbook)
        .arg(&spec)
        .arg(&output)
        .output()
        .expect("run tabkit edit");
    assert!(!edit.status.success());
    let stderr = String::from_utf8_lossy(&edit.stderr);
    assert!(stderr.contains("SPEC_JSON:"), "{stderr}");
    assert!(!output.exists());

    let inspect = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("inspect")
        .arg(&workbook)
        .arg("--kind")
        .arg("invalid-kind")
        .output()
        .expect("run tabkit inspect");
    assert!(!inspect.status.success());
    let stderr = String::from_utf8_lossy(&inspect.stderr);
    assert!(stderr.contains("ARGUMENT:"), "{stderr}");
    assert!(stderr.contains("Unknown inspect kind"), "{stderr}");
}

#[test]
fn cli_edit_writes_candidate_and_rejects_output_clobber() {
    let (dir, workbook) = fixture();
    let spec = dir.path().join("web-dashboard.json");
    let output = dir.path().join("candidate.twb");
    std::fs::write(
        &spec,
        br#"{
          "edits": [{
            "op": "create_web_dashboard",
            "name": "CLI Web",
            "width": 1280,
            "height": 720,
            "extension_url": "http://127.0.0.1:8765/host.html",
            "html": "<main>CLI Web</main>",
            "css": "",
            "javascript": ""
          }]
        }"#,
    )
    .expect("write edit spec");

    let edit = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("edit")
        .arg(&workbook)
        .arg(&spec)
        .arg(&output)
        .output()
        .expect("run tabkit edit");
    assert!(
        edit.status.success(),
        "{}",
        String::from_utf8_lossy(&edit.stderr)
    );
    assert!(output.exists());
    let receipt: Value = serde_json::from_slice(&edit.stdout).expect("parse edit JSON");
    assert_eq!(receipt["output"], output.to_string_lossy().as_ref());
    assert_eq!(receipt["dashboards"][0], "CLI Web");

    let inspect = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("inspect")
        .arg(&output)
        .arg("--kind")
        .arg("dashboard")
        .arg("--query")
        .arg("CLI Web")
        .output()
        .expect("inspect candidate");
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspected: Value = serde_json::from_slice(&inspect.stdout).expect("parse inspect JSON");
    assert_eq!(inspected["count"], 1);

    let clobber = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("edit")
        .arg(&workbook)
        .arg(&spec)
        .arg(&output)
        .output()
        .expect("rerun tabkit edit");
    assert!(!clobber.status.success());
    let stderr = String::from_utf8_lossy(&clobber.stderr);
    assert!(stderr.contains("OUTPUT_EXISTS:"), "{stderr}");
}

#[test]
fn cli_lineage_traces_and_reports_not_found_without_crashing() {
    let (_dir, workbook) = fixture();

    let trace = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("lineage")
        .arg(&workbook)
        .arg("--from")
        .arg("Sales")
        .arg("--kind")
        .arg("field")
        .output()
        .expect("run lineage trace");
    assert!(
        trace.status.success(),
        "{}",
        String::from_utf8_lossy(&trace.stderr)
    );
    let trace_json: Value = serde_json::from_slice(&trace.stdout).expect("parse lineage JSON");
    let nodes = trace_json["nodes"].as_array().expect("trace nodes");
    assert!(
        nodes
            .iter()
            .any(|node| node["kind"] == "worksheet" && node["name"] == "Sales")
    );

    let missing = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("lineage")
        .arg(&workbook)
        .arg("--from")
        .arg("Missing")
        .output()
        .expect("run missing lineage query");
    assert!(missing.status.success());
    let missing_json: Value =
        serde_json::from_slice(&missing.stdout).expect("parse missing lineage JSON");
    assert_eq!(missing_json["error"], "not_found");
    assert_eq!(missing_json["query"], "Missing");

    let invalid_kind = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("lineage")
        .arg(&workbook)
        .arg("--from")
        .arg("Sales")
        .arg("--kind")
        .arg("bogus")
        .output()
        .expect("run invalid lineage kind");
    assert!(!invalid_kind.status.success());
    let stderr = String::from_utf8_lossy(&invalid_kind.stderr);
    assert!(stderr.contains("ARGUMENT:"), "{stderr}");
    assert!(stderr.contains("Unknown lineage kind"), "{stderr}");
}
