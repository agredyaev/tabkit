//! Stateful workflow tests assert effects and rejected effects, not method presence.
use crate::{
    app::App,
    config::{Config, Limits, Policy},
    fs,
    patch::{self, Patch},
    wire,
    xml::Span,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
const GOOD: &str = include_str!("../examples/synthetic.twb");
fn setup(source: &str) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.twb"), source).unwrap();
    let cfg = Config {
        workspace: dir.path().canonicalize().unwrap(),
        limits: Limits::default(),
        policy: Policy {
            allow_unverified_formula_edits: true,
            ..Default::default()
        },
        tableau: None,
        hyper: None,
    };
    (dir, App::new(cfg).unwrap())
}
fn call(app: &App, name: &str, args: Value) -> crate::error::Result<Value> {
    app.dispatch(name, args, &CancellationToken::new())
}
fn calc() -> Value {
    json!({"op":"set_calculation","field_id":3,"expected_formula":"[Profit] / [Sales]","formula":"[Profit] / ([Sales] + 1)"})
}
fn plan(app: &App, source: &str, ops: Value) -> crate::error::Result<Value> {
    call(
        app,
        "workbook_plan",
        json!({"input":"in.twb","output":"plan.json","changes":{
        "schema_version":1,"input_sha256":fs::sha256(source.as_bytes()),"operations":ops}}),
    )
}
fn apply(app: &App, p: &Value) -> crate::error::Result<Value> {
    call(
        app,
        "workbook_apply",
        json!({
    "plan":"plan.json","expected_plan_sha256":p["plan_sha256"],"output":"out.twb"}),
    )
}
fn state(app: &App, section: &str, id: usize) -> Value {
    let r = call(
        app,
        "workbook_inspect",
        json!({"input":"in.twb","section":section}),
    )
    .unwrap();
    r["items"][id].clone()
}
#[test]
fn failed_batch_has_no_partial_file_or_source_mutation() {
    let (dir, app) = setup(GOOD);
    let e=plan(&app,GOOD,json!([calc(),{"op":"set_calculation","field_id":999999,"expected_formula":"x","formula":"1"}])).unwrap_err();
    assert!(!e.code.is_empty());
    assert!(!dir.path().join("plan.json").exists());
    assert!(!dir.path().join("out.twb").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("in.twb")).unwrap(),
        GOOD
    );
}
#[test]
fn duplicate_operations_do_not_silently_use_last_value() {
    let (dir, app) = setup(GOOD);
    assert_eq!(
        plan(&app, GOOD, json!([calc(), calc()])).unwrap_err().code,
        "CONFLICTING_OPERATIONS"
    );
    assert!(!dir.path().join("plan.json").exists());
}
#[test]
fn tampered_patches_are_recomputed_even_with_new_external_hash() {
    let (dir, app) = setup(GOOD);
    let mut p = plan(&app, GOOD, json!([calc()])).unwrap();
    let file = dir.path().join("plan.json");
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    raw["patches"][0]["replacement"] = json!("attacker-controlled XML");
    let bytes = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    p["plan_sha256"] = json!(fs::sha256(&bytes));
    assert_eq!(apply(&app, &p).unwrap_err().code, "PLAN_TAMPERED");
    assert!(!dir.path().join("out.twb").exists());
}
#[test]
fn apply_rejects_changed_source_and_preserves_user_edit() {
    let (dir, app) = setup(GOOD);
    let p = plan(&app, GOOD, json!([calc()])).unwrap();
    let changed = GOOD.replace("x='0'", "x='1'");
    std::fs::write(dir.path().join("in.twb"), &changed).unwrap();
    assert_eq!(apply(&app, &p).unwrap_err().code, "STALE_BASE");
    assert!(!dir.path().join("out.twb").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("in.twb")).unwrap(),
        changed
    );
}
#[test]
fn apply_rechecks_operator_policy_not_only_plan_hash() {
    let (dir, mut app) = setup(GOOD);
    let p = plan(&app, GOOD, json!([calc()])).unwrap();
    app.cfg.policy.allow_unverified_formula_edits = false;
    assert_eq!(apply(&app, &p).unwrap_err().code, "POLICY");
    assert!(!dir.path().join("out.twb").exists());
}
#[test]
fn apply_refuses_to_replace_existing_output() {
    let (dir, app) = setup(GOOD);
    let p = plan(&app, GOOD, json!([calc()])).unwrap();
    std::fs::write(dir.path().join("out.twb"), b"user data").unwrap();
    assert_eq!(apply(&app, &p).unwrap_err().code, "OUTPUT_EXISTS");
    assert_eq!(
        std::fs::read(dir.path().join("out.twb")).unwrap(),
        b"user data"
    );
}
#[test]
fn cancellation_before_plan_does_not_write_artifact() {
    let (dir, app) = setup(GOOD);
    let ct = CancellationToken::new();
    ct.cancel();
    assert_eq!(
        app.dispatch("workbook_plan", json!({}), &ct)
            .unwrap_err()
            .code,
        "CANCELLED"
    );
    assert!(!dir.path().join("plan.json").exists());
}
#[test]
fn changes_cannot_request_raw_xml_or_unknown_operation() {
    for op in [
        json!({"op":"edit_xml","xml":"<workbook/>"}),
        json!({"op":"set_calculation","field_id":3,"expected_formula":"x","formula":"1","xpath":"//*"}),
    ] {
        let (dir, app) = setup(GOOD);
        assert_eq!(plan(&app, GOOD, json!([op])).unwrap_err().code, "ARGUMENT");
        assert!(!dir.path().join("plan.json").exists());
    }
}
#[test]
fn unsupported_filter_does_not_fall_back_to_raw_xml() {
    let source = GOOD.replace(
        "user:ui-enumeration='inclusive'",
        "user:ui-enumeration='exclusive'",
    );
    let (dir, app) = setup(&source);
    let b =
        crate::workbook::Workbook::parse(source.as_bytes().to_vec(), &Limits::default()).unwrap();
    assert!(b.filter_state(crate::workbook::FilterId(0)).is_err());
    let e = plan(
        &app,
        &source,
        json!([{"op":"set_filter_values","filter_id":0,"expected_state_hash":"ignored","values":["East"]}]),
    );
    assert!(e.is_err());
    assert!(!dir.path().join("plan.json").exists());
}
#[test]
fn parameter_value_and_domain_change_together_or_not_at_all() {
    let (dir, app) = setup(GOOD);
    let s = state(&app, "parameters", 0);
    let hash = s["parameter"]["state_hash"].clone();
    let base = json!({"op":"set_parameter","field_id":4,"expected_state_hash":hash,"current":null,
        "domain":{"kind":"range","min":{"kind":"integer","value":50},"max":{"kind":"integer","value":100},"step":{"kind":"integer","value":1}}});
    assert_eq!(
        plan(&app, GOOD, json!([base.clone()])).unwrap_err().code,
        "DOMAIN"
    );
    assert!(!dir.path().join("plan.json").exists());
    let mut valid = base;
    valid["current"] = json!({"kind":"integer","value":80});
    let p = plan(&app, GOOD, json!([valid])).unwrap();
    apply(&app, &p).unwrap();
    let after = std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
    assert_eq!(after.matches("value='80'").count(), 2);
    assert_eq!(after.matches("min='50'").count(), 2);
}
#[test]
fn list_domain_reuses_aliases_and_updates_all_parameter_copies() {
    let source=GOOD.replace("param-domain-type='range'","param-domain-type='list'")
        .replace("<range min='1' max='100' granularity='1' />","<members><member value='10' alias='Ten'/><member value='20' alias='Twenty'/></members>");
    let (dir, app) = setup(&source);
    let s = state(&app, "parameters", 0);
    let p=plan(&app,&source,json!([{"op":"set_parameter","field_id":4,"expected_state_hash":s["parameter"]["state_hash"],
        "current":{"kind":"integer","value":20},"domain":{"kind":"list","values":[{"kind":"integer","value":20},{"kind":"integer","value":30}]}}])).unwrap();
    apply(&app, &p).unwrap();
    let after = std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
    assert_eq!(after.matches("alias='Twenty'").count(), 2);
    assert!(!after.contains("alias='Ten'"));
    assert_eq!(after.matches("value='20'").count(), 4);
    assert!(
        crate::validation::local(
            &crate::workbook::Workbook::parse(after.into_bytes(), &Limits::default()).unwrap()
        )
        .passed
    );
}
#[test]
fn changed_property_fails_its_own_assertion_not_just_validation() {
    let (dir, app) = setup(GOOD);
    let p = plan(&app, GOOD, json!([calc()])).unwrap();
    let output = apply(&app, &p).unwrap();
    let suite = json!({"schema_version":1,"input_sha256":output["sha256"],"assertions":[
        {"assert":"calculation","field_id":3,"formula":"[Profit] / [Sales]"},{"assert":"no_diagnostics"}]});
    std::fs::write(
        dir.path().join("suite.json"),
        serde_json::to_vec(&suite).unwrap(),
    )
    .unwrap();
    let r = call(
        &app,
        "workbook_test",
        json!({"input":"out.twb","suite":"suite.json"}),
    )
    .unwrap();
    assert_eq!(r["passed"], false);
    assert_eq!(r["assertions"][0]["status"], "failed");
    assert_eq!(r["assertions"][1]["status"], "passed");
}
#[test]
fn corrupt_plan_hash_and_engine_version_are_distinct_failures() {
    let (dir, app) = setup(GOOD);
    let mut p = plan(&app, GOOD, json!([calc()])).unwrap();
    p["plan_sha256"] = json!("0".repeat(64));
    assert_eq!(apply(&app, &p).unwrap_err().code, "STALE_PLAN");
    let file = dir.path().join("plan.json");
    let mut v: Value = wire::decode(&std::fs::read(&file).unwrap(), 1 << 20).unwrap();
    v["engine_version"] = json!("unknown");
    let bytes = serde_json::to_vec(&v).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    p["plan_sha256"] = json!(fs::sha256(&bytes));
    assert_eq!(apply(&app, &p).unwrap_err().code, "PLAN_VERSION");
}
#[test]
fn preservation_verifier_rejects_reversed_or_out_of_bounds_ranges() {
    let p = Patch {
        span: Span { start: 1, end: 0 },
        expected: String::new(),
        replacement: String::new(),
        reason: String::new(),
    };
    // This invalid range could otherwise make an extra copy of 'a' look preserved.
    assert!(patch::verify_preservation("a", "aa", &[p]).is_err());
    let p = Patch {
        span: Span { start: 8, end: 12 },
        expected: String::new(),
        replacement: String::new(),
        reason: String::new(),
    };
    assert!(patch::verify_preservation("a", "a", &[p]).is_err());
}

#[test]
fn fuzz_regression_json_number_roundtrip_is_stable() {
    let input = include_bytes!("../tests/fuzz_regressions/json-number-roundtrip.json");
    let value: Value = wire::decode(input, 1024).unwrap();
    let encoded = wire::encode(&value, 1024, false).unwrap();
    assert_eq!(wire::decode::<Value>(&encoded, 1024).unwrap(), value);
}

#[test]
fn mcp_frames_survive_fragmented_reads_and_do_not_leak_invalid_frames() {
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
    struct Chunks<'a> {
        bytes: &'a [u8],
        chunk: usize,
        pending: bool,
    }
    impl AsyncRead for Chunks<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            out: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.pending {
                self.pending = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let n = self.chunk.min(self.bytes.len()).min(out.remaining());
            out.put_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            self.pending = true;
            Poll::Ready(Ok(()))
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let first = b"{\"id\":1,\"text\":\"\\u20ac\"}\n";
        for size in 1..=32 {
            let mut full = first.to_vec();
            full.extend_from_slice(b"{\"id\":2}\n");
            let mut stream = wire::JsonLines::new(
                Chunks {
                    bytes: &full,
                    chunk: size,
                    pending: true,
                },
                1024,
            );
            let mut out = Vec::new();
            stream.read_to_end(&mut out).await.unwrap();
            assert_eq!(out, full);
            let mut bad = first.to_vec();
            bad.extend_from_slice(b"{\"id\":2,\"id\":3}\n");
            let mut stream = wire::JsonLines::new(
                Chunks {
                    bytes: &bad,
                    chunk: size,
                    pending: true,
                },
                1024,
            );
            let mut out = Vec::new();
            assert!(stream.read_to_end(&mut out).await.is_err());
            assert_eq!(out, first);
        }
    });
}

#[cfg(unix)]
#[test]
fn workspace_rejects_real_symlinks_and_path_traversal() {
    use std::os::unix::fs::symlink;
    let (dir, app) = setup(GOOD);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"not user data").unwrap();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    for path in [
        "../secret.txt",
        "link/secret.txt",
        "/etc/passwd",
        ".tabkit/state",
        "a/../in.twb",
    ] {
        assert!(
            call(&app, "file_hash", json!({"input":path})).is_err(),
            "{path}"
        );
    }
    assert_eq!(
        std::fs::read(outside.path().join("secret.txt")).unwrap(),
        b"not user data"
    );
}

#[test]
fn cyclic_calculation_is_rejected_before_artifact_creation() {
    let (dir, app) = setup(GOOD);
    let mut op = calc();
    op["formula"] = json!("[Calculation_Ratio] + 1");
    assert_eq!(
        plan(&app, GOOD, json!([op])).unwrap_err().code,
        "REFERENCE_CYCLE"
    );
    assert!(!dir.path().join("plan.json").exists());
}
#[test]
fn reversed_filter_range_is_not_committed() {
    let (dir, app) = setup(GOOD);
    let s = state(&app, "filters", 1);
    let e = plan(
        &app,
        GOOD,
        json!([{"op":"set_filter_range","filter_id":1,"expected_state_hash":s["state_hash"],
        "min":{"kind":"real","value":"100"},"max":{"kind":"real","value":"1"}}]),
    )
    .unwrap_err();
    assert_eq!(e.code, "DOMAIN");
    assert!(!dir.path().join("plan.json").exists());
}
#[test]
fn datetime_values_validate_calendar_and_clock_not_just_string_length() {
    use crate::scalar::{Domain, Scalar};
    let good = Scalar::DateTime("2024-02-29 23:59:59".into());
    assert_eq!(good.literal("datetime").unwrap(), "#2024-02-29 23:59:59#");
    for bad in [
        "2025-02-29 12:00:00",
        "2024-02-29 24:01:00",
        "2024-02-29 23:60:00",
        "2024-02-29 23:59:61",
        "2024-02-29T23:59:59",
    ] {
        assert!(
            Scalar::DateTime(bad.into()).literal("datetime").is_err(),
            "{bad}"
        );
    }
    let domain = Domain::Range {
        min: Scalar::DateTime("2024-02-29 00:00:00".into()),
        max: good.clone(),
        step: None,
    };
    assert!(domain.accepts(&good, "datetime").is_ok());
    assert!(
        domain
            .accepts(&Scalar::DateTime("2024-03-01 00:00:00".into()), "datetime")
            .is_err()
    );
}

#[test]
fn fuzz_regression_nested_sql_rejects_without_speculative_explosion() {
    let sql = include_str!("../tests/fuzz_regressions/sql-nested-function-resource.sql");
    let start = std::time::Instant::now();
    assert!(crate::sql::admit(sql, 10).is_err());
    // The same 112-byte input exceeded the 3s libFuzzer limit before the fix.
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn nested_supported_sql_functions_keep_their_existing_semantics() {
    let mut expression = "\"Sales\"".to_owned();
    for _ in 0..16 { expression = format!("ABS({expression})"); }
    let query = format!("SELECT {expression} FROM \"Extract\".\"Extract\"");
    let start = std::time::Instant::now();
    let admitted = crate::sql::admit(&query, 10).unwrap();
    assert_eq!(admitted.tables, vec![("Extract".into(), "Extract".into())]);
    assert!(admitted.sql.contains(&expression));
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn sql_line_comments_do_not_turn_appended_text_into_an_executed_statement() {
    let read = "SELECT * FROM \"Extract\".\"Extract\" -- operator note";
    let commented = format!("{read}; DELETE FROM \"Extract\".\"Extract\"");
    let admitted = crate::sql::admit(&commented, 10).unwrap();
    assert!(!admitted.sql.contains("DELETE"));
    let actual_second_statement = format!("{read}\n; DELETE FROM \"Extract\".\"Extract\"");
    assert!(crate::sql::admit(&actual_second_statement, 10).is_err());
}
