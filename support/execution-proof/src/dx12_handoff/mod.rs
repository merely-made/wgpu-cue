// SPDX-License-Identifier: MPL-2.0
//! Private Windows proof. Anonymous pipes belong to this spawned child only.
//! Resource/fence integers designate CHILD handles: the parent duplicates from
//! its owned child process handle, never adopts arbitrary local handle values.
use super::render_wire::WireExecutionJob;
use crate::{Identity, encode_with_limit, read_frame_with_limit, write_frame_with_limit};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use webcue::grafting_frame::{
    Dx12SharedFence, Dx12SharedResource, Dx12SharedTexture, FrameMetadata, FrameSync,
    OwnedNativeFrame, OwnedNativeImage,
};
use webcue::{
    execution::*,
    frame_slots::{FrameSlotPool, SlotLease},
    *,
};
use wgpu_cue::*;
use wgpu_cue_netrender::{hybrid::HybridSceneSource, render_session::*, scene_job::*};
use windows::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    System::Threading::GetCurrentProcess,
};
mod validation;
mod worker;
pub use worker::helper;
pub fn check_validation(host: &grafting::HostWgpuContext) -> io::Result<()> {
    validation::check(host)
}
const LIMIT: usize = 12 * 1024 * 1024;
const SCHEMA: &str = "cue-dx12-shared-v1";
const TIMEOUT: Duration = Duration::from_secs(30);
fn bad(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn backend(e: impl std::fmt::Display) -> Error {
    Error::Backend(e.to_string())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub slot: u32,
    pub epoch: u64,
    pub revision: u64,
    pub allocation: u64,
}
impl Lease {
    fn slot(self, scope: Scope) -> SlotLease {
        SlotLease {
            scope,
            slot: self.slot,
            epoch: self.epoch,
        }
    }
    pub fn stamp(self, scope: Scope) -> FrameStamp {
        FrameStamp {
            scope,
            allocation_epoch: self.allocation,
            content_revision: self.revision,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Init {
    schema: String,
    identity: Identity,
    nonce: u128,
    width: u32,
    height: u32,
    luid: u64,
}
#[derive(Serialize, Deserialize)]
enum Input {
    Job(WireExecutionJob),
    Release(Lease),
}
#[derive(Serialize, Deserialize)]
enum Message {
    Hello {
        adapter: String,
        luid: u64,
    },
    Ready {
        lease: Lease,
        width: u32,
        height: u32,
        resource: u64,
        fence: u64,
        value: u64,
    },
    Released(Lease),
    Stale(Lease),
}
#[derive(Serialize, Deserialize)]
struct Packet {
    schema: String,
    identity: Identity,
    nonce: u128,
    message: Message,
}
impl Packet {
    fn new(execution: ExecutionId, nonce: u128, message: Message) -> Self {
        Self {
            schema: SCHEMA.into(),
            identity: execution.into(),
            nonce,
            message,
        }
    }
    fn check(&self, execution: ExecutionId, nonce: u128) -> io::Result<()> {
        if self.schema != SCHEMA
            || ExecutionId::from(self.identity) != execution
            || self.nonce != nonce
        {
            return Err(bad("DX12 packet identity/schema mismatch"));
        }
        Ok(())
    }
}
/// Hardware-only device. The child must select the parent's exact adapter LUID.
pub fn gpu() -> io::Result<(wgpu::Adapter, GpuHost, grafting::HostWgpuContext)> {
    gpu_for(None)
}
pub fn gpu_with_instance() -> io::Result<(
    wgpu::Instance,
    wgpu::Adapter,
    GpuHost,
    grafting::HostWgpuContext,
)> {
    gpu_for_instance(None)
}
fn gpu_for(luid: Option<u64>) -> io::Result<(wgpu::Adapter, GpuHost, grafting::HostWgpuContext)> {
    let (_, adapter, host, native) = gpu_for_instance(luid)?;
    Ok((adapter, host, native))
}
fn gpu_for_instance(
    luid: Option<u64>,
) -> io::Result<(
    wgpu::Instance,
    wgpu::Adapter,
    GpuHost,
    grafting::HostWgpuContext,
)> {
    validation::enable()?;
    static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
    let instance = INSTANCE.get_or_init(|| {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::VALIDATION,
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        })
    });
    let selected = std::env::var("CUE_DX12_ADAPTER_NAME").ok();
    for adapter in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12)) {
        if selected
            .as_ref()
            .is_some_and(|name| !adapter.get_info().name.contains(name))
        {
            continue;
        }
        if !matches!(
            adapter.get_info().device_type,
            wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu
        ) {
            continue;
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).map_err(bad)?;
        let native = grafting::HostWgpuContext::new(device.clone(), queue.clone());
        let actual = grafting::dx12_surface::adapter_luid(&native).map_err(bad)?;
        if luid.is_some_and(|wanted| wanted != actual) {
            continue;
        }
        validation::check(&native)?;
        eprintln!(
            "DX12 pid={} LUID={actual:016x} adapter={:?}; debug layer required",
            std::process::id(),
            adapter.get_info()
        );
        return Ok((
            instance.clone(),
            adapter,
            GpuHost::new(device, queue).map_err(bad)?,
            native,
        ));
    }
    Err(bad("matching hardware DX12 adapter required"))
}
fn duplicate(child: &Child, value: u64) -> io::Result<OwnedHandle> {
    if value == 0 || value > isize::MAX as u64 {
        return Err(bad("invalid child NT handle"));
    }
    let mut local = HANDLE::default();
    unsafe {
        DuplicateHandle(
            HANDLE(child.as_raw_handle()),
            HANDLE(value as usize as *mut _),
            GetCurrentProcess(),
            &mut local,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }
    .map_err(bad)?;
    Ok(unsafe { OwnedHandle::from_raw_handle(local.0) })
}
// Own startup failure cleanup before the full transport is constructed.
struct Startup(Option<Child>);
impl Drop for Startup {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
struct Offered {
    lease: Lease,
    taken: bool,
}
pub struct Dx12Process {
    child: Child,
    execution: ExecutionId,
    nonce: u128,
    luid: u64,
    tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    packets: mpsc::Receiver<io::Result<Packet>>,
    failed: Arc<AtomicBool>,
    submitted: BTreeMap<u64, (FrameStamp, FrameSize)>,
    offered: BTreeMap<u64, Offered>,
    replies: VecDeque<RenderReply<NativeFrame>>,
    released: Vec<Lease>,
    stale: Vec<Lease>,
    adapter: Option<String>,
    stopped: Option<StopOutcome>,
    forced: bool,
    peer_closed: bool,
}
impl Dx12Process {
    pub fn spawn_execution(
        executable: &Path,
        execution: ExecutionId,
        snapshot: &SceneSnapshot,
        luid: u64,
    ) -> io::Result<Self> {
        if !executable.is_absolute() || execution.incarnation == 0 {
            return Err(bad("absolute helper path/nonzero incarnation required"));
        }
        let mut startup = Startup(Some(
            Command::new(executable)
                // Redirected worker pipes never require a desktop console.
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .arg("--dx12-handoff")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?,
        ));
        let child = startup.0.as_mut().unwrap();
        let input = child.stdin.take().ok_or_else(|| bad("helper stdin"))?;
        let mut output = child.stdout.take().ok_or_else(|| bad("helper stdout"))?;
        let (packet_tx, packets) = mpsc::sync_channel(4);
        let reader = thread::Builder::new()
            .name("cue-dx12-reader".into())
            .spawn(move || {
                loop {
                    match read_frame_with_limit::<Packet>(&mut output, LIMIT) {
                        Ok(Some(packet)) => {
                            if packet_tx.send(Ok(packet)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            let _ = packet_tx.send(Err(e));
                            break;
                        }
                    }
                }
            })?;
        let mut random = [0u8; 16];
        // SystemPreferredRng avoids process/time-derived session nonces.
        unsafe {
            windows::Win32::Security::Cryptography::BCryptGenRandom(
                None,
                &mut random,
                windows::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        }
        .ok()
        .map_err(bad)?;
        let nonce = u128::from_ne_bytes(random);
        let (tx, jobs) = mpsc::sync_channel::<Vec<u8>>(4);
        let failed = Arc::new(AtomicBool::new(false));
        let fault = failed.clone();
        let writer = thread::Builder::new()
            .name("cue-dx12-writer".into())
            .spawn(move || {
                let mut input = input;
                while let Ok(bytes) = jobs.recv() {
                    if write_frame_with_limit(&mut input, &bytes, LIMIT).is_err() {
                        fault.store(true, Ordering::Release);
                        break;
                    }
                }
            })?;
        let mut process = Self {
            child: startup.0.take().unwrap(),
            execution,
            nonce,
            luid,
            tx: Some(tx),
            writer: Some(writer),
            reader: Some(reader),
            packets,
            failed,
            submitted: BTreeMap::new(),
            offered: BTreeMap::new(),
            replies: VecDeque::new(),
            released: Vec::new(),
            stale: Vec::new(),
            adapter: None,
            stopped: None,
            forced: false,
            peer_closed: false,
        };
        let size = snapshot.size();
        process.send_bytes(encode_with_limit(
            &Init {
                schema: SCHEMA.into(),
                identity: execution.into(),
                nonce,
                width: size.width,
                height: size.height,
                luid,
            },
            LIMIT,
        )?)?;
        let end = Instant::now() + TIMEOUT;
        while process.adapter.is_none() {
            process.poll()?;
            if process.peer_closed || Instant::now() >= end {
                return Err(bad("DX12 helper startup failed/deadline"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(process)
    }
    fn send_bytes(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.tx
            .as_ref()
            .ok_or_else(|| bad("DX12 retired"))?
            .try_send(bytes)
            .map_err(bad)
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn adapter(&self) -> &str {
        self.adapter.as_deref().unwrap_or("")
    }
    pub fn lease(&self, stamp: FrameStamp) -> Option<Lease> {
        self.offered
            .get(&stamp.content_revision)
            .filter(|o| o.lease.stamp(self.execution.scope) == stamp)
            .map(|o| o.lease)
    }
    pub fn release(&mut self, stamp: FrameStamp) -> io::Result<()> {
        let offer = self
            .offered
            .get(&stamp.content_revision)
            .filter(|o| o.taken && o.lease.stamp(self.execution.scope) == stamp)
            .ok_or_else(|| bad("unknown DX12 release"))?;
        self.send_release(offer.lease)
    }
    pub fn send_release(&self, lease: Lease) -> io::Result<()> {
        self.send_bytes(encode_with_limit(&Input::Release(lease), LIMIT)?)
    }
    pub fn released(&self) -> &[Lease] {
        &self.released
    }
    pub fn stale(&self) -> &[Lease] {
        &self.stale
    }
    pub fn poll(&mut self) -> io::Result<()> {
        loop {
            let packet = match self.packets.try_recv() {
                Ok(packet) => packet?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.peer_closed = true;
                    break;
                }
            };
            packet.check(self.execution, self.nonce)?;
            match packet.message {
                Message::Hello { adapter, luid } if self.adapter.is_none() && luid == self.luid => {
                    self.adapter = Some(adapter);
                }
                Message::Ready {
                    lease,
                    width,
                    height,
                    resource,
                    fence,
                    value,
                } => {
                    let (stamp, size) = self
                        .submitted
                        .remove(&lease.revision)
                        .ok_or_else(|| bad("unsolicited/duplicate DX12 frame"))?;
                    if stamp != lease.stamp(self.execution.scope)
                        || size != FrameSize::new(width, height)
                        || lease.slot >= 2
                        || lease.epoch == 0
                        || value == 0
                        || value % 2 == 0
                        || value == u64::MAX
                    {
                        return Err(bad("DX12 frame differs from submitted job"));
                    }
                    let resource =
                        Dx12SharedResource::from_owned_handle(duplicate(&self.child, resource)?);
                    let fence = Dx12SharedFence::from_owned_handle(duplicate(&self.child, fence)?);
                    let frame = NativeFrame {
                        stamp,
                        alpha: Alpha::Premultiplied,
                        encoding: Encoding::Srgb,
                        frame: OwnedNativeFrame::new(
                            FrameMetadata {
                                size,
                                format: PixelFormat::Rgba8Unorm,
                                generation: stamp.allocation_epoch,
                            },
                            OwnedNativeImage::Dx12SharedTexture(Dx12SharedTexture::new(resource)),
                            FrameSync::Dx12Fence { fence, value },
                        ),
                    };
                    self.offered.insert(
                        lease.revision,
                        Offered {
                            lease,
                            taken: false,
                        },
                    );
                    self.replies.push_back(RenderReply {
                        execution: self.execution,
                        stamp,
                        result: Ok(frame),
                    });
                }
                Message::Released(lease) => {
                    if !self
                        .offered
                        .get(&lease.revision)
                        .is_some_and(|o| o.lease == lease)
                    {
                        return Err(bad("unsolicited DX12 release receipt"));
                    }
                    self.offered.remove(&lease.revision);
                    if self.released.len() == 64 {
                        self.released.remove(0);
                    }
                    self.released.push(lease);
                }
                Message::Stale(lease) => {
                    if self.stale.len() == 64 {
                        self.stale.remove(0);
                    }
                    self.stale.push(lease);
                }
                _ => return Err(bad("unexpected DX12 message")),
            }
        }
        Ok(())
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.tx.take();
        if !self.forced && self.stopped.is_none() {
            if let Some(status) = self.child.try_wait()? {
                self.stopped = Some(if status.success() {
                    StopOutcome::Exited
                } else {
                    StopOutcome::Crashed
                });
            } else {
                self.child.kill()?;
                self.forced = true;
            }
        }
        Ok(())
    }
}
impl RenderTransport for Dx12Process {
    type Frame = NativeFrame;
    fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus> {
        if request.execution != self.execution
            || request.job.stamp.scope != self.execution.scope
            || self
                .submitted
                .contains_key(&request.job.stamp.content_revision)
            || self
                .offered
                .contains_key(&request.job.stamp.content_revision)
        {
            return Err(Error::Stale);
        }
        if self.failed.load(Ordering::Acquire) {
            return Err(backend("DX12 job writer lost"));
        }
        if self.submitted.len() >= 2 {
            return Ok(SendStatus::Busy);
        }
        let bytes = encode_with_limit(
            &Input::Job(WireExecutionJob::new(request).map_err(backend)?),
            LIMIT,
        )
        .map_err(backend)?;
        match self.tx.as_ref().ok_or(Error::Retired)?.try_send(bytes) {
            Ok(()) => {
                self.submitted.insert(
                    request.job.stamp.content_revision,
                    (request.job.stamp, request.job.snapshot.size()),
                );
                Ok(SendStatus::Sent)
            }
            Err(mpsc::TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(_) => Err(backend("DX12 writer disconnected")),
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<Self::Frame>>> {
        self.poll().map_err(backend)?;
        if let Some(reply) = self.replies.pop_front() {
            if let Some(offer) = self.offered.get_mut(&reply.stamp.content_revision) {
                offer.taken = true;
            }
            return Ok(Some(reply));
        }
        if self.peer_closed || self.poll_stopped()?.is_some() {
            return Err(backend("DX12 renderer exited"));
        }
        Ok(None)
    }
    fn request_stop(&mut self) -> Result<()> {
        self.kill().map_err(backend)
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        if self.stopped.is_none()
            && let Some(status) = self.child.try_wait().map_err(backend)?
        {
            self.stopped = Some(if self.forced {
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
impl Drop for Dx12Process {
    fn drop(&mut self) {
        let _ = self.kill();
        let _ = self.child.wait();
        self.tx.take();
        // Drain bounded reader queue after child death before joining its sender.
        while self.packets.recv_timeout(Duration::from_millis(10)).is_ok() {}
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
