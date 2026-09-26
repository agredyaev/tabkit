#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::{config::Limits,package::Package,workbook::Workbook,validation};
fuzz_target!(|data:&[u8]|{
    if data.len()>65536{return;}
    let tmp=tempfile::tempdir().unwrap();let input=tmp.path().join("input.twbx");std::fs::write(&input,data).unwrap();
    let limits=Limits{file_bytes:65536,xml_bytes:65536,xml_nodes:2000,zip_entries:32,zip_expanded_bytes:131072,..Default::default()};
    if let Ok((pkg,xml))=Package::open(&input,&limits){
        if let Ok(book)=Workbook::parse(xml.text.as_bytes().to_vec(),&limits){let _=validation::local(&book);}
        let out=tmp.path().join("copy.twbx");
        // Admission plus an unchanged candidate must support a no-op round trip.
        // Do not silently discard write failures on already accepted packages.
        pkg.write_candidate(&out,&xml,&limits).expect("admitted package must copy unchanged");
        assert_eq!(std::fs::read(out).unwrap(),data);
        assert_eq!(std::fs::read(input).unwrap(),data);
    }
});
