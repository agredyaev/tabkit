#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::json;
use tabkit_fuzz::{config::{Config,Limits,Policy},edit::{self,ChangeSet,Operation},fs,patch,
    scalar::Scalar,validation,workbook::{Workbook,FieldId,FilterId,state_hash}};
const GOOD:&str=include_str!("../../examples/synthetic.twb");
fuzz_target!(|data:&[u8]|{
    if data.len()>512{return;}
    let at=|i:usize|data.get(i).copied().unwrap_or(0);
    let mut source=GOOD.to_owned();
    if at(0)&1!=0{source=source.replace('\n',"\r\n");}
    if at(0)&2!=0{source=source.replace('\'',"\"");}
    if at(0)&4!=0{source.insert(0,'\u{FEFF}');}
    let cfg=Config{workspace:std::path::PathBuf::new(),limits:Limits::default(),
        policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None};
    let book=Workbook::parse(source.as_bytes().to_vec(),&cfg.limits).unwrap();
    let formula=format!("[Profit] / ([Sales] + {})",u16::from(at(1))+1);
    let n=i64::from(at(2)%100)+1;
    let low=(-i64::from(at(3))).to_string();let high=(i64::from(at(4))+1).to_string();
    let min=Scalar::Real(low.clone());let max=Scalar::Real(high.clone());
    let values:Vec<String>=if at(5)&1==0{vec!["East".into(),"West".into()]}else{vec!["España & 日本".into()]};
    let expected_members:Vec<_>=values.iter().map(|v|format!("\"{v}\"")).collect();
    let (current,domain)=book.parameter_state(FieldId(4)).unwrap();
    let mut ops=vec![
        Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.clone()},
        Operation::SetParameter{field_id:FieldId(4),expected_state_hash:state_hash(&json!({"current":current,"domain":domain})),current:Some(Scalar::Integer(n)),domain:None},
        Operation::SetFilterValues{filter_id:FilterId(0),expected_state_hash:state_hash(&book.filter_state(FilterId(0)).unwrap()),values},
        Operation::SetFilterRange{filter_id:FilterId(1),expected_state_hash:state_hash(&book.filter_state(FilterId(1)).unwrap()),min,max},
    ];
    ops.rotate_left((at(6)%4) as usize);
    let input_hash=fs::sha256(source.as_bytes());
    let (plan,after)=edit::plan_owned("in.twb",&input_hash,book,ChangeSet{schema_version:1,input_sha256:input_hash.clone(),operations:ops.clone()},&cfg).unwrap();
    // Independently assemble byte replacements; do not reuse the product writer.
    let mut oracle=source.as_bytes().to_vec();let mut patches:Vec<_>=plan.patches.iter().collect();
    patches.sort_by_key(|p|p.span.start);
    for p in patches.iter().rev(){oracle.splice(p.span.range(),p.replacement.bytes());}
    assert_eq!(oracle,after.xml.text.as_bytes());
    patch::verify_preservation(&source,&after.xml.text,&plan.patches).unwrap();
    let doc=roxmltree::Document::parse(&after.xml.text).unwrap();
    let mut calculations=0;let mut parameters=0;let number=n.to_string();
    for column in doc.descendants().filter(|n|n.has_tag_name("column")){
        if column.attribute("name")==Some("[Calculation_Ratio]"){
            let child=column.children().find(|n|n.has_tag_name("calculation")).unwrap();
            assert_eq!(child.attribute("formula"),Some(formula.as_str()));calculations+=1;
        }
        if column.attribute("name")==Some("[Parameter 1]"){
            assert_eq!(column.attribute("value"),Some(number.as_str()));parameters+=1;
            let child=column.children().find(|n|n.has_tag_name("calculation")).unwrap();
            assert_eq!(child.attribute("formula"),Some(number.as_str()));
        }
    }
    assert_eq!((calculations,parameters),(2,2));
    let categorical=doc.descendants().find(|n|n.has_tag_name("filter") && n.attribute("class")==Some("categorical")).unwrap();
    let members:Vec<_>=categorical.descendants().filter(|n|n.attribute("function")==Some("member"))
        .map(|n|n.attribute("member").unwrap().to_owned()).collect();
    assert_eq!(members,expected_members);
    let range=doc.descendants().find(|n|n.has_tag_name("filter") && n.attribute("class")==Some("quantitative")).unwrap();
    assert_eq!(range.children().find(|n|n.has_tag_name("min")).unwrap().text(),Some(low.as_str()));
    assert_eq!(range.children().find(|n|n.has_tag_name("max")).unwrap().text(),Some(high.as_str()));
    let start=source.find("<dashboards>").unwrap();
    let end=source.find("</dashboards>").unwrap()+"</dashboards>".len();
    assert!(after.xml.text.contains(&source[start..end]));
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
