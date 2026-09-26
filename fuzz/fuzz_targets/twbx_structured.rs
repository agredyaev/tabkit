#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Write;
use tabkit_fuzz::{config::{Config,Limits,Policy},edit::{self,ChangeSet,Operation},package::Package,patch,validation,workbook::{FieldId,Workbook}};
use zip::{ZipWriter,write::SimpleFileOptions,CompressionMethod};
const GOOD:&str=include_str!("../../examples/synthetic.twb");
fuzz_target!(|data:&[u8]|{
    if data.len()>4096{return;}
    let dir=tempfile::tempdir().unwrap();let input=dir.path().join("input.twbx");
    let file=std::fs::File::create(&input).unwrap();let mut z=ZipWriter::new(file);
    let method=if data.first().copied().unwrap_or(0)&1==0{CompressionMethod::Deflated}else{CompressionMethod::Stored};
    if z.start_file("workbook.twb",SimpleFileOptions::default().compression_method(method)).is_err(){return;}
    z.write_all(GOOD.as_bytes()).unwrap();
    const NAMES:&[&str]=&["Data/extract.hyper","assets/info.txt","../escape.txt","Data/../evil.txt","WORKBOOK.TWB","nested/data.bin"];
    for(i,chunk)in data.get(1..).unwrap_or_default().chunks(64).take(4).enumerate(){
        let name=NAMES[chunk.first().copied().unwrap_or(i as u8) as usize%NAMES.len()];
        let m=if chunk.get(1).copied().unwrap_or(0)&1==0{CompressionMethod::Deflated}else{CompressionMethod::Stored};
        if z.start_file(name,SimpleFileOptions::default().compression_method(m)).is_err(){return;}
        if z.write_all(chunk).is_err(){return;}
    }
    if z.finish().is_err(){return;}
    let limits=Limits{file_bytes:1<<20,xml_bytes:1<<20,zip_entries:16,zip_expanded_bytes:2<<20,..Default::default()};
    let Ok((pkg,xml))=Package::open(&input,&limits) else{return;};
    let book=Workbook::from_xml(xml).unwrap();let hash=pkg.sha256.clone();
    let cfg=Config{workspace:dir.path().to_path_buf(),limits:limits.clone(),policy:Policy{allow_unverified_formula_edits:true,..Default::default()},tableau:None,hyper:None};
    let formula=format!("[Profit] / ([Sales] + {})",data.get(2).copied().unwrap_or(1) as u16+1);
    let changes=ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:vec![Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula}]};
    let Ok((plan,after))=edit::plan("input.twbx",&hash,&book,changes,&cfg) else{return;};
    patch::verify_preservation(GOOD,&after.xml.text,&plan.patches).unwrap();assert!(validation::local(&after).passed);
    let output=dir.path().join("out.twbx");pkg.write_candidate(&output,&after.xml,&limits).unwrap();
    let (out_pkg,out_xml)=Package::open(&output,&limits).unwrap();assert_eq!(out_pkg.entries.len(),pkg.entries.len());
    assert_eq!(out_xml.text,after.xml.text);assert_eq!(std::fs::read(&input).unwrap(),std::fs::read(&pkg.path).unwrap());
});
