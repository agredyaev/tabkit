//! Local behavior and persistence checks. Never connects to external services.
use crate::{app::App,config::{Config,Limits,Policy},fs,package::Package,
    scalar::Scalar,workbook::Workbook,wire};
use serde_json::{json,Value};
use std::{io::{Read,Write},path::Path};
use tokio_util::sync::CancellationToken;
const GOOD:&str=include_str!("../examples/synthetic.twb");
fn setup(source:&str)->(tempfile::TempDir,App){
    let dir=tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.twb"),source).unwrap();
    let app=App::new(Config{workspace:dir.path().canonicalize().unwrap(),limits:Limits::default(),
        policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None}).unwrap();
    (dir,app)
}
fn call(app:&App,name:&str,args:Value)->crate::error::Result<Value>{
    app.dispatch(name,args,&CancellationToken::new())
}
fn calc(id:u32,before:&str,after:&str)->Value{
    json!({"op":"set_calculation","field_id":id,"expected_formula":before,"formula":after})
}
fn plan(app:&App,source:&str,ops:Vec<Value>)->crate::error::Result<Value>{
    call(app,"workbook_plan",json!({"input":"in.twb","output":"plan.json","changes":{
        "schema_version":1,"input_sha256":fs::sha256(source.as_bytes()),"operations":ops}}))
}
fn unchanged(dir:&Path,source:&str){
    assert_eq!(std::fs::read(dir.join("in.twb")).unwrap(),source.as_bytes());
    assert!(!dir.join("plan.json").exists());assert!(!dir.join("out.twb").exists());
}
fn graph_book(second_has_extra:bool,outer:&str)->String{
    let column=|name:&str,formula:Option<&str>|match formula{
        Some(f)=>format!("<column name='[{name}]' datatype='real'><calculation class='tableau' formula='{f}'/></column>"),
        None=>format!("<column name='[{name}]' datatype='real'/>")};
    let base=column("Base",None);let extra=column("Extra",None);
    let mid=column("Mid",Some("[Base]"));let end=column("Outer",Some(outer));
    let second=if second_has_extra{extra.as_str()}else{""};
    format!("<workbook source-build='2025.1.0'><datasources><datasource name='d'>{base}{extra}{mid}{end}</datasource></datasources><worksheets><worksheet name='First'><datasource-dependencies datasource='d'>{base}{extra}{mid}{end}</datasource-dependencies></worksheet><worksheet name='Second'><datasource-dependencies datasource='d'>{base}{second}{mid}{end}</datasource-dependencies></worksheet></worksheets></workbook>")
}
#[test]
fn dependency_update_must_be_valid_in_every_using_sheet(){
    let source=graph_book(false,"[Mid] * 2");let (dir,app)=setup(&source);
    let e=plan(&app,&source,vec![calc(2,"[Base]","[Extra]")]).unwrap_err();
    assert_eq!(e.code,"DEPENDENCY_UPDATE_REQUIRED");assert!(e.message.contains("Second"));unchanged(dir.path(),&source);
    let source=graph_book(true,"[Mid] * 2");let (_dir,app)=setup(&source);
    let p=plan(&app,&source,vec![calc(2,"[Base]","[Extra]")]).unwrap();
    assert_eq!(p["patch_count"],3);
}
#[test]
fn individually_valid_edits_that_form_a_batch_cycle_are_rejected(){
    let source=graph_book(true,"[Base]");
    for op in [calc(2,"[Base]","[Outer]"),calc(3,"[Base]","[Mid]")]{
        let (_dir,app)=setup(&source);plan(&app,&source,vec![op]).unwrap();
    }
    let (dir,app)=setup(&source);
    let e=plan(&app,&source,vec![calc(2,"[Base]","[Outer]"),calc(3,"[Base]","[Mid]")]).unwrap_err();
    assert_eq!(e.code,"REFERENCE_CYCLE");unchanged(dir.path(),&source);
}
#[test]
fn large_review_is_compacted_without_losing_written_artifact_identity(){
    let (dir,mut app)=setup(GOOD);app.cfg.limits.result_bytes=8192;
    let formula=format!("[Profit] / [Sales]{}"," + 0".repeat(2500));
    let p=plan(&app,GOOD,vec![calc(3,"[Profit] / [Sales]",&formula)]).unwrap();
    assert_eq!(p["details_truncated"],true);assert_eq!(p["delta_count"],1);
    let bytes=std::fs::read(dir.path().join("plan.json")).unwrap();
    assert_eq!(p["plan_sha256"],fs::sha256(&bytes));
    let full:Value=wire::decode(&bytes,1<<20).unwrap();
    assert_eq!(full["delta"][0]["after"],formula);
    let result=call(&app,"workbook_apply",json!({"plan":"plan.json","expected_plan_sha256":p["plan_sha256"],"output":"out.twb"})).unwrap();
    let output=std::fs::read(dir.path().join("out.twb")).unwrap();
    assert_eq!(result["sha256"],fs::sha256(&output));
    assert_eq!(result["details_truncated"],true);
    assert_eq!(std::fs::read(dir.path().join("in.twb")).unwrap(),GOOD.as_bytes());
}
#[test]
fn oversized_plan_is_rejected_before_file_creation(){
    let (dir,mut app)=setup(GOOD);app.cfg.limits.plan_json_bytes=8192;
    let formula=format!("[Profit] / [Sales]{}"," + 0".repeat(2500));
    let e=plan(&app,GOOD,vec![calc(3,"[Profit] / [Sales]",&formula)]).unwrap_err();
    assert_eq!(e.code,"JSON_OUTPUT_LIMIT");unchanged(dir.path(),GOOD);
}
#[test]
fn xml_and_node_budgets_are_enforced_for_local_inspection(){
    let (_dir,mut app)=setup(GOOD);app.cfg.limits.xml_bytes=GOOD.len() as u64-1;
    assert_eq!(call(&app,"workbook_inspect",json!({"input":"in.twb"})).unwrap_err().code,"LIMIT");
    app.cfg.limits.xml_bytes=GOOD.len() as u64;app.cfg.limits.xml_nodes=3;
    assert_eq!(call(&app,"workbook_inspect",json!({"input":"in.twb"})).unwrap_err().code,"XML");
}
fn make_package(path:&Path,entries:&[(&str,&[u8])]){
    let mut zip=zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name,data) in entries{
        zip.start_file(*name,zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}
#[test]
fn extract_selects_exact_member_and_does_not_replace_existing_file(){
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("book.twbx");
    let payload:Vec<_>=(0..=255u8).cycle().take(8193).collect();
    make_package(&input,&[("book.twb",GOOD.as_bytes()),("Data/ventas-é.hyper",&payload),("Data/other.hyper",b"other")]);
    let original=std::fs::read(&input).unwrap();let limits=Limits::default();
    let (pkg,_)=Package::open(&input,&limits).unwrap();let out=dir.path().join("result.hyper");
    let sha=pkg.extract_hyper("Data/ventas-é.hyper",&out,&limits).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(),payload);assert_eq!(sha,fs::sha256(&payload));
    assert!(pkg.extract_hyper("Data/other.hyper",&out,&limits).is_err());
    assert_eq!(std::fs::read(&out).unwrap(),payload);assert_eq!(std::fs::read(&input).unwrap(),original);
}
#[test]
fn changed_package_resource_invalidates_candidate_before_commit(){
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("in.twbx");let limits=Limits::default();
    make_package(&input,&[("book.twb",GOOD.as_bytes()),("image.bin",b"first")]);
    let (pkg,xml)=Package::open(&input,&limits).unwrap();
    make_package(&input,&[("book.twb",GOOD.as_bytes()),("image.bin",b"other")]);
    let changed=std::fs::read(&input).unwrap();let output=dir.path().join("out.twbx");
    assert_eq!(pkg.write_candidate(&output,&xml,&limits).unwrap_err().code,"STALE_BASE");
    assert!(!output.exists());assert_eq!(std::fs::read(&input).unwrap(),changed);
}

// Added cases exercise actual workbook files and independently inspect the result.
fn apply_candidate(app:&App,p:&Value)->Value{
    call(app,"workbook_apply",json!({"plan":"plan.json",
        "expected_plan_sha256":p["plan_sha256"],"output":"out.twb"})).unwrap()
}
fn checked_book(source:&str)->Workbook{
    Workbook::parse(source.as_bytes().to_vec(),&Limits::default()).unwrap()
}
#[test]
fn independent_batch_order_produces_identical_candidate_bytes(){
    let source=graph_book(true,"[Base]");
    let first=calc(2,"[Base]","[Extra] + 1");
    let second=calc(3,"[Base]","[Base] * 3");
    let (a,app_a)=setup(&source);let (b,app_b)=setup(&source);
    let pa=plan(&app_a,&source,vec![first.clone(),second.clone()]).unwrap();
    let pb=plan(&app_b,&source,vec![second,first]).unwrap();
    let ra=apply_candidate(&app_a,&pa);let rb=apply_candidate(&app_b,&pb);
    assert_eq!(ra["sha256"],rb["sha256"]);
    assert_eq!(std::fs::read(a.path().join("out.twb")).unwrap(),std::fs::read(b.path().join("out.twb")).unwrap());
    assert_eq!(pa["delta"],pb["delta"]);
    assert_eq!(std::fs::read(a.path().join("in.twb")).unwrap(),source.as_bytes());
}
#[test]
fn formula_edit_preserves_unicode_comments_and_unknown_vendor_nodes(){
    let vendor="<user:note key='España &amp; 日本'>keep this text</user:note>";
    let source=GOOD.replace("<windows />",&format!("<windows />{vendor}"));
    let (dir,app)=setup(&source);
    let formula="IF [Sales] < 2 THEN 'España & 日本' ELSE 'O''Brien' END";
    let p=plan(&app,&source,vec![calc(3,"[Profit] / [Sales]",formula)]).unwrap();
    apply_candidate(&app,&p);
    let output=std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
    assert!(output.contains(vendor));
    assert!(output.contains("<!-- Synthetic contract example."));
    let doc=roxmltree::Document::parse(&output).unwrap();
    let copies:Vec<_>=doc.descendants().filter(|n|n.is_element() && n.attribute("name")==Some("[Calculation_Ratio]")).collect();
    assert_eq!(copies.len(),2);
    for n in copies{let c=n.children().find(|c|c.has_tag_name("calculation")).unwrap();assert_eq!(c.attribute("formula"),Some(formula));}
    assert_eq!(std::fs::read(dir.path().join("in.twb")).unwrap(),source.as_bytes());
}
#[test]
fn inconsistent_formula_copy_is_reported_and_never_silently_repaired(){
    let source=GOOD.replacen("formula='[Profit] / [Sales]'","formula='[Profit] / ([Sales] + 1)'",1);
    let (dir,app)=setup(&source);
    let report=call(&app,"workbook_validate",json!({"input":"in.twb"})).unwrap();
    assert_eq!(report["passed"],false);
    assert!(report["diagnostics"].as_array().unwrap().iter().any(|d|d["code"]=="INCONSISTENT_DEFINITION"));
    let error=plan(&app,&source,vec![calc(3,"[Profit] / ([Sales] + 1)","[Sales]")]).unwrap_err();
    assert_eq!(error.code,"INCONSISTENT_DEFINITION");unchanged(dir.path(),&source);
}
#[test]
fn date_filter_changes_calendar_bounds_without_changing_other_fields(){
    let source=GOOD.replace("name='[Sales]' datatype='real'","name='[Sales]' datatype='date'")
        .replace("<min>0</min><max>100000</max>","<min>#2024-01-01#</min><max>#2025-12-31#</max>");
    let (dir,app)=setup(&source);let b=checked_book(&source);
    let hash=crate::workbook::state_hash(&b.filter_state(crate::workbook::FilterId(1)).unwrap());
    let p=plan(&app,&source,vec![json!({"op":"set_filter_range","filter_id":1,"expected_state_hash":hash,
        "min":{"kind":"date","value":"2024-02-29"},"max":{"kind":"date","value":"2025-03-01"}})]).unwrap();
    apply_candidate(&app,&p);
    let output=std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
    let doc=roxmltree::Document::parse(&output).unwrap();
    let filter=doc.descendants().find(|n|n.has_tag_name("filter") && n.attribute("class")==Some("quantitative")).unwrap();
    assert_eq!(filter.children().find(|n|n.has_tag_name("min")).unwrap().text(),Some("#2024-02-29#"));
    assert_eq!(filter.children().find(|n|n.has_tag_name("max")).unwrap().text(),Some("#2025-03-01#"));
    assert_eq!(output.matches("formula='[Profit] / [Sales]'").count(),2);
    assert_eq!(p["patch_count"],2);
}
#[test]
fn invalid_calendar_bound_is_rejected_before_creating_plan(){
    let source=GOOD.replace("name='[Sales]' datatype='real'","name='[Sales]' datatype='date'")
        .replace("<min>0</min><max>100000</max>","<min>#2025-01-01#</min><max>#2025-12-31#</max>");
    let (dir,app)=setup(&source);let b=checked_book(&source);
    let hash=crate::workbook::state_hash(&b.filter_state(crate::workbook::FilterId(1)).unwrap());
    let e=plan(&app,&source,vec![json!({"op":"set_filter_range","filter_id":1,"expected_state_hash":hash,
        "min":{"kind":"date","value":"2025-02-29"},"max":{"kind":"date","value":"2025-03-01"}})]).unwrap_err();
    assert_eq!(e.code,"LITERAL");unchanged(dir.path(),&source);
}
#[test]
fn generated_parameter_edits_check_actual_saved_values_and_rejections(){
    for n in [-5,0,1,2,10,25,50,99,100,101,200]{
        let (dir,app)=setup(GOOD);let b=checked_book(GOOD);
        let (current,domain)=b.parameter_state(crate::workbook::FieldId(4)).unwrap();
        let hash=crate::workbook::state_hash(&json!({"current":current,"domain":domain}));
        let result=plan(&app,GOOD,vec![json!({"op":"set_parameter","field_id":4,"expected_state_hash":hash,
            "current":{"kind":"integer","value":n},"domain":null})]);
        if (1..=100).contains(&n){
            let p=result.unwrap();apply_candidate(&app,&p);
            let output=std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
            let doc=roxmltree::Document::parse(&output).unwrap();let expected=n.to_string();
            let nodes:Vec<_>=doc.descendants().filter(|n|n.attribute("name")==Some("[Parameter 1]")).collect();
            assert_eq!(nodes.len(),2);
            for node in nodes{
                assert_eq!(node.attribute("value"),Some(expected.as_str()));
                assert_eq!(node.children().find(|c|c.has_tag_name("calculation")).unwrap().attribute("formula"),Some(expected.as_str()));
                assert_eq!(node.children().find(|c|c.has_tag_name("range")).unwrap().attribute("min"),Some("1"));
            }
            assert_eq!(std::fs::read(dir.path().join("in.twb")).unwrap(),GOOD.as_bytes());
        }else{assert_eq!(result.unwrap_err().code,"DOMAIN");unchanged(dir.path(),GOOD);}
    }
}
#[test]
fn scalar_type_mismatch_is_not_coerced_to_a_working_parameter(){
    let (dir,app)=setup(GOOD);let b=checked_book(GOOD);let (c,d)=b.parameter_state(crate::workbook::FieldId(4)).unwrap();
    let e=plan(&app,GOOD,vec![json!({"op":"set_parameter","field_id":4,"expected_state_hash":crate::workbook::state_hash(&json!({"current":c,"domain":d})),
        "current":{"kind":"string","value":"20"},"domain":null})]).unwrap_err();
    assert_eq!(e.code,"TYPE_MISMATCH");unchanged(dir.path(),GOOD);
}
#[test]
fn mixed_package_roundtrip_preserves_each_resource_and_entry_order(){
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("mixed.twbx");
    let mut writer=zip::ZipWriter::new(std::fs::File::create(&input).unwrap());
    writer.add_directory("Data/",zip::write::SimpleFileOptions::default()).unwrap();
    let entries=[("book.twb",GOOD.as_bytes(),zip::CompressionMethod::Deflated),
        ("Data/stored.bin",b"binary\0data\xff".as_slice(),zip::CompressionMethod::Stored),
        ("Data/compressed.txt",b"repeat repeat repeat".as_slice(),zip::CompressionMethod::Deflated)];
    for (name,data,method) in entries{
        writer.start_file(name,zip::write::SimpleFileOptions::default().compression_method(method)).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap();let original=std::fs::read(&input).unwrap();let limits=Limits::default();
    let (pkg,_)=Package::open(&input,&limits).unwrap();
    let changed=GOOD.replace("x='0'","x='19'");
    let xml=crate::xml::Xml::parse(changed.as_bytes().to_vec(),&limits).unwrap();
    let output=dir.path().join("out.twbx");pkg.write_candidate(&output,&xml,&limits).unwrap();
    let mut out=zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    assert_eq!(out.len(),4);assert_eq!(out.by_index(0).unwrap().name(),"Data/");
    for (i,(name,data,method)) in entries.iter().enumerate(){
        let mut member=out.by_index(i+1).unwrap();assert_eq!(member.name(),*name);assert_eq!(member.compression(),*method);
        let mut bytes=Vec::new();member.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes,if i==0{changed.as_bytes()}else{*data});
    }
    assert_eq!(std::fs::read(&input).unwrap(),original);
}
#[test]
fn categorical_filter_supports_union_to_member_transition_and_preserves_order(){
    let (dir,app)=setup(GOOD);let mut source=GOOD.to_owned();
    for (round,values) in [vec!["日本","East & West","España"],vec!["West"]].into_iter().enumerate(){
        if round>0{std::fs::write(dir.path().join("in.twb"),&source).unwrap();std::fs::remove_file(dir.path().join("plan.json")).unwrap();std::fs::remove_file(dir.path().join("out.twb")).unwrap();}
        let b=checked_book(&source);let state=b.filter_state(crate::workbook::FilterId(0)).unwrap();
        let p=plan(&app,&source,vec![json!({"op":"set_filter_values","filter_id":0,
            "expected_state_hash":crate::workbook::state_hash(&state),"values":values})]).unwrap();
        apply_candidate(&app,&p);source=std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
        let doc=roxmltree::Document::parse(&source).unwrap();
        let filter=doc.descendants().find(|n|n.has_tag_name("filter") && n.attribute("class")==Some("categorical")).unwrap();
        let actual:Vec<_>=filter.descendants().filter(|n|n.attribute("function")==Some("member"))
            .map(|n|n.attribute("member").unwrap().to_owned()).collect();
        let expected:Vec<_>=values.iter().map(|s|format!("\"{s}\"")).collect();assert_eq!(actual,expected);
        let group=filter.children().find(|n|n.has_tag_name("groupfilter")).unwrap();
        assert_eq!(group.attribute("function"),Some(if values.len()==1{"member"}else{"union"}));
        assert!(source.contains("<min>0</min><max>100000</max>"));
    }
}
#[test]
fn supported_unicode_scalar_literals_roundtrip_without_normalization(){
    for value in ["", "España", "日本", "O'Brien", "A & B < C", "e\u{301}", "é"]{
        let scalar=Scalar::String(value.to_owned());
        let encoded=scalar.literal("string").unwrap();
        assert_eq!(Scalar::parse("string",&encoded).unwrap(),scalar);
    }
}
fn assertion_report(dir:&Path,app:&App,source:&str,checks:Vec<Value>)->Value{
    let suite=json!({"schema_version":1,"input_sha256":fs::sha256(source.as_bytes()),"assertions":checks});
    std::fs::write(dir.join("checks.json"),serde_json::to_vec(&suite).unwrap()).unwrap();
    call(app,"workbook_test",json!({"input":"in.twb","suite":"checks.json"})).unwrap()
}
#[test]
fn absent_protected_section_differs_from_present_empty_section(){
    let without=GOOD.replace("  <windows />\n","");assert_ne!(without,GOOD);
    let (dir,app)=setup(&without);std::fs::write(dir.path().join("base.twb"),&without).unwrap();
    let checks=vec![json!({"assert":"subtree_unchanged","baseline":"base.twb","section":"windows"})];
    assert_eq!(assertion_report(dir.path(),&app,&without,checks.clone())["passed"],true);
    std::fs::write(dir.path().join("base.twb"),GOOD).unwrap();
    let report=assertion_report(dir.path(),&app,&without,checks);
    assert_eq!(report["passed"],false);assert_eq!(report["assertions"][0]["error"]["code"],"ASSERT_SUBTREE");
}
#[test]
fn inventory_and_protected_subtrees_have_distinct_observable_contracts(){
    let source=GOOD.replace("formula='[Profit] / [Sales]'","formula='[Profit] / ([Sales] + 1)'");
    let (dir,app)=setup(&source);std::fs::write(dir.path().join("base.twb"),GOOD).unwrap();
    let report=assertion_report(dir.path(),&app,&source,vec![
        json!({"assert":"inventory_unchanged","baseline":"base.twb"}),
        json!({"assert":"subtree_unchanged","baseline":"base.twb","section":"datasources"}),
        json!({"assert":"subtree_unchanged","baseline":"base.twb","section":"dashboards"})]);
    assert_eq!(report["passed"],false);
    assert_eq!(report["assertions"][0]["status"],"passed");assert_eq!(report["assertions"][1]["error"]["code"],"ASSERT_SUBTREE");
    assert_eq!(report["assertions"][2]["status"],"passed");
}
#[test]
fn ambiguous_or_missing_workbook_member_is_not_selected_arbitrarily(){
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("in.twbx");
    for (entries,expected) in [
        (vec![("one.twb",GOOD.as_bytes()),("two.twb",GOOD.as_bytes())],"AMBIGUOUS_PACKAGE"),
        (vec![("notes.txt",b"no workbook".as_slice())],"FORMAT")]{
        make_package(&input,&entries);let original=std::fs::read(&input).unwrap();
        let error=Package::open(&input,&Limits::default()).err().unwrap();assert_eq!(error.code,expected);
        assert_eq!(std::fs::read(&input).unwrap(),original);
    }
}
#[test]
fn package_entry_and_expanded_size_limits_reject_without_output(){
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("in.twbx");
    make_package(&input,&[("book.twb",GOOD.as_bytes()),("notes.txt",b"notes")]);
    for limits in [Limits{zip_entries:1,..Limits::default()},Limits{zip_expanded_bytes:GOOD.len() as u64,..Limits::default()}]{
        assert_eq!(Package::open(&input,&limits).err().unwrap().code,"ZIP_LIMIT");
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),1);
}
#[test]
fn plain_twb_package_and_xml_share_the_exact_admitted_digest(){
    let (dir,_)=setup(GOOD);let limits=Limits::default();
    let (pkg,xml)=Package::open(&dir.path().join("in.twb"),&limits).unwrap();
    let expected=fs::sha256(GOOD.as_bytes());
    assert_eq!(pkg.sha256,expected);
    assert_eq!(pkg.twb_sha256,expected);
    assert_eq!(xml.sha256,expected);
}
#[test]
fn candidate_extension_must_match_original_package_kind(){
    let (dir,_)=setup(GOOD);let limits=Limits::default();
    let (pkg,xml)=Package::open(&dir.path().join("in.twb"),&limits).unwrap();
    let out=dir.path().join("wrong.twbx");
    assert_eq!(pkg.write_candidate(&out,&xml,&limits).unwrap_err().code,"FORMAT");assert!(!out.exists());
}
proptest::proptest! {
    #[test]
    fn generated_formatting_variants_match_independent_expected_output(
        divisor in 1u32..10000, double_quotes in proptest::bool::ANY,
        crlf in proptest::bool::ANY, bom in proptest::bool::ANY
    ) {
        let mut source=GOOD.to_owned();
        if double_quotes{source=source.replace('\'',"\"");}
        if crlf{source=source.replace('\n',"\r\n");}
        if bom{source.insert(0,'\u{FEFF}');}
        let quote=if double_quotes{'"'}else{'\''};
        let before=format!("formula={quote}[Profit] / [Sales]{quote}");
        let formula=format!("[Profit] / ([Sales] + {divisor})");
        let expected=source.replace(&before,&format!("formula={quote}{formula}{quote}"));
        proptest::prop_assert_eq!(source.matches(&before).count(),2);
        let (dir,app)=setup(&source);
        let p=plan(&app,&source,vec![calc(3,"[Profit] / [Sales]",&formula)]).unwrap();
        apply_candidate(&app,&p);
        let actual=std::fs::read_to_string(dir.path().join("out.twb")).unwrap();
        proptest::prop_assert_eq!(actual,expected);
        proptest::prop_assert_eq!(std::fs::read(dir.path().join("in.twb")).unwrap(),source.as_bytes());
    }
}
