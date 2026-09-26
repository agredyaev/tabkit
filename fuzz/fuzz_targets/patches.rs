#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::{patch::{self,Patch},xml::Span,fs};
fuzz_target!(|data:&[u8]|{
    if data.len()<4||data.len()>4096{return;}
    let cut=4+(data.len()-4)/2;
    let input=String::from_utf8_lossy(&data[4..cut]);let replacement=String::from_utf8_lossy(&data[cut..]).into_owned();
    let a=u16::from_le_bytes([data[0],data[1]]) as usize%(input.len()+4);
    let b=u16::from_le_bytes([data[2],data[3]]) as usize%(input.len()+4);
    let valid=a<=b&&input.get(a..b).is_some();
    let p=Patch{span:Span{start:a as u32,end:b as u32},expected:input.get(a..b).unwrap_or("").into(),replacement,reason:String::new()};
    let result=patch::apply(&input,&fs::sha256(input.as_bytes()),&[p.clone()],65536);
    assert_eq!(result.is_ok(),valid);
    if let Ok(out)=result {
        let mut oracle=input.as_bytes().to_vec();oracle.splice(a..b,p.replacement.bytes());assert_eq!(out.as_bytes(),oracle);
        patch::verify_preservation(&input,&out,&[p.clone()]).unwrap();
        assert!(patch::verify_preservation(&input,&format!("!{out}"),&[p]).is_err());
    }else{
        assert!(patch::verify_preservation(&input,&format!("{input}{input}"),&[p]).is_err());
    }
});
