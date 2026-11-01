// SPDX-License-Identifier: MPL-2.0
//! Fixture-only cooperative worker. Genet is constructed and used entirely on
//! its owning thread. This is not a worker pool or a forced-termination adapter.
use super::{Executor, Request, encode};
use std::{
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

pub struct Local {
    tx: Option<SyncSender<Vec<u8>>>,
    rx: Option<Receiver<Result<ExecutionUpdate>>>,
    worker: Option<JoinHandle<Result<()>>>,
    stopping: Arc<AtomicBool>,
    drained: bool,
    outcome: Option<StopOutcome>,
}

impl Local {
    pub fn new() -> Result<Self> {
        Self::spawn_with(Executor::new)
    }

    fn spawn_with(factory: impl FnOnce() -> Result<Executor> + Send + 'static) -> Result<Self> {
        let (tx, requests) = mpsc::sync_channel::<Vec<u8>>(8);
        let (updates, rx) = mpsc::sync_channel(8);
        let stopping = Arc::new(AtomicBool::new(false));
        let cancelled = stopping.clone();
        let worker = thread::Builder::new()
            .name("cue-proof-local".into())
            .spawn(move || {
                let result: Result<()> = (|| {
                    let mut executor = factory()?;
                    while let Ok(bytes) = requests.recv() {
                        if cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        let request = serde_json::from_slice(&bytes).map_err(backend)?;
                        for update in executor.execute(request)? {
                            if cancelled.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            // Match the process fixture's per-update byte bound.
                            encode(&update).map_err(backend)?;
                            if updates.send(update.into_update()).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = &result {
                    let _ = updates.send(Err(error.clone()));
                }
                result
            })
            .map_err(backend)?;
        Ok(Self {
            tx: Some(tx),
            rx: Some(rx),
            worker: Some(worker),
            stopping,
            drained: false,
            outcome: None,
        })
    }
}

impl ExecutionTransport for Local {
    fn termination(&self) -> Termination {
        Termination::Cooperative
    }

    fn try_send(&mut self, request: &ExecutionRequest) -> Result<SendStatus> {
        let tx = self.tx.as_ref().ok_or(Error::Retired)?;
        let bytes = encode(&Request::from_request(request)?).map_err(backend)?;
        match tx.try_send(bytes) {
            Ok(()) => Ok(SendStatus::Sent),
            Err(TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(TrySendError::Disconnected(_)) => {
                Err(Error::Backend("local proof worker disconnected".into()))
            }
        }
    }

    fn try_recv(&mut self) -> Result<Option<ExecutionUpdate>> {
        match self.rx.as_ref().ok_or(Error::Retired)?.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.drained = true;
                Err(Error::Backend("local proof worker disconnected".into()))
            }
        }
    }

    fn request_stop(&mut self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        self.tx.take();
        // Wake an idle worker or one blocked on a full observation channel.
        // A currently executing command still has to yield cooperatively.
        self.rx.take();
        Ok(())
    }

    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        if !self.stopping.load(Ordering::Acquire) && !self.drained {
            return Ok(None);
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let worker = self.worker.take().expect("finished worker exists");
            self.outcome = Some(match worker.join() {
                Ok(Ok(())) => StopOutcome::Exited,
                _ => StopOutcome::Crashed,
            });
        }
        Ok(self.outcome)
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        let _ = self.request_stop();
        // Fixture cleanup joins trusted, finite static-document work. Hosts that
        // need to stop non-yielding work must require a forced process adapter.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn admission_and_stop_do_not_wait_for_executor_initialization() {
        let (release, wait) = mpsc::channel();
        let (observed, observations) = mpsc::channel();
        let owner = thread::spawn(move || {
            let mut local = Local::spawn_with(move || {
                wait.recv().unwrap();
                Executor::new()
            })
            .unwrap();
            let execution = ExecutionId {
                scope: Scope {
                    owner: 9,
                    generation: 1,
                },
                incarnation: 1,
            };
            for request in 1..=8 {
                assert_eq!(
                    local
                        .try_send(&ExecutionRequest {
                            execution,
                            request: RequestKey {
                                scope: execution.scope,
                                id: RequestId(request)
                            },
                            command: Command::Reload,
                        })
                        .unwrap(),
                    SendStatus::Sent
                );
            }
            assert_eq!(
                local
                    .try_send(&ExecutionRequest {
                        execution,
                        request: RequestKey {
                            scope: execution.scope,
                            id: RequestId(9)
                        },
                        command: Command::Reload,
                    })
                    .unwrap(),
                SendStatus::Busy
            );
            local.request_stop().unwrap();
            let stopped = local.poll_stopped().unwrap();
            observed.send(stopped).unwrap();
        });
        let result = observations.recv_timeout(Duration::from_secs(10));
        // Always release the worker before asserting, so failure cannot strand it.
        let _ = release.send(());
        owner.join().unwrap();
        assert_eq!(result.unwrap(), None);
    }
}
