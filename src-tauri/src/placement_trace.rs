//! Bounded sampling, not a log line for every failed reconciliation tick.
use std::{collections::VecDeque, sync::{LazyLock,Mutex}, time::Instant};
use windows::Win32::{Foundation::HWND, System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*};

#[derive(Default)]
struct Budget { recent: VecDeque<u64>, targets: VecDeque<(isize,u64)> }
impl Budget {
    fn admit(&mut self, id:isize, now:u64) -> bool {
        while self.recent.front().is_some_and(|t| now.saturating_sub(*t)>=1000) { self.recent.pop_front(); }
        if self.recent.len()>=4 || self.targets.iter().any(|(h,t)|*h==id && now.saturating_sub(*t)<2000) { return false; }
        self.targets.retain(|(h,_)|*h!=id);
        if self.targets.len()==32 { self.targets.pop_front(); }
        self.targets.push_back((id,now)); self.recent.push_back(now); true
    }
}
struct Pending { id:isize, root:isize, pid:u32, tid:u32, at:u64, text:String }
struct Trace { start:Instant, budget:Budget, attempts:u64, errors:u64, sampled:u64,
    skipped_ignored:u64, skipped_ineligible:u64, pending:Vec<Pending>, records:VecDeque<String>, disk_lines:u32 }
impl Trace {
    fn new()->Self { Self { start:Instant::now(),budget:Budget::default(),attempts:0,errors:0,sampled:0,
        skipped_ignored:0,skipped_ineligible:0,pending:Vec::new(),records:VecDeque::new(),disk_lines:0 } }
}
static TRACE:LazyLock<Mutex<Trace>>=LazyLock::new(||Mutex::new(Trace::new()));

unsafe fn identity(h:HWND)->(u32,u32) { let mut pid=0; let tid=GetWindowThreadProcessId(h,Some(&mut pid)); (pid,tid) }
unsafe fn security(pid:u32)->String {
    use windows::Win32::{Foundation::{CloseHandle,HANDLE}, Security::*, System::Threading::*};
    let process=match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) {
        Ok(h)=>h,Err(e)=>return format!("unavailable({:#x})",e.code().0) };
    let mut token=HANDLE::default();
    let opened=OpenProcessToken(process,TOKEN_QUERY,&mut token);
    let _=CloseHandle(process);
    if let Err(e)=opened {return format!("unavailable({:#x})",e.code().0);}
    let result=(|| -> windows::core::Result<(u32,u32)> {
        let mut len=0;
        let _=GetTokenInformation(token,TokenIntegrityLevel,None,0,&mut len);
        if len==0 { return Err(windows::core::Error::from_win32()); }
        // usize allocation provides TOKEN_MANDATORY_LABEL alignment.
        let mut buffer=vec![0usize;(len as usize+std::mem::size_of::<usize>()-1)/std::mem::size_of::<usize>()];
        GetTokenInformation(token,TokenIntegrityLevel,Some(buffer.as_mut_ptr() as *mut _),len,&mut len)?;
        let label=&*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count=*GetSidSubAuthorityCount(label.Label.Sid);
        if count==0 {return Err(windows::core::Error::from_win32());}
        let integrity=*GetSidSubAuthority(label.Label.Sid,count as u32-1);
        let mut ui_access=0u32;
        GetTokenInformation(token,TokenUIAccess,Some(&mut ui_access as *mut _ as *mut _),4,&mut len)?;
        Ok((integrity,ui_access))
    })();
    let _=CloseHandle(token);
    match result {Ok((rid,ui))=>format!("integrity_rid={rid:#x},uiaccess={ui}"),Err(e)=>format!("unavailable({:#x})",e.code().0)}
}
unsafe fn position(h:HWND,root:HWND)->String {
    let owner=GetWindow(h,GW_OWNER).unwrap_or_default().0 as isize;
    let prev=GetWindow(h,GW_HWNDPREV).unwrap_or_default().0 as isize;
    let next=GetWindow(h,GW_HWNDNEXT).unwrap_or_default().0 as isize;
    format!("valid={} root_valid={} above_root={:?} prev={prev:#x} next={next:#x} owner={owner:#x} exstyle={:#x} gpu=[{}]",
        IsWindow(Some(h)).as_bool(),IsWindow(Some(root)).as_bool(),crate::window_order::above(h,root),
        GetWindowLongW(h,GWL_EXSTYLE),crate::gpu_blur::diagnostic_order(h.0 as isize))
}

/// Sample the synchronous API result and immediate position. Next observation
/// happens at tracker-loop entry before our next GPU pin or reconciliation.
pub unsafe fn lower(target:HWND,root:HWND,source:&str)->windows::core::Result<()> {
    lower_after(target,root,root,source)
}

pub unsafe fn lower_after(target:HWND,after_window:HWND,root:HWND,source:&str)->windows::core::Result<()> {
    let sample={ let mut trace=TRACE.lock().unwrap(); trace.attempts+=1;
        let now=trace.start.elapsed().as_millis() as u64;
        if trace.pending.len()<4 && trace.budget.admit(target.0 as isize,now) { trace.sampled+=1; Some((trace.sampled,now)) } else { None } };
    let (pid,tid)=if sample.is_some(){identity(target)}else{(0,0)};
    let security_info=sample.map(|_|format!("target_security=[{}] self_security=[{}]",security(pid),security(std::process::id())));
    let before=sample.map(|_|position(target,root));
    let result=crate::window_order::lower_window(target,after_window);
    let after=sample.map(|_|position(target,root));
    let mut trace=TRACE.lock().unwrap();
    if result.is_err() { trace.errors+=1; }
    if let Some((sequence,now))=sample {
        let outcome=match &result {Ok(())=>"ok".to_string(),Err(e)=>format!("error_hresult={:#x}",e.code().0)};
        trace.pending.push(Pending { id:target.0 as isize,root:root.0 as isize,pid,tid,at:now,
            text:format!("placement#{sequence} t_ms={now} source={source} hwnd={:#x} root={:#x} insert_after={:#x} pid={pid} tid={tid} caller_tid={} {} result={outcome} before=[{}] after=[{}]",
                target.0 as isize,root.0 as isize,after_window.0 as isize,GetCurrentThreadId(),security_info.unwrap(),before.unwrap(),after.unwrap()) });
    }
    result
}

pub unsafe fn observe_next() {
    let pending={ std::mem::take(&mut TRACE.lock().unwrap().pending) };
    for p in pending {
        let h=HWND(p.id as *mut _); let root=HWND(p.root as *mut _);
        let same=identity(h)==(p.pid,p.tid) && p.pid!=0;
        let observed=if same {position(h,root)} else {"destroyed-or-reused".into()};
        let mut trace=TRACE.lock().unwrap();
        let elapsed=(trace.start.elapsed().as_millis() as u64).saturating_sub(p.at);
        let text=format!("{} next_after_ms={elapsed} next=[{observed}]",p.text);
        if trace.records.len()==16 { trace.records.pop_front(); }
        trace.records.push_back(text.clone());
        let log=trace.disk_lines<120;
        if log {trace.disk_lines+=1;}
        drop(trace);
        if log {crate::logging::log(&text);}
    }
}

pub fn skipped(ignored:bool) {
    let mut trace=TRACE.lock().unwrap();
    if ignored {trace.skipped_ignored+=1;} else {trace.skipped_ineligible+=1;}
}

pub fn report()->String {
    let trace=TRACE.lock().unwrap();
    format!("placement_diagnostic=1 attempts={} api_errors={} skipped_ignored={} skipped_ineligible={} sampled={} pending={} disk_lines={}/120 sampling=4_per_second_global,1_per_2_seconds_per_hwnd records=last_16\n{}\n",
        trace.attempts,trace.errors,trace.skipped_ignored,trace.skipped_ineligible,trace.sampled,trace.pending.len(),trace.disk_lines,
        trace.records.iter().cloned().collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sampling_is_bounded_and_does_not_starve_a_second_target() {
        let mut budget=Budget::default();
        assert!(budget.admit(1,0)); assert!(!budget.admit(1,1)); assert!(budget.admit(2,1));
        assert!(budget.admit(3,2)); assert!(budget.admit(4,3)); assert!(!budget.admit(5,4));
        assert!(!budget.admit(1,1000)); assert!(budget.admit(1,2000));
        for id in 10..100 { budget.admit(id,id as u64*3000); }
        assert!(budget.targets.len()<=32); assert!(budget.recent.len()<=4);
    }
}
