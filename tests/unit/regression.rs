//! Focused regressions added with the audit repair. Execute with cargo test.
//! Fixtures are synthetic, never evidence of Tableau 2025 runtime compatibility.
use crate::{app::App,config::{Config,Limits,Policy,TableauConfig,TableauAuth,OAuthConfig},edit::{self,ChangeSet,Operation},
    fs,patch,validation::{self,Status},wire,workbook::{Workbook,FieldId},xml::{Xml,NodeId,Span}};
use serde_json::json;
use tokio_util::sync::CancellationToken;
const GOOD:&str=include_str!("../../examples/synthetic.twb");
const NEW_REF:&str=include_str!("../fixtures/new-reference.before.twb");
const INVALID_PARAM:&str=include_str!("../fixtures/invalid-parameter.twb");
const BAD_REFERENCE:&str=include_str!("../fixtures/known-local-error.twb");
const INLINE:&str=include_str!("../fixtures/inline-calculation.twb");
fn cfg(path:&std::path::Path)->Config {
    Config{workspace:path.canonicalize().unwrap(),limits:Limits::default(),
        policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None}
}
fn book(s:&str)->Workbook{Workbook::parse(s.as_bytes().to_vec(),&Limits::default()).unwrap()}
fn changes(s:&str,formula:&str)->ChangeSet {
    ChangeSet{schema_version:1,input_sha256:fs::sha256(s.as_bytes()),operations:vec![Operation::SetCalculation{
        field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.into()}]}
}
#[test]
fn invalid_parameter_is_not_unsupported_success(){
    let v=validation::local(&book(INVALID_PARAM));
    assert!(!v.passed); assert_eq!(v.status,Status::Invalid);
    assert!(v.diagnostics.iter().any(|d|d.code=="DOMAIN"));
    assert!(!v.unsupported_objects.iter().any(|d|d.code=="DOMAIN"));
}
#[test]
fn independent_known_reference_error_blocks_validation(){
    let v=validation::local(&book(BAD_REFERENCE));
    assert!(!v.passed); assert!(v.diagnostics.iter().any(|d|d.code=="UNRESOLVED_REFERENCE"));
}
#[test]
fn new_reference_missing_from_sheet_is_refused(){
    let temp=tempfile::tempdir().unwrap();
    let r=edit::plan("input.twb",&fs::sha256(NEW_REF.as_bytes()),&book(NEW_REF),changes(NEW_REF,"[Discount] / [Sales]"),&cfg(temp.path()));
    let e=r.err().expect("missing worksheet dependency must be rejected");
    assert_eq!(e.code,"DEPENDENCY_UPDATE_REQUIRED");
}
#[test]
fn supported_edit_keeps_unrelated_bytes_and_updates_copies(){
    let temp=tempfile::tempdir().unwrap();
    let formula="IF [Sales] = 0 THEN 0 ELSE [Profit] / [Sales] END";
    let (plan,after)=edit::plan("input.twb",&fs::sha256(GOOD.as_bytes()),&book(GOOD),changes(GOOD,formula),&cfg(temp.path())).unwrap();
    assert_eq!(plan.patches.len(),2);
    assert_eq!(after.xml.text.matches(formula).count(),2);
    patch::verify_preservation(GOOD,&after.xml.text,&plan.patches).unwrap();
    assert!(validation::local(&after).passed);
}
#[test]
fn identical_inputs_produce_identical_plans(){
    let temp=tempfile::tempdir().unwrap(); let c=cfg(temp.path()); let b=book(GOOD);
    let run=||edit::plan("input.twb",&fs::sha256(GOOD.as_bytes()),&b,changes(GOOD,"[Profit] / ([Sales] + 1)"),&c).unwrap();
    let(a,aa)=run();let(z,zz)=run(); assert_eq!(a,z);assert_eq!(aa.xml.text,zz.xml.text);
}
#[test]
fn no_op_has_no_patches(){
    let temp=tempfile::tempdir().unwrap();
    let (p,a)=edit::plan("input.twb",&fs::sha256(GOOD.as_bytes()),&book(GOOD),changes(GOOD,"[Profit] / [Sales]"),&cfg(temp.path())).unwrap();
    assert!(p.patches.is_empty());assert!(p.delta.is_empty());assert_eq!(a.xml.text,GOOD);
}
#[test]
fn stale_input_hash_is_rejected(){
    let temp=tempfile::tempdir().unwrap();
    let e=edit::plan("input.twb","changed",&book(GOOD),changes(GOOD,"[Sales]"),&cfg(temp.path())).err().unwrap();
    assert_eq!(e.code,"STALE_BASE");
}
#[test]
fn local_calculation_remains_visible_and_explicitly_unsupported(){
    let b=book(INLINE);assert!(!b.local_definitions.is_empty());
    let v=validation::local(&b);
    assert!(v.unsupported_objects.iter().any(|d|d.code=="UNSUPPORTED_LOCAL_DEFINITION"));
}
#[test]
fn lexical_admission_rejects_decoded_duplicate_keys(){
    for bytes in [br#"{"input":"a","input":"b"}"#.as_slice(),br#"{"field_id":1,"\u0066ield_id":2}"#.as_slice(),br#"{"nested":{"a":1,"a":2}}"#.as_slice()] {
        assert_eq!(wire::validate(bytes,1024).unwrap_err().code,"JSON_WIRE");
    }
    wire::validate(br#"{"input":"a","nested":[null,1,true,"x"]}"#,1024).unwrap();
}
#[test]
fn oversized_json_and_output_are_bounded(){
    assert_eq!(wire::validate(br#"{"a":"long"}"#,3).unwrap_err().code,"INPUT_LIMIT");
    assert!(wire::encode(&json!({"x":"a".repeat(100)}),16,false).is_err());
}
#[test]
fn xml_leaf_text_uses_original_namespace_context(){
    let xml=Xml::parse(b"<workbook xmlns:u='urn:test'><u:v>A&amp;B</u:v></workbook>".to_vec(),&Limits::default()).unwrap();
    assert_eq!(xml.text_content(NodeId(1)).unwrap(),"A&B");
}
#[test]
fn admitted_patch_emission_matches_checked_apply_and_hashes_candidate_once() {
    let source="abc<d x='1'/>xyz";
    let patch=patch::Patch{span:Span{start:9,end:10},expected:"1".into(),replacement:"2".into(),reason:"test".into()};
    let hash=fs::sha256(source.as_bytes());
    let checked=patch::apply(source,&hash,std::slice::from_ref(&patch),1024).unwrap();
    let (admitted,candidate_hash)=patch::apply_admitted(source,std::slice::from_ref(&patch),1024).unwrap();
    assert_eq!(checked,admitted);
    assert_eq!(candidate_hash,fs::sha256(admitted.as_bytes()));
}
#[test]
fn patch_budget_checked_before_candidate_allocation(){
    let p=patch::Patch{span:Span{start:1,end:2},expected:"b".into(),replacement:"0123456789".into(),reason:"test".into()};
    assert_eq!(patch::apply("abc",&fs::sha256(b"abc"),&[p],4).unwrap_err().code,"LIMIT");
}
#[test]
fn publish_errors_precede_transport_and_approval_creation(){
    for source in [INVALID_PARAM,BAD_REFERENCE] {
        let temp=tempfile::tempdir().unwrap();std::fs::write(temp.path().join("input.twb"),source).unwrap();
        let mut c=cfg(temp.path());c.policy.publish_enabled=true;
        let project="00000000-0000-0000-0000-000000000002";
        // Invalid endpoint deliberately ensures a regressed early transport call cannot contact a server.
        c.tableau=Some(TableauConfig{server:"not-a-url".into(),site:String::new(),api_version:"3.25".into(),
            auth:TableauAuth::OAuth(OAuthConfig{issuer:"https://issuer.invalid".into(),client_id:"test-client".into(),redirect_uri:"http://127.0.0.1:8765/callback".into(),scopes:vec!["tableau:content:read".into()]}),
            ca_certificate:None,publish_projects:vec![project.into()]});
        let app=App::new(c).unwrap();
        let e=app.dispatch("tableau_prepare_publish",json!({"input":"input.twb","expected_sha256":fs::sha256(source.as_bytes()),
            "name":"Test","project_id":project,"overwrite_workbook_id":null,"acknowledge_tableau_not_run":true,
            "acknowledge_unsupported_objects":true}),&CancellationToken::new()).unwrap_err();
        assert_eq!(e.code,"LOCAL_VALIDATION_FAILED");
        assert_eq!(std::fs::read_dir(temp.path().join(".tabkit")).unwrap().count(),0);
    }
}
#[test]
fn empty_assertion_suite_is_not_success(){
    let temp=tempfile::tempdir().unwrap();let c=cfg(temp.path());let ws=fs::Workspace::new(temp.path().canonicalize().unwrap()).unwrap();
    let path=temp.path().join("input.twb");std::fs::write(&path,GOOD).unwrap();
    let(p,x)=crate::package::Package::open(&path,&c.limits).unwrap();let b=Workbook::from_xml(x).unwrap();
    let suite=crate::assertions::Suite{schema_version:1,input_sha256:p.sha256.clone(),assertions:vec![]};
    assert!(crate::assertions::run(&c,&ws,&p,&b,&suite,&CancellationToken::new()).is_err());
}
#[test]
fn layout_change_is_detected_by_subtree_not_inventory(){
    let temp=tempfile::tempdir().unwrap();let c=cfg(temp.path());let ws=fs::Workspace::new(temp.path().canonicalize().unwrap()).unwrap();
    std::fs::write(temp.path().join("baseline.twb"),GOOD).unwrap();
    let changed=GOOD.replace("x='0'","x='1'");let path=temp.path().join("candidate.twb");std::fs::write(&path,changed).unwrap();
    let(p,x)=crate::package::Package::open(&path,&c.limits).unwrap();let b=Workbook::from_xml(x).unwrap();
    let suite=crate::assertions::Suite{schema_version:1,input_sha256:p.sha256.clone(),assertions:vec![
        crate::assertions::Assertion::InventoryUnchanged{baseline:"baseline.twb".into()},
        crate::assertions::Assertion::SubtreeUnchanged{baseline:"baseline.twb".into(),section:crate::assertions::ProtectedSection::Dashboards}]};
    let report=crate::assertions::run(&c,&ws,&p,&b,&suite,&CancellationToken::new()).unwrap();
    assert!(!report.passed);assert_eq!(report.assertions[0]["status"],"passed");assert_eq!(report.assertions[1]["status"],"failed");
}
#[test]
fn mcp_gate_accepts_complete_lines_and_rejects_ambiguous_or_truncated_frames(){
    use tokio::io::AsyncReadExt;
    let rt=tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(async {
        let valid=b"{\"id\":1}\n{\"id\":2}\n";
        let mut gate=wire::JsonLines::new(valid.as_slice(),128);let mut out=Vec::new();
        gate.read_to_end(&mut out).await.unwrap();assert_eq!(out,valid);
        for bytes in [b"{\"id\":1,\"id\":2}\n".as_slice(),b"{\"id\":1}".as_slice()] {
            let mut gate=wire::JsonLines::new(bytes,128);let mut out=Vec::new();
            assert!(gate.read_to_end(&mut out).await.is_err());assert!(out.is_empty());
        }
        let mut gate=wire::JsonLines::new(valid.as_slice(),4);let mut out=Vec::new();
        assert!(gate.read_to_end(&mut out).await.is_err());assert!(out.is_empty());
    });
}

#[test]
fn cli_definition_and_minimal_profiles_are_valid() {
    use clap::{CommandFactory, Parser};
    crate::Cli::command().debug_assert();
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap();
    let offline = crate::Cli::try_parse_from(["tabkit", "--workspace", ws, "mcp"]).unwrap();
    assert!(Config::from_startup(offline.startup).unwrap().tableau.is_none());
    let pat = crate::Cli::try_parse_from(["tabkit", "--workspace", ws,
        "--tableau-server", "https://tableau.invalid", "--tableau-auth", "pat",
        "--tableau-pat-name", "dev", "--tableau-pat-secret-env", "TEST_PAT", "mcp"]).unwrap();
    assert!(matches!(Config::from_startup(pat.startup).unwrap().tableau.unwrap().auth,
        TableauAuth::Pat(_)));
    let oauth = crate::Cli::try_parse_from(["tabkit", "--workspace", ws,
        "--tableau-server", "https://tableau.invalid", "--oauth-issuer", "https://issuer.invalid",
        "--oauth-client-id", "public", "--oauth-scope", "openid,profile",
        "--oauth-scope", "tableau:content:read", "mcp"]).unwrap();
    match Config::from_startup(oauth.startup).unwrap().tableau.unwrap().auth {
        TableauAuth::OAuth(c) => assert_eq!(c.scopes, ["openid", "profile", "tableau:content:read"]),
        _ => panic!("OAuth mode was not preserved"),
    }
}
#[test]
fn cli_rejects_unknown_missing_and_mixed_auth_arguments() {
    use clap::Parser;
    assert!(crate::Cli::try_parse_from(["tabkit", "mcp"]).is_err());
    assert!(crate::Cli::try_parse_from(["tabkit", "--workspace", "/", "--made-up", "mcp"]).is_err());
    let tmp = tempfile::tempdir().unwrap();
    let a = crate::Cli::try_parse_from(["tabkit", "--workspace", tmp.path().to_str().unwrap(),
        "--tableau-server", "https://tableau.invalid", "--tableau-auth", "pat",
        "--tableau-pat-name", "dev", "--oauth-client-id", "not-allowed", "mcp"]).unwrap();
    assert!(Config::from_startup(a.startup).is_err());
}
#[test]
fn digest_hex_preserves_all_nibbles_and_sha_vectors() {
    assert_eq!(fs::digest_hex([0; 32]), "0".repeat(64));
    assert_eq!(fs::digest_hex([255; 32]), "f".repeat(64));
    assert_eq!(fs::digest_hex(std::array::from_fn(|i| i as u8)),
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    assert_eq!(fs::sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(fs::sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
fn packed(path: &std::path::Path, names: &[(&str, &[u8])]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data) in names {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}
#[test]
fn single_deflate_backend_repackages_with_unrelated_bytes_preserved() {
    use std::io::Read;
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("input.twbx");
    let data = b"Synthetic extract payload, not an actual Hyper database";
    packed(&input, &[("book.twb", GOOD.as_bytes()), ("Data/source.hyper", data), ("Images/a.bin", b"image bytes")]);
    let c = cfg(tmp.path());
    let (p, x) = crate::package::Package::open(&input, &c.limits).unwrap();
    let b = Workbook::from_xml(x).unwrap();
    let mut changes = changes(GOOD, "[Profit] / ([Sales] + 1)");
    changes.input_sha256 = p.sha256.clone();
    let (_, after) = edit::plan("input.twbx", &p.sha256, &b, changes, &c).unwrap();
    let output = tmp.path().join("output.twbx");
    p.write_candidate(&output, &after.xml, &c.limits).unwrap();
    let (new, _) = crate::package::Package::open(&output, &c.limits).unwrap();
    for i in 1..p.entries.len() {
        assert_eq!(p.entries[i].compressed_sha256, new.entries[i].compressed_sha256);
    }
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    let mut actual = Vec::new();
    archive.by_name("Data/source.hyper").unwrap().read_to_end(&mut actual).unwrap();
    assert_eq!(actual, data);
    assert_eq!(fs::hash_file(&input, c.limits.file_bytes).unwrap(), p.sha256);
    assert!(p.write_candidate(&output, &after.xml, &c.limits).is_err());
}
#[test]
fn no_op_twbx_is_byte_identical() {
    let tmp = tempfile::tempdir().unwrap(); let c = cfg(tmp.path());
    let input = tmp.path().join("input.twbx");
    packed(&input, &[("book.twb", GOOD.as_bytes()), ("asset.bin", b"keep me")]);
    let (p, x) = crate::package::Package::open(&input, &c.limits).unwrap();
    let output = tmp.path().join("output.twbx");
    assert_eq!(p.write_candidate(&output, &x, &c.limits).unwrap(), p.sha256);
    assert_eq!(std::fs::read(input).unwrap(), std::fs::read(output).unwrap());
}
#[test]
fn twbx_rejects_traversal_and_case_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    for (i, names) in [vec![("../book.twb", GOOD.as_bytes())],
        vec![("book.twb", GOOD.as_bytes()), ("BOOK.TWB", GOOD.as_bytes())]].into_iter().enumerate() {
        let input = tmp.path().join(format!("bad{i}.twbx")); packed(&input, &names);
        assert!(crate::package::Package::open(&input, &Limits::default()).is_err());
    }
}
#[test]
fn sql_admission_retains_control_aggregate_queries() {
    let q = crate::sql::admit("SELECT SUM(\"Profit\") / NULLIF(SUM(\"Sales\"), 0) FROM \"Extract\".\"Extract\" WHERE \"Region\" = 'West'", 100).unwrap();
    assert_eq!(q.tables, vec![("Extract".into(), "Extract".into())]);
    assert!(q.sql.ends_with("LIMIT 101"));
}
#[test]
fn sql_admission_rejects_side_effects_and_unreviewed_constructs() {
    for sql in ["DELETE FROM \"Extract\".\"Extract\"", "SELECT * INTO x FROM \"Extract\".\"Extract\"",
        "SELECT * FROM \"Extract\".\"Extract\"; DROP TABLE x", "SELECT pg_sleep(10) FROM \"Extract\".\"Extract\"",
        "WITH q AS (SELECT * FROM \"Extract\".\"Extract\") SELECT * FROM q",
        "SELECT * FROM \"pg_catalog\".\"pg_class\"", "SELECT * FROM Extract.Extract",
        "SELECT * FROM \"Extract\".\"Extract\" FOR UPDATE"] {
        assert!(crate::sql::admit(sql, 10).is_err(), "unexpected admission: {sql}");
    }
}
#[test]
fn sql_identifiers_are_quoted_not_executed_as_fragments() {
    assert_eq!(crate::sql::identifier("a\"b").unwrap(), "\"a\"\"b\"");
    assert!(crate::sql::identifier("a\nb").is_err());
    assert!(crate::sql::identifier("").is_err());
}
