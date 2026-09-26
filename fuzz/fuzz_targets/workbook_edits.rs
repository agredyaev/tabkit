#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::{config::{Config,Limits,Policy},edit::{self,Operation,ChangeSet},fs,scalar::Scalar,workbook::{Workbook,FieldId,FilterId,state_hash},patch,validation};
use serde_json::json;
const GOOD:&str=include_str!("../../examples/synthetic.twb");
fuzz_target!(|data:&[u8]|{
    if data.is_empty()||data.len()>2048{return;}
    let cfg=Config{workspace:std::path::PathBuf::new(),limits:Limits::default(),policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None};
    let b=Workbook::parse(GOOD.as_bytes().to_vec(),&cfg.limits).unwrap();
    let mut ops=Vec::new();
    for (i,selector) in data.iter().take(4).enumerate() {
        let suffix=data.get(4+i..).unwrap_or_default();
        let value=String::from_utf8_lossy(suffix);
        let op=match selector%4 {
            0=>Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:value.into_owned()},
            1=>{let(c,d)=b.parameter_state(FieldId(4)).unwrap();Operation::SetParameter{field_id:FieldId(4),expected_state_hash:state_hash(&json!({"current":c,"domain":d})),current:Some(Scalar::Integer(*selector as i64)),domain:None}},
            2=>Operation::SetFilterValues{filter_id:FilterId(0),expected_state_hash:state_hash(&b.filter_state(FilterId(0)).unwrap()),values:value.split('|').take(32).map(str::to_owned).collect()},
            _=>Operation::SetFilterRange{filter_id:FilterId(1),expected_state_hash:state_hash(&b.filter_state(FilterId(1)).unwrap()),min:Scalar::Real("0".into()),max:Scalar::Real(value.into_owned())},
        };
        ops.push(op);
    }
    if ops.is_empty(){return;}
    let hash=fs::sha256(GOOD.as_bytes());let ch=ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:ops};
    if let Ok((p,after))=edit::plan("in.twb",&hash,&b,ch.clone(),&cfg){
        patch::verify_preservation(GOOD,&after.xml.text,&p.patches).unwrap();
        assert!(validation::local(&after).passed);
        let(p2,a2)=edit::plan("in.twb",&hash,&b,ch,&cfg).unwrap();assert_eq!(p,p2);assert_eq!(after.xml.text,a2.xml.text);
        assert!(after.xml.text.contains("x='0' y='0' w='100' h='100'"));
    }
});
