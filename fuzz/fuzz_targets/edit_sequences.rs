#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::json;
use tabkit_fuzz::{config::{Config,Limits,Policy},edit::{self,ChangeSet,Operation},fs,patch,
    scalar::Scalar,validation,workbook::{Workbook,FieldId,FilterId,state_hash}};
const GOOD:&str=include_str!("../../examples/synthetic.twb");
fuzz_target!(|data:&[u8]|{
    if data.len()>512{return;}
    let at=|i:usize|data.get(i).copied().unwrap_or(0);
    let source=if at(0)&1==0{GOOD.to_owned()}else{GOOD.replace('\n',"\r\n")};
    let cfg=Config{workspace:std::path::PathBuf::new(),limits:Limits::default(),
        policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None};
    let book=Workbook::parse(source.as_bytes().to_vec(),&cfg.limits).unwrap();
    let formula=format!("[Profit] / ([Sales] + {})",u16::from(at(1))+1);
    let n=i64::from(at(2)%100)+1;
    let min=Scalar::Real((-i64::from(at(3))).to_string());
    let max=Scalar::Real((i64::from(at(4))+1).to_string());
    let values=if at(5)&1==0{vec!["East".into(),"West".into()]}else{vec!["España".into()]};
    let (current,domain)=book.parameter_state(FieldId(4)).unwrap();
    let mut ops=vec![
        Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.clone()},
        Operation::SetParameter{field_id:FieldId(4),expected_state_hash:state_hash(&json!({"current":current,"domain":domain})),current:Some(Scalar::Integer(n)),domain:None},
        Operation::SetFilterValues{filter_id:FilterId(0),expected_state_hash:state_hash(&book.filter_state(FilterId(0)).unwrap()),values},
        Operation::SetFilterRange{filter_id:FilterId(1),expected_state_hash:state_hash(&book.filter_state(FilterId(1)).unwrap()),min,max},
    ];
    // Vary request order without varying the intended result.
    ops.rotate_left((at(6)%4) as usize);
    let input_hash=fs::sha256(source.as_bytes());
    let (plan,after)=edit::plan("in.twb",&input_hash,&book,ChangeSet{schema_version:1,input_sha256:input_hash.clone(),operations:ops.clone()},&cfg).unwrap();
    // Independent emission oracle: Vec byte replacement, not the product writer/verifier.
    let mut oracle=source.as_bytes().to_vec();let mut patches:Vec<_>=plan.patches.iter().collect();
    patches.sort_by_key(|p|p.span.start);
    for p in patches.iter().rev(){oracle.splice(p.span.range(),p.replacement.bytes());}
    assert_eq!(oracle,after.xml.text.as_bytes());
    patch::verify_preservation(&source,&after.xml.text,&plan.patches).unwrap();
    let doc=roxmltree::Document::parse(&after.xml.text).unwrap();
    let mut calculation_count=0;let mut parameter_count=0;let number=n.to_string();
    for column in doc.descendants().filter(|n|n.is_element()&&n.tag_name().name()=="column"){
        if column.attribute("name")==Some("[Calculation_Ratio]"){
            let child=column.children().find(|n|n.is_element()&&n.tag_name().name()=="calculation").unwrap();
            assert_eq!(child.attribute("formula"),Some(formula.as_str()));calculation_count+=1;
        }
        if column.attribute("name")==Some("[Parameter 1]"){
            assert_eq!(column.attribute("value"),Some(number.as_str()));parameter_count+=1;
        }
    }
    assert_eq!((calculation_count,parameter_count),(2,2));
    assert!(after.xml.text.contains("x='0' y='0' w='100' h='100'"));
    assert!(validation::local(&after).passed);
    for op in &mut ops{match op{
        Operation::SetCalculation{expected_formula,formula,..}=>*expected_formula=formula.clone(),
        Operation::SetParameter{expected_state_hash,..}=>{
            let(c,d)=after.parameter_state(FieldId(4)).unwrap();
            *expected_state_hash=state_hash(&json!({"current":c,"domain":d}));
        },
        Operation::SetFilterValues{filter_id,expected_state_hash,..}|Operation::SetFilterRange{filter_id,expected_state_hash,..}=>
            *expected_state_hash=state_hash(&after.filter_state(*filter_id).unwrap()),
    }}
    let (second,unchanged)=edit::plan("in.twb",&after.xml.sha256,&after,
        ChangeSet{schema_version:1,input_sha256:after.xml.sha256.clone(),operations:ops},&cfg).unwrap();
    assert!(second.patches.is_empty());assert!(second.delta.is_empty());
    assert_eq!(unchanged.xml.text,after.xml.text);
});
