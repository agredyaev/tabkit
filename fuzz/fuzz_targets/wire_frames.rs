#![no_main]
use libfuzzer_sys::fuzz_target;
use std::{io,pin::Pin,task::{Context,Poll,Waker}};
use tokio::io::{AsyncRead,ReadBuf};
use tabkit_fuzz::wire::JsonLines;
struct Chunks<'a>{bytes:&'a[u8],size:usize,pending:bool}
impl AsyncRead for Chunks<'_>{
    fn poll_read(mut self:Pin<&mut Self>,cx:&mut Context<'_>,out:&mut ReadBuf<'_>)->Poll<io::Result<()>>{
        if self.pending{self.pending=false;cx.waker().wake_by_ref();return Poll::Pending;}
        let n=self.size.min(self.bytes.len()).min(out.remaining());
        out.put_slice(&self.bytes[..n]);self.bytes=&self.bytes[n..];self.pending=true;
        Poll::Ready(Ok(()))
    }
}
fuzz_target!(|data:&[u8]|{
    if data.len()>512{return;}
    let at=|i:usize|data.get(i).copied().unwrap_or(0);
    let text=String::from_utf8_lossy(data.get(3..).unwrap_or_default());
    let mut first=serde_json::to_vec(&serde_json::json!({"id":1,"text":text})).unwrap();first.push(b'\n');
    let mode=at(0)%4;
    let max=if mode==3{first.len().max(32)}else{4096};
    let suffix=match mode{
        0=>b"{\"id\":2}\n".to_vec(),
        1=>b"{\"id\":2,\"id\":3}\n".to_vec(),
        2=>b"{\"id\":2}".to_vec(),
        _=>{let mut v=serde_json::to_vec(&"x".repeat(max+1)).unwrap();v.push(b'\n');v},
    };
    let mut input=first.clone();input.extend_from_slice(&suffix);
    let mut gate=JsonLines::new(Chunks{bytes:&input,size:usize::from(at(1)%64)+1,pending:true},max);
    let mut context=Context::from_waker(Waker::noop());let mut output=Vec::new();let mut complete=false;
    for _ in 0..input.len()*6+64{
        let mut empty=[];let mut zero=ReadBuf::new(&mut empty);
        assert!(matches!(Pin::new(&mut gate).poll_read(&mut context,&mut zero),Poll::Ready(Ok(()))));
        let mut storage=[0u8;64];let mut buf=ReadBuf::new(&mut storage[..usize::from(at(2)%64)+1]);
        match Pin::new(&mut gate).poll_read(&mut context,&mut buf){
            Poll::Pending=>continue,
            Poll::Ready(Ok(())) if buf.filled().is_empty()=>{
                assert_eq!(mode,0);assert_eq!(output,input);complete=true;break;
            },
            Poll::Ready(Ok(()))=>output.extend_from_slice(buf.filled()),
            Poll::Ready(Err(_))=>{
                assert_ne!(mode,0);assert_eq!(output,first);
                let mut again=ReadBuf::new(&mut storage);
                assert!(matches!(Pin::new(&mut gate).poll_read(&mut context,&mut again),Poll::Ready(Err(_))));
                assert!(again.filled().is_empty());complete=true;break;
            },
        }
    }
    assert!(complete,"frame reader must make bounded progress");
});
