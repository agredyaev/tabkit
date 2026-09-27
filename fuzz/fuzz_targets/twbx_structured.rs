#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Read, Write};
use tabkit_fuzz::{config::{Config,Limits,Policy},edit::{self,ChangeSet,Operation},package::Package,patch,validation,workbook::{FieldId,Workbook}};
use zip::{ZipArchive,ZipWriter,write::SimpleFileOptions,CompressionMethod};
const GOOD:&str=include_str!("../../examples/synthetic.twb");
fuzz_target!(|data:&[u8]|{
    if data.len()>4096{return;}
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("input.twbx");
    let mut z=ZipWriter::new(std::fs::File::create(&input).unwrap());
    let method=if data.first().copied().unwrap_or(0)&1==0{CompressionMethod::Deflated}else{CompressionMethod::Stored};
    z.start_file("workbook.twb",SimpleFileOptions::default().compression_method(method)).unwrap();
    z.write_all(GOOD.as_bytes()).unwrap();
    let mut expected=Vec::new();
    for(i,chunk)in data.get(1..).unwrap_or_default().chunks(64).take(4).enumerate(){
        let name=format!("Data/item-{i}.bin");
        let m=if chunk.first().copied().unwrap_or(0)&1==0{CompressionMethod::Deflated}else{CompressionMethod::Stored};
        z.start_file(&name,SimpleFileOptions::default().compression_method(m)).unwrap();
        z.write_all(chunk).unwrap();expected.push((name,chunk.to_vec()));
    }
    z.finish().unwrap();
    // Capture before invoking product code; comparing two reads of the same path is not an oracle.
    let before=std::fs::read(&input).unwrap();
    let limits=Limits{file_bytes:1<<20,xml_bytes:1<<20,zip_entries:16,zip_expanded_bytes:2<<20,..Default::default()};
    let (pkg,xml)=Package::open(&input,&limits).expect("generated package is within the supported contract");
    let book=Workbook::from_xml(xml).unwrap();let hash=pkg.sha256.clone();
    let cfg=Config{workspace:dir.path().to_path_buf(),limits:limits.clone(),policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None};
    let formula=format!("[Profit] / ([Sales] + {})",data.get(2).copied().unwrap_or(1) as u16+1);
    let changes=ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:vec![Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:formula.clone()}]};
    let (plan,candidate)=edit::plan_product_owned("input.twbx",&hash,book,changes,&cfg)
        .expect("supported generated edit must work");
    let after=Workbook::parse(candidate.text().as_bytes().to_vec(),&limits).unwrap();
    assert_eq!(candidate.sha256(),after.xml.sha256);
    patch::verify_preservation(GOOD,candidate.text(),&plan.patches).unwrap();assert!(validation::local(&after).passed);
    let output=dir.path().join("out.twbx");pkg.write_planned_candidate(&output,&candidate,&limits).unwrap();
    // Independently read the result and compare every untouched payload to generated data.
    let mut output_zip=ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    assert_eq!(output_zip.len(),expected.len()+1);
    for(name,bytes)in expected{
        let mut actual=Vec::new();output_zip.by_name(&name).unwrap().read_to_end(&mut actual).unwrap();
        assert_eq!(actual,bytes);
    }
    let mut twb=String::new();output_zip.by_name("workbook.twb").unwrap().read_to_string(&mut twb).unwrap();
    assert_eq!(twb,candidate.text());
    let document=roxmltree::Document::parse(&twb).unwrap();let mut copies=0;
    for column in document.descendants().filter(|n|n.is_element()&&n.attribute("name")==Some("[Calculation_Ratio]")){
        let calculation=column.children().find(|n|n.is_element()&&n.tag_name().name()=="calculation").unwrap();
        assert_eq!(calculation.attribute("formula"),Some(formula.as_str()));copies+=1;
    }
    assert_eq!(copies,2);
    assert_eq!(std::fs::read(&input).unwrap(),before);
});
