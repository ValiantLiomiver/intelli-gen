//! Typed requests and inspection work. Every stopped handle belongs to one
//! adapter-side epoch; late replies can never populate a later stop or session.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc};
use std::thread::JoinHandle;
use serde_json::{Value, json};
use super::{DebugClient, DebugError};
use super::breakpoints::BreakpointStatus;
use super::variables::{Evaluation, Scope, Variable, array, id, text};

#[derive(Clone, Debug)]
pub struct Thread { pub id: i64, pub name: String }
#[derive(Clone, Debug)]
pub struct StackFrame {
    pub id: i64, pub name: String, pub source: Option<PathBuf>, pub line: u32, pub column: u32,
}
/// Odd generations represent a stop. Even generations invalidate every handle.
#[derive(Clone, Default)]
pub struct StopEpoch(Arc<AtomicU64>);
impl StopEpoch {
    pub fn current(&self) -> u64 { self.0.load(Ordering::Acquire) }
    pub fn stopped(&self) { let _ = self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| Some(if value % 2 == 0 { value.wrapping_add(1) } else { value.wrapping_add(2) })); }
    pub fn resumed(&self) { let _ = self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| Some(if value % 2 == 1 { value.wrapping_add(1) } else { value })); }
    pub(super) fn resume_if(&self, epoch: u64) -> bool { self.0.compare_exchange(epoch, epoch.wrapping_add(1), Ordering::AcqRel, Ordering::Acquire).is_ok() }
}
#[derive(Clone)]
pub struct DebugController { client: DebugClient, stop: StopEpoch, pub epoch: u64 }
impl DebugController {
    pub fn new(client: DebugClient, stop: StopEpoch) -> Self { let epoch = stop.current(); Self { client, stop, epoch } }
    fn check(&self) -> Result<(), DebugError> {
        if self.epoch % 2 == 1 && self.stop.current() == self.epoch { Ok(()) }
        else { Err(DebugError::Configuration("The stopped frame is no longer current".into())) }
    }
    async fn stopped_request(&self, command: &str, args: Value) -> Result<Value, DebugError> {
        self.check()?;
        let result = self.client.request_checked(command, args, || self.check()).await?;
        self.check()?;
        Ok(result)
    }
    async fn resume(&self, command: &str, thread_id: i64) -> Result<(), DebugError> {
        self.check()?;
        self.client.request_checked(command, json!({"threadId":thread_id,"singleThread":false}), || self.check()).await?;
        self.client.resumed(self.epoch, thread_id)?;
        Ok(())
    }
    pub async fn continue_execution(&self, thread_id: i64) -> Result<(), DebugError> { self.resume("continue",thread_id).await }
    pub async fn step_over(&self, thread_id: i64) -> Result<(), DebugError> { self.resume("next",thread_id).await }
    pub async fn step_into(&self, thread_id: i64) -> Result<(), DebugError> { self.resume("stepIn",thread_id).await }
    pub async fn step_out(&self, thread_id: i64) -> Result<(), DebugError> { self.resume("stepOut",thread_id).await }
    fn check_running(&self) -> Result<(), DebugError> {
        if self.epoch % 2 == 0 && self.stop.current() == self.epoch { Ok(()) }
        else { Err(DebugError::Configuration("The running debug state is no longer current".into())) }
    }
    pub async fn pause(&self) -> Result<(), DebugError> {
        self.check_running()?;
        // Acquire a live thread here rather than reusing a previous stopped ID.
        let body = self.client.request_checked("threads",json!({}), || self.check_running()).await?;
        let thread = array(&body,"threads")?.first().ok_or_else(|| DebugError::Configuration("No live Java thread to pause".into()))?;
        self.client.request_checked("pause",json!({"threadId":id(thread,"id")?}), || self.check_running()).await?;
        Ok(())
    }
    pub async fn threads(&self) -> Result<Vec<Thread>, DebugError> {
        let body = self.stopped_request("threads",json!({})).await?;
        array(&body,"threads")?.iter().map(|v| Ok(Thread{id:id(v,"id")?,name:text(v,"name")})).collect()
    }
    pub async fn stack_trace(&self, thread_id: i64) -> Result<Vec<StackFrame>, DebugError> {
        let body = self.stopped_request("stackTrace",json!({"threadId":thread_id,"startFrame":0})).await?;
        array(&body,"stackFrames")?.iter().map(|v| Ok(StackFrame {
            id:id(v,"id")?,name:text(v,"name"),source:v["source"]["path"].as_str().map(PathBuf::from),
            line:v["line"].as_u64().unwrap_or(1) as u32,column:v["column"].as_u64().unwrap_or(1) as u32,
        })).collect()
    }
    pub async fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>, DebugError> {
        let body = self.stopped_request("scopes",json!({"frameId":frame_id})).await?;
        array(&body,"scopes")?.iter().map(|v| Ok(Scope {name:text(v,"name"),variables_reference:id(v,"variablesReference")?,expensive:v["expensive"].as_bool().unwrap_or(false)})).collect()
    }
    pub async fn variables(&self, reference: i64) -> Result<Vec<Variable>, DebugError> {
        if reference <= 0 { return Ok(Vec::new()); }
        let body = self.stopped_request("variables",json!({"variablesReference":reference})).await?;
        array(&body,"variables")?.iter().map(|v| Ok(Variable { name:text(v,"name"),value:text(v,"value"),type_name:v["type"].as_str().map(str::to_owned),variables_reference:id(v,"variablesReference")? })).collect()
    }
    pub async fn evaluate(&self, expression: &str, frame_id: i64) -> Result<Evaluation, DebugError> {
        let body = self.stopped_request("evaluate",json!({"expression":expression,"frameId":frame_id,"context":"repl"})).await?;
        Ok(Evaluation {result:text(&body,"result"),type_name:body["type"].as_str().map(str::to_owned),variables_reference:body["variablesReference"].as_i64().unwrap_or(0)})
    }
    pub async fn set_breakpoints(&self, arguments: Value) -> Result<Vec<BreakpointStatus>, DebugError> {
        let body = self.client.request("setBreakpoints", arguments).await?;
        Ok(array(&body,"breakpoints")?.iter().map(BreakpointStatus::from_adapter).collect())
    }
}

#[derive(Clone, Debug)]
pub enum InspectRequest {
    Collapse(i64), Threads, Stack(i64), Scopes(i64), Variables { frame: i64, reference: i64 }, Evaluate { frame: i64, expression: String },
    Continue(i64), Pause, StepOver(i64), StepInto(i64), StepOut(i64),
    Breakpoints { source: PathBuf, revision: u64, arguments: Value },
}
pub enum InspectReply { Threads(Vec<Thread>), Stack(Vec<StackFrame>), Scopes(Vec<Scope>), Variables(Vec<Variable>), Evaluation(Evaluation), Breakpoints(Vec<BreakpointStatus>), Control }
struct Work { controller: DebugController, selection: u64, request: InspectRequest }
struct Completion { epoch: u64, selection: u64, request: InspectRequest, result: Result<InspectReply, DebugError> }
struct Worker { commands: mpsc::SyncSender<Work>, completions: mpsc::Receiver<Completion>, handle: JoinHandle<()> }
#[derive(Default)]
pub struct DebugInspector {
    worker: Option<Worker>, retiring: Vec<JoinHandle<()>>,
    pending_breakpoints: BTreeMap<PathBuf, Work>,
    // Only one Threads -> Stack -> Scopes stage is current at a time. Keep its
    // unsent work across backpressure, scoped to the stop and selection.
    pending_inspection: Option<Work>,
    breakpoint_inflight: HashMap<PathBuf, u64>,
    pub epoch: u64, pub selection: u64, pub threads: Vec<Thread>, pub frames: Vec<StackFrame>, pub scopes: Vec<Scope>,
    pub preferred_thread: Option<i64>,
    pub selected_thread: Option<i64>, pub selected_frame: Option<i64>, pub variables: HashMap<i64, Vec<Variable>>,
    pub expanded: HashSet<i64>, pending_variables: HashSet<i64>,
    pub error: Option<String>, pub console: Vec<(String, Result<Evaluation,String>)>, pub expression: String,
    pub navigation: Option<(PathBuf, usize)>, pub control_pending: bool, control_epoch: Option<u64>,
}
impl DebugInspector {
    pub fn has_workers(&self) -> bool { self.worker.is_some() || !self.retiring.is_empty() }
    pub fn clear_handles(&mut self, epoch: u64) {
        self.pending_inspection=None;
        for (_, result) in &mut self.console {
            if let Ok(evaluation) = result {
                evaluation.variables_reference = 0;
            }
        }
        self.epoch=epoch; self.selection=self.selection.wrapping_add(1); self.threads.clear(); self.frames.clear(); self.scopes.clear();
        self.preferred_thread=None; self.selected_thread=None; self.selected_frame=None; self.variables.clear(); self.expanded.clear(); self.pending_variables.clear(); self.navigation=None; self.control_pending=false; self.control_epoch=None;
    }
    pub fn detach(&mut self) {
        if let Some(worker)=self.worker.take() { drop(worker.commands); self.retiring.push(worker.handle); }
        self.pending_breakpoints.clear(); self.breakpoint_inflight.clear();
        self.clear_handles(0);
    }
    pub fn attach(&mut self) -> Result<(), DebugError> {
        self.detach(); self.console.clear(); self.error=None;
        let (commands, requests)=mpsc::sync_channel::<Work>(64);
        let (replies, completions)=mpsc::channel();
        let handle=std::thread::Builder::new().name("debug-inspection".into()).spawn(move || {
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build();
            while let Ok(work)=requests.recv() {
                let result=match &runtime { Ok(runtime)=>runtime.block_on(execute(&work.controller,&work.request)),Err(error)=>Err(DebugError::Transport(format!("Cannot create inspection runtime: {error}"))) };
                if replies.send(Completion {epoch:work.controller.epoch,selection:work.selection,request:work.request,result}).is_err() { break; }
            }
        }).map_err(|error|DebugError::Transport(format!("Cannot start debugger inspection worker: {error}")))?;
        self.worker=Some(Worker{commands,completions,handle}); Ok(())
    }
    fn flush_breakpoints(&mut self, breakpoints: &mut super::breakpoints::Breakpoints) {
        let Some(worker)=&self.worker else {return;};
        let sources:Vec<_>=self.pending_breakpoints.keys().cloned().collect();
        for source in sources {
            if self.breakpoint_inflight.contains_key(&source) {continue;}
            let Some(work)=self.pending_breakpoints.remove(&source) else {continue;};
            let revision=match &work.request {InspectRequest::Breakpoints{revision,..}=>*revision,_=>continue};
            match worker.commands.try_send(work) {
                Ok(())=>{breakpoints.begin_sync(&source,revision);self.breakpoint_inflight.insert(source,revision);}
                Err(mpsc::TrySendError::Full(work))=>{self.pending_breakpoints.insert(source,work);break;}
                Err(mpsc::TrySendError::Disconnected(work))=>{
                    self.pending_breakpoints.insert(source,work);
                    self.error=Some("Debugger inspection worker stopped before breakpoint synchronization".into());
                    break;
                }
            }
        }
    }
    pub fn submit(&mut self, controller: DebugController, request: InspectRequest) {
        if let InspectRequest::Breakpoints{source,..}=&request {
            // Keep only the latest desired replacement per source. A full queue
            // leaves it dirty here; poll retries without blocking the UI.
            self.pending_breakpoints.insert(source.clone(),Work{controller,selection:self.selection,request});
            return;
        }
        if matches!(&request, InspectRequest::Threads | InspectRequest::Stack(_) | InspectRequest::Scopes(_)) {
            self.pending_inspection=Some(Work{controller,selection:self.selection,request});
            return;
        }
        let control=matches!(&request, InspectRequest::Continue(_) | InspectRequest::Pause | InspectRequest::StepOver(_) | InspectRequest::StepInto(_) | InspectRequest::StepOut(_));
        let request_epoch=controller.epoch;
        let variable=match &request {InspectRequest::Variables{reference,..}=>Some(*reference),_=>None};
        let evaluation=match &request {InspectRequest::Evaluate{expression,..}=>Some(expression.clone()),_=>None};
        let result=self.worker.as_ref().ok_or_else(||"No debug session is active".to_owned()).and_then(|worker|
            worker.commands.try_send(Work{controller,selection:self.selection,request}).map_err(|error|format!("Debugger request queue unavailable: {error}")));
        match result {
            Ok(())=>{if control {self.control_pending=true;self.control_epoch=Some(request_epoch);}},
            Err(error)=>{
                // A rejected expansion must be clickable again, rather than
                // displaying Loading forever with no request in flight.
                if let Some(reference)=variable {self.pending_variables.remove(&reference);self.expanded.remove(&reference);}
                if let Some(expression)=evaluation {self.console.push((expression,Err(error.clone())));}
                self.error=Some(error);
            }
        }
    }
    fn flush_inspection(&mut self) {
        let Some(work)=self.pending_inspection.take() else {return;};
        if work.controller.epoch!=self.epoch || work.selection!=self.selection || work.controller.stop.current()!=self.epoch {return;}
        let Some(worker)=&self.worker else {self.error=Some("No debug session is active".into());return;};
        match worker.commands.try_send(work) {
            Ok(())=>{},
            Err(mpsc::TrySendError::Full(work))=>self.pending_inspection=Some(work),
            Err(mpsc::TrySendError::Disconnected(_))=>self.error=Some("Debugger inspection worker stopped".into()),
        }
    }
    pub fn select_thread(&mut self, controller: DebugController, thread: i64) {
        if !self.threads.iter().any(|item|item.id==thread) {return;}
        self.selection=self.selection.wrapping_add(1); self.selected_thread=Some(thread); self.selected_frame=None;
        self.frames.clear(); self.scopes.clear(); self.variables.clear(); self.expanded.clear(); self.pending_variables.clear();
        self.submit(controller,InspectRequest::Stack(thread));
    }
    pub fn select_frame(&mut self, controller: DebugController, frame: i64) {
        let Some(selected)=self.frames.iter().find(|item|item.id==frame) else {return;};
        self.navigation=selected.source.clone().map(|source|(source,selected.line.max(1) as usize));
        self.selection=self.selection.wrapping_add(1); self.selected_frame=Some(frame); self.scopes.clear(); self.variables.clear(); self.expanded.clear(); self.pending_variables.clear();
        self.submit(controller,InspectRequest::Scopes(frame));
    }
    pub fn expand(&mut self, controller: DebugController, frame: i64, reference: i64) {
        if self.selected_frame!=Some(frame) || reference<=0 {return;}
        self.expanded.insert(reference);
        if !self.variables.contains_key(&reference) && self.pending_variables.insert(reference) { self.submit(controller,InspectRequest::Variables{frame,reference}); }
    }
    pub fn reap(&mut self) {
        let mut pending=Vec::new();
        for handle in self.retiring.drain(..) { if handle.is_finished() {let _=handle.join();} else {pending.push(handle);} }
        self.retiring=pending;
    }
    pub fn poll(&mut self, controller: Option<DebugController>, breakpoints: &mut super::breakpoints::Breakpoints, stopped_thread: Option<i64>) {
        self.reap();
        let Some(controller)=controller else {return;};
        if controller.epoch!=self.epoch {
            let preferred=stopped_thread.or(self.selected_thread);
            self.clear_handles(controller.epoch);
            self.preferred_thread=preferred;
            if self.epoch%2==1 { self.submit(controller.clone(),InspectRequest::Threads); }
        }
        if let Some(thread)=stopped_thread { self.preferred_thread=Some(thread); }
        let completions: Vec<_>=self.worker.as_ref().map(|worker|worker.completions.try_iter().collect()).unwrap_or_default();
        for completion in completions {
            if let InspectRequest::Breakpoints{source,revision,..}=&completion.request {
                if self.breakpoint_inflight.get(source)==Some(revision) {self.breakpoint_inflight.remove(source);}
                if *revision==breakpoints.source_revision(source) {match completion.result {Ok(InspectReply::Breakpoints(status))=>breakpoints.apply(source,*revision,status),Err(error)=>{breakpoints.finish_sync(source,*revision);self.error=Some(error.to_string());},_=>{breakpoints.finish_sync(source,*revision);}}}
                continue;
            }
            if matches!(&completion.request,InspectRequest::Continue(_)|InspectRequest::Pause|InspectRequest::StepOver(_)|InspectRequest::StepInto(_)|InspectRequest::StepOut(_)) {
                if self.control_epoch!=Some(completion.epoch) {continue;}
                self.control_pending=false; self.control_epoch=None;
                if let Err(error)=completion.result {self.error=Some(error.to_string());}
                continue;
            }
            if completion.epoch!=self.epoch || completion.selection!=self.selection || controller.stop.current()!=self.epoch {continue;}
            match completion.result {
                Err(error)=>{
                    if let InspectRequest::Evaluate{expression,..}=completion.request { self.console.push((expression,Err(error.to_string()))); }
                    else { if let InspectRequest::Variables{reference,..}=completion.request {self.pending_variables.remove(&reference);self.expanded.remove(&reference);} self.error=Some(error.to_string()); }
                }
                Ok(reply)=>match reply {
                    InspectReply::Threads(threads)=>{
                        let preferred=self.preferred_thread.take().or(self.selected_thread); self.threads=threads;
                        if let Some(thread)=preferred.filter(|thread|self.threads.iter().any(|item|item.id==*thread)).or_else(||self.threads.first().map(|thread|thread.id)) { self.select_thread(controller.clone(),thread); }
                    }
                    InspectReply::Stack(frames)=>{
                        if let InspectRequest::Stack(thread)=completion.request {if self.selected_thread!=Some(thread) {continue;}}
                        self.frames=frames; if let Some(frame)=self.frames.first() {self.select_frame(controller.clone(),frame.id);}
                    }
                    InspectReply::Scopes(scopes)=>{
                        self.scopes=scopes;
                        let refs:Vec<_>=self.scopes.iter().filter(|scope|!scope.expensive).map(|scope|scope.variables_reference).collect();
                        if let Some(frame)=self.selected_frame {for reference in refs {self.expand(controller.clone(),frame,reference);}}
                    }
                    InspectReply::Variables(variables)=>{if let InspectRequest::Variables{reference,..}=completion.request {self.pending_variables.remove(&reference);self.variables.insert(reference,variables);}}
                    InspectReply::Evaluation(result)=>{if let InspectRequest::Evaluate{expression,..}=completion.request {self.console.push((expression,Ok(result)));}}
                    _=>{}
                }
            }
        }
        self.flush_inspection();
        self.flush_breakpoints(breakpoints);
        if self.console.len()>200 {self.console.drain(..self.console.len()-200);}
    }
}
async fn execute(controller:&DebugController, request:&InspectRequest)->Result<InspectReply,DebugError> {
    Ok(match request {
        InspectRequest::Collapse(_)=>InspectReply::Control,
        InspectRequest::Threads=>InspectReply::Threads(controller.threads().await?),
        InspectRequest::Stack(thread)=>InspectReply::Stack(controller.stack_trace(*thread).await?),
        InspectRequest::Scopes(frame)=>InspectReply::Scopes(controller.scopes(*frame).await?),
        InspectRequest::Variables{reference,..}=>InspectReply::Variables(controller.variables(*reference).await?),
        InspectRequest::Evaluate{frame,expression}=>InspectReply::Evaluation(controller.evaluate(expression,*frame).await?),
        InspectRequest::Breakpoints{arguments,..}=>InspectReply::Breakpoints(controller.set_breakpoints(arguments.clone()).await?),
        InspectRequest::Continue(thread)=>{controller.continue_execution(*thread).await?;InspectReply::Control},
        InspectRequest::Pause=>{controller.pause().await?;InspectReply::Control},
        InspectRequest::StepOver(thread)=>{controller.step_over(*thread).await?;InspectReply::Control},
        InspectRequest::StepInto(thread)=>{controller.step_into(*thread).await?;InspectReply::Control},
        InspectRequest::StepOut(thread)=>{controller.step_out(*thread).await?;InspectReply::Control},
    })
}
