//! Equivalence checks for consumed ownership and range-based relationships.
use crate::{config::{Config, Limits, Policy}, edit::{self, ChangeSet, Operation}, fs,
    workbook::{Workbook, FieldId}};
use serde_json::json;
const GOOD: &str = include_str!("../examples/synthetic.twb");
fn cfg() -> Config { Config { workspace: Default::default(), limits: Limits::default(),
    policy: Policy { allow_unverified_formula_edits: true, ..Default::default() }, tableau: None, hyper: None } }
#[test]
fn consumed_and_borrowed_planners_have_identical_outputs_and_errors() {
    let config = cfg(); let hash = fs::sha256(GOOD.as_bytes());
    for formula in ["[Profit] / ([Sales] + 1)", "[Profit] / [Sales]", "[Missing]", "[Calculation_Ratio]"] {
        let make = || Workbook::parse(GOOD.as_bytes().to_vec(), &config.limits).unwrap();
        let changes = ChangeSet { schema_version: 1, input_sha256: hash.clone(), operations: vec![
            Operation::SetCalculation { field_id: FieldId(3), expected_formula: "[Profit] / [Sales]".into(), formula: formula.into() }] };
        let a = edit::plan("in.twb", &hash, &make(), changes.clone(), &config);
        let b = edit::plan_owned("in.twb", &hash, make(), changes, &config);
        match (a,b) {
            (Ok((p,x)), Ok((q,y))) => { assert_eq!(p,q); assert_eq!(x.xml.text,y.xml.text); },
            (Err(a),Err(b)) => { assert_eq!(a.code,b.code); assert_eq!(a.message,b.message); },
            _ => panic!("ownership changed the result"),
        }
    }
}
#[test]
fn indexed_field_reports_match_independent_full_relationship_scans() {
    let b = Workbook::parse(GOOD.as_bytes().to_vec(), &Limits::default()).unwrap();
    for i in 0..b.fields.len() {
        let id = FieldId(i as u32); let actual = b.field_report(i);
        let declared: std::collections::BTreeSet<_> = b.dependency_scopes.iter()
            .filter(|s|b.dependency_fields[s.fields.clone()].contains(&id))
            .filter_map(|s|s.worksheet.and_then(|w|b.xml.value(w,"name"))).collect();
        let uses: std::collections::BTreeSet<_> = b.known_uses.iter().filter(|u|u.field_id==id)
            .filter_map(|u|b.xml.value(u.worksheet_node,"name")).collect();
        let callers: Vec<_> = b.edges.iter().filter(|e|e.to==id).map(|e|e.from).collect();
        let filters: Vec<_> = b.filters.iter().filter(|f|f.field==Some(id)).map(|f|f.id).collect();
        assert_eq!(actual["declared_in_worksheets"],json!(declared));
        assert_eq!(actual["known_worksheet_uses"],json!(uses));
        assert_eq!(actual["referenced_by_calculations"],json!(callers));
        assert_eq!(actual["filters"],json!(filters));
    }
}
#[test]
fn field_lookup_is_scoped_to_datasource_and_keeps_duplicate_rejection() {
    use crate::workbook::DatasourceId;
    let source="<workbook><datasources><datasource name='a'><column name='[X]'/></datasource><datasource name='b'><column name='[X]'/></datasource></datasources></workbook>";
    let b=Workbook::parse(source.as_bytes().to_vec(),&Limits::default()).unwrap();
    let reference=crate::formula::analyze("[X]").unwrap().references.remove(0);
    assert_eq!(b.resolve(DatasourceId(0),&reference).unwrap(),FieldId(0));
    assert_eq!(b.resolve(DatasourceId(1),&reference).unwrap(),FieldId(1));
    let duplicate=source.replace("<column name='[X]'/>","<column name='[X]'/><column name='[X]'/>");
    assert_eq!(Workbook::parse(duplicate.into_bytes(),&Limits::default()).err().unwrap().code,"AMBIGUOUS_TARGET");
}

#[test]
fn admitted_formula_override_preflight_matches_candidate_graph_failures() {
    let config=cfg(); let hash=fs::sha256(GOOD.as_bytes());
    for (formula,code) in [
        ("[Calculation_Ratio]","REFERENCE_CYCLE"),
        ("[Missing]","UNRESOLVED_REFERENCE"),
    ] {
        let changes=ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:vec![
            Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.into()}]};
        let e=edit::plan_owned("in.twb",&hash,
            Workbook::parse(GOOD.as_bytes().to_vec(),&config.limits).unwrap(),changes,&config).err().unwrap();
        assert_eq!(e.code,code);
    }
}

fn representative_batch(book:&Workbook,hash:&str)->ChangeSet{
    let (current,domain)=book.parameter_state(crate::workbook::FieldId(4)).unwrap();
    ChangeSet{schema_version:1,input_sha256:hash.into(),operations:vec![
        Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),
            formula:"[Profit] / ([Sales] + 1)".into()},
        Operation::SetParameter{field_id:FieldId(4),
            expected_state_hash:crate::workbook::state_hash(&json!({"current":current,"domain":domain})),
            current:Some(crate::scalar::Scalar::Integer(7)),domain:None},
        Operation::SetFilterValues{filter_id:crate::workbook::FilterId(0),
            expected_state_hash:crate::workbook::state_hash(&book.filter_state(crate::workbook::FilterId(0)).unwrap()),
            values:vec!["East".into(),"West".into()]},
        Operation::SetFilterRange{filter_id:crate::workbook::FilterId(1),
            expected_state_hash:crate::workbook::state_hash(&book.filter_state(crate::workbook::FilterId(1)).unwrap()),
            min:crate::scalar::Scalar::Real("-2".into()),max:crate::scalar::Scalar::Real("77".into())},
    ]}
}
#[test]
fn product_candidate_matches_full_candidate_re_admission_oracle() {
    let config=cfg(); let hash=fs::sha256(GOOD.as_bytes());
    let source=Workbook::parse(GOOD.as_bytes().to_vec(),&config.limits).unwrap();
    let changes=representative_batch(&source,&hash);
    let (full,after)=edit::plan_owned("in.twb",&hash,
        Workbook::parse(GOOD.as_bytes().to_vec(),&config.limits).unwrap(),changes.clone(),&config).unwrap();
    let (fast,candidate)=edit::plan_product_owned("in.twb",&hash,source,changes,&config).unwrap();
    assert_eq!(fast,full);
    assert_eq!(candidate.text(),after.xml.text);
    assert_eq!(candidate.sha256(),after.xml.sha256);
    assert!(crate::validation::local(&after).passed);
}
#[test]
fn product_candidate_errors_match_full_oracle_for_graph_and_preconditions() {
    let config=cfg(); let hash=fs::sha256(GOOD.as_bytes());
    for formula in ["[Calculation_Ratio]","[Missing]"] {
        let changes=ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:vec![
            Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.into()}]};
        let full=edit::plan_owned("in.twb",&hash,
            Workbook::parse(GOOD.as_bytes().to_vec(),&config.limits).unwrap(),changes.clone(),&config).err().unwrap();
        let fast=edit::plan_product_owned("in.twb",&hash,
            Workbook::parse(GOOD.as_bytes().to_vec(),&config.limits).unwrap(),changes,&config).err().unwrap();
        assert_eq!((fast.code,fast.message),(full.code,full.message));
    }
}
