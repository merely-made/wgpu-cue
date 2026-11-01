// SPDX-License-Identifier: MPL-2.0
//! Fixture-only supervisor. The helper has no subprocess-spawning code. It is
//! unsandboxed and accepts trusted static HTML. Methods never join; Drop performs
//! final kill/reap and bridge joins so failing tests cannot leak helpers.
use super::{Request, Update, encode, read_frame, write_frame};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};
use webcue::{execution::*, *};

fn backend(error: impl std::fmt::Display) -> Error {
    Error::Backend(error.to_string())
}

pub struct Process {
    child: Child,
    tx: Option<SyncSender<Vec<u8>>>,
    rx: Option<Receiver<Result<ExecutionUpdate>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    failed: Arc<AtomicBool>,
    stopping: bool,
    drained: bool,
    outcome: Option<StopOutcome>,
}
impl Process {
    pub fn spawn(executable: &Path, hold: bool) -> Result<Self> {
        if !executable.is_absolute() {
            return Err(Error::Invalid("proof helper path must be absolute".into()));
        }
        let mut command = Command::new(executable);
        if hold {
            command.arg("--hold");
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(backend)?;
        // Install custody before any later fallible initialization.
        let mut process = Self {
            child,
            tx: None,
            rx: None,
            writer: None,
            reader: None,
            failed: Arc::new(AtomicBool::new(false)),
            stopping: false,
            drained: false,
            outcome: None,
        };
        let mut input = process
            .child
            .stdin
            .take()
            .ok_or_else(|| Error::Invalid("missing stdin".into()))?;
        let mut output = process
            .child
            .stdout
            .take()
            .ok_or_else(|| Error::Invalid("missing stdout".into()))?;
        let (tx, requests) = mpsc::sync_channel::<Vec<u8>>(8);
        let (updates, rx) = mpsc::sync_channel(8);
        process.tx = Some(tx);
        process.rx = Some(rx);
        let failed = process.failed.clone();
        process.writer = Some(
            thread::Builder::new()
                .name("cue-proof-writer".into())
                .spawn(move || {
                    while let Ok(bytes) = requests.recv() {
                        if write_frame(&mut input, &bytes).is_err() {
                            failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                })
                .map_err(backend)?,
        );
        process.reader = Some(
            thread::Builder::new()
                .name("cue-proof-reader".into())
                .spawn(move || {
                    loop {
                        let value = match read_frame::<Update>(&mut output) {
                            Ok(Some(update)) => update.into_update(),
                            Ok(None) => break,
                            Err(error) => {
                                let _ = updates.send(Err(backend(error)));
                                break;
                            }
                        };
                        if updates.send(value).is_err() {
                            break;
                        }
                    }
                })
                .map_err(backend)?,
        );
        Ok(process)
    }
}
impl ExecutionTransport for Process {
    fn termination(&self) -> Termination {
        Termination::Forced
    }
    fn try_send(&mut self, request: &ExecutionRequest) -> Result<SendStatus> {
        if self.stopping {
            return Err(Error::Retired);
        }
        if self.failed.load(Ordering::Acquire) {
            return Err(Error::Backend("proof writer disconnected".into()));
        }
        let bytes = encode(&Request::from_request(request)?).map_err(backend)?;
        match self.tx.as_ref().ok_or(Error::Retired)?.try_send(bytes) {
            Ok(()) => Ok(SendStatus::Sent),
            Err(TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(TrySendError::Disconnected(_)) => {
                Err(Error::Backend("proof writer disconnected".into()))
            }
        }
    }
    fn try_recv(&mut self) -> Result<Option<ExecutionUpdate>> {
        match self.rx.as_ref().ok_or(Error::Retired)?.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.drained = true;
                Err(Error::Backend("proof reader disconnected".into()))
            }
        }
    }
    fn request_stop(&mut self) -> Result<()> {
        if self.stopping || self.outcome.is_some() {
            return Ok(());
        }
        if let Some(status) = self.child.try_wait().map_err(backend)? {
            self.outcome = Some(if status.success() {
                StopOutcome::Exited
            } else {
                StopOutcome::Crashed
            });
        } else {
            self.child.kill().map_err(backend)?;
            self.stopping = true;
        }
        self.tx.take();
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        // Before shutdown, let the reader deliver all buffered observations even
        // if the OS already knows that the child exited.
        if !self.stopping && !self.drained && self.outcome.is_none() {
            return Ok(None);
        }
        if self.outcome.is_none()
            && let Some(status) = self.child.try_wait().map_err(backend)?
        {
            self.outcome = Some(if self.stopping {
                StopOutcome::Terminated
            } else if status.success() {
                StopOutcome::Exited
            } else {
                StopOutcome::Crashed
            });
        }
        Ok(self.outcome)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.tx.take();
        self.rx.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
