//! Lifetime owner for the native worker, not a generic process/executor framework.
//! stdin is delivered only AFTER the worker is attached to its process boundary.
use crate::error::{Error,Result};
use std::{io::Write,process::{Child,Command,Stdio}};
pub struct ProcessTree {
    pub child:Child,
    #[cfg(unix)] group:i32,
    #[cfg(windows)] job:windows_sys::Win32::Foundation::HANDLE,
}
impl ProcessTree {
    pub fn spawn(command:&mut Command,payload:&[u8])->Result<Self>{
        command.stdin(Stdio::piped());
        #[cfg(unix)] {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child=command.spawn()?;
        #[cfg(unix)]
        let mut owner={
            let group=match i32::try_from(child.id()) {
                Ok(id) if id>0=>id,
                _=>{let _=child.kill();let _=child.wait();return Err(Error::new("PROCESS_GROUP","Invalid worker process id"));}
            };
            Self{child,group}
        };
        #[cfg(windows)]
        let mut owner={
            use std::{os::windows::io::AsRawHandle,ptr};
            use windows_sys::Win32::{Foundation::CloseHandle,System::JobObjects::*};
            // SAFETY: named pointers are null; returned handle is checked and owned here.
            let job=unsafe{CreateJobObjectW(ptr::null(),ptr::null())};
            if job.is_null(){let _=child.kill();let _=child.wait();return Err(Error::new("PROCESS_JOB","Cannot create worker job"));}
            // SAFETY: the Win32 structure is plain numeric POD; zeroed optional fields are valid.
            let mut limits:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=unsafe{std::mem::zeroed()};
            limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: both handles are live. The pointer/size name the exact Win32 structure.
            let configured=unsafe{SetInformationJobObject(job,JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),std::mem::size_of_val(&limits) as u32)}!=0;
            let assigned=configured&&unsafe{AssignProcessToJobObject(job,child.as_raw_handle().cast())}!=0;
            if !assigned {
                let _=child.kill();let _=child.wait();
                // SAFETY: job is exclusively owned and has not been closed.
                unsafe{CloseHandle(job)};
                return Err(Error::new("PROCESS_JOB","Cannot attach worker to kill-on-close job; native query was not started"));
            }
            Self{child,job}
        };
        #[cfg(not(any(unix,windows)))] {
            let _=child.kill();let _=child.wait();
            return Err(Error::new("UNSUPPORTED_PLATFORM","Native worker process boundary is unavailable"));
        }
        #[cfg(any(unix,windows))] {
            // Worker reads through EOF before it opens the native runtime. No race
            // between native process creation and Windows job assignment.
            let mut stdin=owner.child.stdin.take().ok_or_else(||Error::new("WORKER_STDIN","Worker pipe missing"))?;
            stdin.write_all(payload)?;
            drop(stdin);
            Ok(owner)
        }
    }
    pub fn stop(&mut self){
        #[cfg(unix)] {
            unsafe extern "C" { fn kill(pid:std::os::raw::c_int,sig:std::os::raw::c_int)->std::os::raw::c_int; }
            // SAFETY: negative PGID targets only the group created for this child.
            // SIGKILL is 9 on the supported Linux/macOS targets. No signal to PGID 0.
            if self.group>0 {unsafe{kill(-self.group,9)}; self.group=0;}
        }
        #[cfg(windows)] {
            use windows_sys::Win32::{Foundation::CloseHandle,System::JobObjects::TerminateJobObject};
            if !self.job.is_null(){
                // SAFETY: job is exclusively owned; explicit termination plus final
                // close covers remaining descendants even after worker exit.
                unsafe{TerminateJobObject(self.job,1);CloseHandle(self.job);}
                self.job=std::ptr::null_mut();
            }
        }
        let _=self.child.kill();
        let _=self.child.wait();
    }
}
impl Drop for ProcessTree {fn drop(&mut self){self.stop();}}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn worker_receives_bounded_payload_after_ownership_is_established() {
        use std::io::Read;
        let mut command = Command::new("/bin/cat"); command.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut owner = ProcessTree::spawn(&mut command, b"request payload").unwrap();
        let mut output = Vec::new(); owner.child.stdout.take().unwrap().read_to_end(&mut output).unwrap();
        assert!(owner.child.wait().unwrap().success()); assert_eq!(output,b"request payload");
        owner.stop(); owner.stop();
    }
    #[test]
    fn stop_terminates_owned_worker_and_is_idempotent() {
        let mut command = Command::new("/bin/sleep"); command.arg("60").stdout(Stdio::null()).stderr(Stdio::null());
        let mut owner = ProcessTree::spawn(&mut command, b"").unwrap();
        owner.stop(); assert!(owner.child.try_wait().unwrap().is_some()); owner.stop();
    }
}
