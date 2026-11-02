// SPDX-License-Identifier: MPL-2.0
//! Private persistent CPU endpoints for host lifecycle qualification. Scheduling
//! is fixture mechanics; no production worker pool or process policy is imposed.
use crate::{
    encode_with_limit,
    raster::{Pixels, WireReply},
    read_frame_with_limit,
    render_wire::WireExecutionJob,
    write_frame_with_limit,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};
use webcue::{
    execution::{ExecutionId, SendStatus, StopOutcome},
    *,
};
use wgpu_cue_netrender::{cpu::rasterize, render_session::*, scene_job::RenderLimits};
const LIMIT: usize = 128 * 1024 * 1024;
fn backend(e: impl std::fmt::Display) -> Error {
    Error::Backend(e.to_string())
}
fn compute(request: RenderRequest) -> RenderReply<CpuFrame> {
    RenderReply {
        execution: request.execution,
        stamp: request.job.stamp,
        result: rasterize(&request.job, RenderLimits::default())
            .map_err(|e| RenderFailure::Refused(e.to_string())),
    }
}
/// A cooperative, bounded local owner. Only CPU frames cross its thread channel.
pub struct LocalCpu {
    tx: Option<SyncSender<RenderRequest>>,
    rx: Option<Receiver<RenderReply<CpuFrame>>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    stopped: Option<StopOutcome>,
}
impl LocalCpu {
    pub fn new() -> Result<Self> {
        let (tx, jobs) = mpsc::sync_channel(2);
        let (replies, rx) = mpsc::sync_channel(2);
        let stopping = Arc::new(AtomicBool::new(false));
        let cancelled = stopping.clone();
        let worker = thread::Builder::new()
            .name("cue-render-local".into())
            .spawn(move || {
                while let Ok(request) = jobs.recv() {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let result = compute(request);
                    if cancelled.load(Ordering::Acquire) || replies.send(result).is_err() {
                        break;
                    }
                }
            })
            .map_err(backend)?;
        Ok(Self {
            tx: Some(tx),
            rx: Some(rx),
            stopping,
            worker: Some(worker),
            stopped: None,
        })
    }
}
impl RenderTransport for LocalCpu {
    type Frame = CpuFrame;
    fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus> {
        match self
            .tx
            .as_ref()
            .ok_or(Error::Retired)?
            .try_send(request.clone())
        {
            Ok(()) => Ok(SendStatus::Sent),
            Err(TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(TrySendError::Disconnected(_)) => Err(backend("local render owner lost")),
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<CpuFrame>>> {
        match self.rx.as_ref().ok_or(Error::Retired)?.try_recv() {
            Ok(reply) => Ok(Some(reply)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(backend("local render owner exited")),
        }
    }
    fn request_stop(&mut self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        self.tx.take();
        self.rx.take();
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.stopped = Some(if self.worker.take().unwrap().join().is_ok() {
                StopOutcome::Exited
            } else {
                StopOutcome::Crashed
            });
        }
        Ok(self.stopped)
    }
}
impl Drop for LocalCpu {
    fn drop(&mut self) {
        let _ = self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Identity {
    owner: u128,
    generation: u64,
    incarnation: u64,
    allocation: u64,
    revision: u64,
}
impl Identity {
    fn new(execution: ExecutionId, stamp: FrameStamp) -> Self {
        Self {
            owner: execution.scope.owner,
            generation: execution.scope.generation,
            incarnation: execution.incarnation,
            allocation: stamp.allocation_epoch,
            revision: stamp.content_revision,
        }
    }
    fn execution(&self) -> ExecutionId {
        ExecutionId {
            scope: Scope {
                owner: self.owner,
                generation: self.generation,
            },
            incarnation: self.incarnation,
        }
    }
    fn stamp(&self) -> FrameStamp {
        FrameStamp {
            scope: self.execution().scope,
            allocation_epoch: self.allocation,
            content_revision: self.revision,
        }
    }
}
#[derive(Serialize, Deserialize)]
enum Reply {
    Started(Identity),
    Finished {
        identity: Identity,
        pixels: WireReply,
    },
}
pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let hold = std::env::args().any(|s| s == "--hold-render");
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some(wire) = read_frame_with_limit::<WireExecutionJob>(&mut input, LIMIT)? {
        let request = wire.request()?;
        let identity = Identity::new(request.execution, request.job.stamp);
        write_frame_with_limit(
            &mut output,
            &encode_with_limit(
                &Reply::Started(Identity::new(request.execution, request.job.stamp)),
                LIMIT,
            )?,
            LIMIT,
        )?;
        if hold {
            loop {
                thread::park();
            }
        }
        let result = compute(request);
        let pixels = match result.result {
            Ok(frame) => WireReply::Frame {
                owner: frame.stamp.scope.owner,
                generation: frame.stamp.scope.generation,
                allocation: frame.stamp.allocation_epoch,
                revision: frame.stamp.content_revision,
                width: frame.size.width,
                height: frame.size.height,
                stride: frame.stride,
                convention: Pixels::PremultipliedSrgb,
                rgba: frame.rgba,
            },
            Err(error) => WireReply::Refused(format!("{error:?}")),
        };
        write_frame_with_limit(
            &mut output,
            &encode_with_limit(&Reply::Finished { identity, pixels }, LIMIT)?,
            LIMIT,
        )?;
    }
    Ok(())
}
/// Persistent child, bounded writer/reader bridges, and unconditional kill/reap
/// cleanup. Ordinary transport calls never join or perform pipe I/O.
pub struct ProcessCpu {
    child: Child,
    tx: Option<SyncSender<Vec<u8>>>,
    rx: Option<Receiver<io::Result<Reply>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    failed: Arc<AtomicBool>,
    stopping: bool,
    forced: bool,
    stopped: Option<StopOutcome>,
    started: BTreeSet<u64>,
}
impl ProcessCpu {
    pub fn spawn(executable: &Path, hold: bool) -> Result<Self> {
        if !executable.is_absolute() {
            return Err(Error::Invalid("absolute helper path required".into()));
        }
        let mut command = Command::new(executable);
        command.arg("--render-stream");
        if hold {
            command.arg("--hold-render");
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(backend)?;
        let mut process = Self {
            child,
            tx: None,
            rx: None,
            writer: None,
            reader: None,
            failed: Arc::new(AtomicBool::new(false)),
            stopping: false,
            forced: false,
            stopped: None,
            started: BTreeSet::new(),
        };
        let mut input = process
            .child
            .stdin
            .take()
            .ok_or_else(|| backend("helper stdin"))?;
        let mut output = process
            .child
            .stdout
            .take()
            .ok_or_else(|| backend("helper stdout"))?;
        let (tx, jobs) = mpsc::sync_channel::<Vec<u8>>(2);
        process.tx = Some(tx);
        let (replies, rx) = mpsc::sync_channel(2);
        process.rx = Some(rx);
        let failed = process.failed.clone();
        process.writer = Some(
            thread::Builder::new()
                .name("cue-render-writer".into())
                .spawn(move || {
                    while let Ok(bytes) = jobs.recv() {
                        if write_frame_with_limit(&mut input, &bytes, LIMIT).is_err() {
                            failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                })
                .map_err(backend)?,
        );
        process.reader = Some(
            thread::Builder::new()
                .name("cue-render-reader".into())
                .spawn(move || {
                    loop {
                        match read_frame_with_limit(&mut output, LIMIT) {
                            Ok(Some(reply)) => {
                                if replies.send(Ok(reply)).is_err() {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                let _ = replies.send(Err(e));
                                break;
                            }
                        }
                    }
                })
                .map_err(backend)?,
        );
        Ok(process)
    }
    pub fn finish_input(&mut self) {
        self.tx.take();
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn started(&self, revision: u64) -> bool {
        self.started.contains(&revision)
    }
    pub fn kill(&mut self) -> Result<()> {
        self.request_stop()
    }
}
impl RenderTransport for ProcessCpu {
    type Frame = CpuFrame;
    fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus> {
        if self.failed.load(Ordering::Acquire) {
            return Err(backend("render writer lost"));
        }
        let bytes = encode_with_limit(&WireExecutionJob::new(request).map_err(backend)?, LIMIT)
            .map_err(backend)?;
        match self.tx.as_ref().ok_or(Error::Retired)?.try_send(bytes) {
            Ok(()) => Ok(SendStatus::Sent),
            Err(TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(TrySendError::Disconnected(_)) => Err(backend("render child lost")),
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<CpuFrame>>> {
        for _ in 0..64 {
            match self.rx.as_ref().ok_or(Error::Retired)?.try_recv() {
                Ok(Ok(Reply::Started(id))) => {
                    if self.started.len() >= 64 {
                        return Err(backend("started witness budget"));
                    }
                    self.started.insert(id.revision);
                }
                Ok(Ok(Reply::Finished { identity, pixels })) => {
                    self.started.remove(&identity.revision);
                    let execution = identity.execution();
                    let stamp = identity.stamp();
                    let result = match pixels {
                        WireReply::Refused(reason) => Err(RenderFailure::Refused(reason)),
                        other => Ok(other.frame().map_err(backend)?),
                    };
                    return Ok(Some(RenderReply {
                        execution,
                        stamp,
                        result,
                    }));
                }
                Ok(Err(e)) => return Err(backend(e)),
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => return Err(backend("render child exited")),
            }
        }
        Ok(None)
    }
    fn request_stop(&mut self) -> Result<()> {
        if !self.stopping {
            self.stopping = true;
            self.tx.take();
            self.rx.take();
            if self.child.try_wait().map_err(backend)?.is_none() {
                self.child.kill().map_err(backend)?;
                self.forced = true;
            }
        }
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        if self.stopped.is_some() {
            return Ok(self.stopped);
        }
        if let Some(status) = self.child.try_wait().map_err(backend)? {
            self.stopped = Some(if self.forced && !status.success() {
                StopOutcome::Terminated
            } else if status.success() {
                StopOutcome::Exited
            } else {
                StopOutcome::Crashed
            });
        }
        Ok(self.stopped)
    }
}
impl Drop for ProcessCpu {
    fn drop(&mut self) {
        let _ = self.request_stop();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
