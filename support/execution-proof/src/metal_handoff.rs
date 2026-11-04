// SPDX-License-Identifier: MPL-2.0
//! Private two-process Metal proof. Mach carries capabilities; the bounded
//! control protocol carries leases. Ready follows a producer GPU completion
//! callback. Release follows the consumer importer's final-use fence and drop.
//! This is not a public wire format, a sandbox, or BrowserEngineKit integration.
use crate::{
    encode_with_limit, read_frame_with_limit, render_wire::WireExecutionJob, write_frame_with_limit,
};
use grafting::metal_surface::{MachSendRight, SharedMetalSurface, lookup_surface};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::CString,
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, SyncSender, TrySendError},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use webcue::execution::{ExecutionId, SendStatus, StopOutcome};
use webcue::{
    frame_slots::{FrameSlotPool, SlotLease},
    grafting_frame::{FrameMetadata, FrameSync, OwnedNativeImage},
    *,
};
use wgpu_cue::{wgpu, *};
use wgpu_cue_netrender::resource_table::{ResourceReceiver, ResourceSender};
use wgpu_cue_netrender::{
    hybrid::HybridSceneSource,
    render_session::{RenderReply, RenderRequest, RenderTransport},
    scene_job::{RenderJob, RenderLimits, SceneSnapshot},
};
const LIMIT: usize = 128 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const SCHEMA: &str = "cue-metal-iosurface-v2";
unsafe extern "C" {
    fn cue_receive_port(out: *mut u32) -> i32;
    fn cue_register(name: *const std::ffi::c_char, port: u32) -> i32;
    fn cue_lookup(name: *const std::ffi::c_char, out: *mut u32) -> i32;
    fn cue_destroy_port(port: u32);
    fn cue_send(remote: u32, bytes: *const u8, length: u32, right: u32) -> i32;
    fn cue_receive(
        local: u32,
        bytes: *mut u8,
        length: *mut u32,
        right: *mut u32,
        pid: *mut i32,
    ) -> i32;
}
fn os(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("Mach result {result}")))
    }
}
struct Inbox {
    port: u32,
    name: Option<CString>,
}
impl Inbox {
    fn new(name: Option<CString>) -> io::Result<Self> {
        let mut port = 0;
        // SAFETY: C writes a single owned receive-port name into a valid pointer.
        unsafe {
            os(cue_receive_port(&mut port))?;
        }
        let mut inbox = Self { port, name: None };
        if let Some(name) = name {
            unsafe {
                os(cue_register(name.as_ptr(), port))?;
            }
            // Unregister only after this Inbox successfully owns the service.
            inbox.name = Some(name);
        }
        Ok(inbox)
    }
    fn receive(&self) -> io::Result<Option<(Wire, Option<MachSendRight>, u32)>> {
        let mut bytes = [0u8; 4096];
        let mut length = 0;
        let mut right = 0;
        let mut pid = 0;
        // SAFETY: bounded C bridge writes at most 4096 bytes and transfers any
        // received send right exactly once; receive is nonblocking.
        let status = unsafe {
            cue_receive(
                self.port,
                bytes.as_mut_ptr(),
                &mut length,
                &mut right,
                &mut pid,
            )
        };
        if status == 1 {
            return Ok(None);
        }
        os(status)?;
        let right = (right != 0).then(|| unsafe { MachSendRight::from_raw(right) });
        let wire = serde_json::from_slice(&bytes[..length as usize])?;
        Ok(Some((wire, right, pid as u32)))
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        unsafe {
            if let Some(name) = &self.name {
                cue_register(name.as_ptr(), 0);
            }
            cue_destroy_port(self.port);
        }
    }
}
fn lookup(name: &str) -> io::Result<MachSendRight> {
    let name = CString::new(name)?;
    let mut port = 0;
    unsafe {
        os(cue_lookup(name.as_ptr(), &mut port))?;
        Ok(MachSendRight::from_raw(port))
    }
}
fn send(port: u32, wire: &Wire, right: Option<&MachSendRight>) -> io::Result<()> {
    let bytes = serde_json::to_vec(wire)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("control packet budget"));
    }
    unsafe {
        os(cue_send(
            port,
            bytes.as_ptr(),
            bytes.len() as u32,
            right.map_or(0, MachSendRight::as_raw),
        ))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
    fn matches(self, other: Self) -> bool {
        self.slot == other.slot
            && self.epoch == other.epoch
            && self.revision == other.revision
            && self.allocation == other.allocation
    }
}
#[derive(Debug, Serialize, Deserialize)]
enum Message {
    Hello {
        adapter: String,
    },
    Acquire {
        revision: u64,
    },
    Offer {
        lease: Lease,
        width: u32,
        height: u32,
    },
    Copy(Lease),
    Ready(Lease),
    Release(Lease),
    Released {
        lease: Lease,
        occupied: usize,
    },
    Refused(String),
    Shutdown,
    Bye,
}
#[derive(Debug, Serialize, Deserialize)]
struct Wire {
    incarnation: u64,
    nonce: u128,
    owner: u128,
    generation: u64,
    message: Message,
}
fn wire(nonce: u128, execution: ExecutionId, message: Message) -> Wire {
    let scope = execution.scope;
    Wire {
        incarnation: execution.incarnation,
        nonce,
        owner: scope.owner,
        generation: scope.generation,
        message,
    }
}
fn check(w: &Wire, nonce: u128, execution: ExecutionId) -> io::Result<()> {
    let scope = execution.scope;
    if w.nonce != nonce
        || w.owner != scope.owner
        || w.generation != scope.generation
        || w.incarnation != execution.incarnation
    {
        return Err(io::Error::other("stale Metal session"));
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Init {
    incarnation: u64,
    schema: String,
    endpoint: String,
    nonce: u128,
    owner: u128,
    generation: u64,
    parent: u32,
    width: u32,
    height: u32,
    scene: Option<String>,
}
struct Offer {
    size: FrameSize,
    lease: Lease,
    right: Option<MachSendRight>,
    ready: bool,
    taken: bool,
}
/// Host-owned transport with two outstanding native frame slots. Drop kills and
/// reaps the private helper, retires the session and drops pending capabilities.
pub struct MetalProcess {
    resources: ResourceSender,
    child: Child,
    inbox: Inbox,
    reply: Option<MachSendRight>,
    nonce: u128,
    scope: Scope,
    incarnation: u64,
    writer: Option<JoinHandle<()>>,
    tx: Option<SyncSender<Vec<u8>>>,
    submitted: BTreeMap<u64, (FrameStamp, FrameSize)>,
    stopping: bool,
    forced: bool,
    last_revision: u64,
    offers: BTreeMap<u64, Offer>,
    responses: Vec<Message>,
    releasing: Option<Lease>,
    lost: bool,
}
impl MetalProcess {
    pub fn spawn(executable: &Path, scope: Scope, snapshot: &SceneSnapshot) -> io::Result<Self> {
        Self::spawn_inner(
            executable,
            ExecutionId {
                scope,
                incarnation: 1,
            },
            snapshot,
            true,
        )
    }
    pub fn spawn_execution(
        executable: &Path,
        execution: ExecutionId,
        snapshot: &SceneSnapshot,
    ) -> io::Result<Self> {
        Self::spawn_inner(executable, execution, snapshot, false)
    }
    fn spawn_inner(
        executable: &Path,
        execution: ExecutionId,
        snapshot: &SceneSnapshot,
        legacy: bool,
    ) -> io::Result<Self> {
        let scope = execution.scope;
        if !executable.is_absolute() || execution.incarnation == 0 {
            return Err(io::Error::other(
                "absolute helper path and positive incarnation required",
            ));
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let endpoint = format!("merely.cue.frame.{}.{nonce:x}", std::process::id());
        let inbox = Inbox::new(Some(CString::new(endpoint.clone())?))?;
        let child = Command::new(executable)
            .arg("--metal-handoff")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut process = Self {
            resources: ResourceSender::new(execution, RenderLimits::default())
                .map_err(io::Error::other)?,
            child,
            inbox,
            reply: None,
            nonce,
            scope,
            incarnation: execution.incarnation,
            writer: None,
            tx: None,
            submitted: BTreeMap::new(),
            stopping: false,
            forced: false,
            last_revision: 0,
            offers: BTreeMap::new(),
            responses: Vec::new(),
            releasing: None,
            lost: false,
        };
        let init = Init {
            incarnation: execution.incarnation,
            schema: SCHEMA.into(),
            endpoint,
            nonce,
            owner: scope.owner,
            generation: scope.generation,
            parent: std::process::id(),
            width: snapshot.size().width,
            height: snapshot.size().height,
            scene: if legacy {
                Some(String::from_utf8(snapshot.bytes().to_vec()).map_err(io::Error::other)?)
            } else {
                None
            },
        };
        write_frame_with_limit(
            process.child.stdin.as_mut().unwrap(),
            &encode_with_limit(&init, LIMIT)?,
            LIMIT,
        )?;
        let mut input = process
            .child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("helper stdin"))?;
        let (tx, jobs) = mpsc::sync_channel::<Vec<u8>>(2);
        process.tx = Some(tx);
        process.writer = Some(
            std::thread::Builder::new()
                .name("cue-metal-input".into())
                .spawn(move || {
                    while let Ok(bytes) = jobs.recv() {
                        if write_frame_with_limit(&mut input, &bytes, LIMIT).is_err() {
                            break;
                        }
                    }
                })?,
        );
        process.wait(|p| p.reply.is_some())?;
        Ok(process)
    }
    pub fn shutdown(&mut self) -> io::Result<()> {
        if !self.offers.is_empty() {
            return Err(io::Error::other("outstanding frame leases"));
        }
        self.send(Message::Shutdown)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some((packet, right, pid)) = self.inbox.receive()? {
                check(&packet, self.nonce, self.execution())?;
                if pid != self.child.id()
                    || right.is_some()
                    || !matches!(packet.message, Message::Bye)
                {
                    return Err(io::Error::other("unexpected shutdown reply"));
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("shutdown reply deadline"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.lost = true;
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("helper shutdown failure"))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("helper exit deadline"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub fn execution(&self) -> ExecutionId {
        ExecutionId {
            scope: self.scope,
            incarnation: self.incarnation,
        }
    }
    pub fn scope(&self) -> Scope {
        self.scope
    }
    pub fn resource_stats(&self) -> (usize, usize, u64) {
        (
            self.resources.entries(),
            self.resources.retained_bytes(),
            self.resources.packets(),
        )
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    fn send(&self, message: Message) -> io::Result<()> {
        if self.lost {
            return Err(io::Error::other("Metal executor lost"));
        }
        send(
            self.reply
                .as_ref()
                .ok_or_else(|| io::Error::other("no return channel"))?
                .as_raw(),
            &wire(self.nonce, self.execution(), message),
            None,
        )
    }
    pub fn poll(&mut self) -> io::Result<()> {
        let result = self.poll_inner();
        if result.is_err() && !self.lost {
            self.abandon();
        }
        result
    }
    fn abandon(&mut self) {
        self.lost = true;
        self.offers.clear();
        self.responses.clear();
        self.releasing = None;
        if self.child.kill().is_ok() {
            self.forced = true;
        }
        let _ = self.child.wait();
    }
    fn poll_inner(&mut self) -> io::Result<()> {
        if self.lost {
            return Err(io::Error::other("Metal executor lost"));
        }
        while let Some((packet, right, pid)) = self.inbox.receive()? {
            if pid != self.child.id() {
                return Err(io::Error::other("unexpected Mach sender"));
            }
            check(&packet, self.nonce, self.execution())?;
            match packet.message {
                Message::Hello { adapter } if self.reply.is_none() => {
                    self.reply =
                        Some(right.ok_or_else(|| io::Error::other("missing return right"))?);
                    eprintln!("Metal helper pid={pid}, adapter={adapter}");
                }
                Message::Offer {
                    lease,
                    width,
                    height,
                } => {
                    if self.offers.len() >= 2
                        || self.offers.contains_key(&lease.epoch)
                        || lease.slot >= 2
                        || width == 0
                        || height == 0
                        || u64::from(width) * u64::from(height) * 4
                            > RenderLimits::default().frame_bytes as u64
                        || self
                            .submitted
                            .get(&lease.revision)
                            .is_some_and(|(stamp, size)| {
                                *stamp != lease.stamp(self.scope)
                                    || *size != FrameSize::new(width, height)
                            })
                        || lease.epoch == 0
                        || lease.revision == 0
                        || self.offers.values().any(|o| o.lease.slot == lease.slot)
                    {
                        return Err(io::Error::other("invalid Metal offer"));
                    }
                    let right = right.ok_or_else(|| io::Error::other("missing IOSurface right"))?;
                    self.offers.insert(
                        lease.epoch,
                        Offer {
                            size: FrameSize::new(width, height),
                            lease,
                            right: Some(right),
                            ready: false,
                            taken: false,
                        },
                    );
                }
                Message::Ready(lease) if right.is_none() => {
                    let offer = self
                        .offers
                        .get_mut(&lease.epoch)
                        .filter(|o| o.lease.matches(lease) && !o.ready)
                        .ok_or_else(|| io::Error::other("stale or duplicate readiness"))?;
                    offer.ready = true;
                }
                Message::Released { lease, occupied } if right.is_none() => {
                    if self.responses.len() >= 8
                        || occupied >= 2
                        || !self
                            .releasing
                            .is_some_and(|expected| expected.matches(lease))
                        || !self.offers.get(&lease.epoch).is_some_and(|offer| {
                            offer.ready && offer.taken && offer.lease.matches(lease)
                        })
                    {
                        return Err(io::Error::other(
                            "unsolicited or stale Metal release acknowledgement",
                        ));
                    }
                    // Apply custody transfer in message order. The helper may
                    // already have queued a new Offer for this same slot.
                    self.offers.remove(&lease.epoch);
                    self.releasing = None;
                    self.responses.push(Message::Released { lease, occupied });
                }
                message if right.is_none() => {
                    if self.responses.len() >= 8 {
                        return Err(io::Error::other("response budget"));
                    }
                    self.responses.push(message);
                }
                _ => return Err(io::Error::other("unexpected capability")),
            }
        }
        if self.child.try_wait()?.is_some() {
            self.lost = true;
            // A fresh process uses fresh storage. EOF never licenses old reuse.
            self.offers.retain(|_, o| o.ready);
            if self.offers.values().any(|o| o.ready && !o.taken) {
                return Ok(());
            }
            return Err(io::Error::other("Metal executor exited"));
        }
        Ok(())
    }
    fn wait(&mut self, predicate: impl Fn(&Self) -> bool) -> io::Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.poll()?;
            if predicate(self) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.abandon();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Metal helper deadline",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub fn acquire(&mut self, revision: u64) -> io::Result<Lease> {
        if revision <= self.last_revision {
            return Err(io::Error::other("stale content revision"));
        }
        self.send(Message::Acquire { revision })?;
        self.wait(|p| {
            p.offers.values().any(|o| o.lease.revision == revision) || !p.responses.is_empty()
        })?;
        if let Some(o) = self.offers.values().find(|o| o.lease.revision == revision) {
            self.last_revision = revision;
            return Ok(o.lease);
        }
        Err(io::Error::other(format!("{:?}", self.responses.remove(0))))
    }
    pub fn allow_copy(&self, lease: Lease) -> io::Result<()> {
        self.send(Message::Copy(lease))
    }
    pub fn wait_ready(&mut self, lease: Lease) -> io::Result<()> {
        self.wait(|p| p.offers.get(&lease.epoch).is_some_and(|o| o.ready))
    }
    /// Nonblocking. No frame reaches Cue until the matching Ready packet has
    /// arrived after the helper observed its copy-completion callback.
    pub fn try_frame(&mut self, lease: Lease) -> io::Result<Option<NativeFrame>> {
        self.poll()?;
        self.take_frame(lease)
    }
    fn take_frame(&mut self, lease: Lease) -> io::Result<Option<NativeFrame>> {
        let offer = self
            .offers
            .get_mut(&lease.epoch)
            .filter(|o| o.lease.matches(lease))
            .ok_or_else(|| io::Error::other("stale frame lease"))?;
        if !offer.ready {
            return Ok(None);
        }
        if offer.taken {
            return Err(io::Error::other("frame already acquired"));
        }
        let right = offer
            .right
            .take()
            .ok_or_else(|| io::Error::other("missing surface"))?;
        let surface = lookup_surface(&right).map_err(io::Error::other)?;
        offer.taken = true;
        Ok(Some(NativeFrame {
            stamp: lease.stamp(self.scope),
            frame: OwnedNativeFrame::new(
                FrameMetadata {
                    size: offer.size,
                    format: PixelFormat::Rgba8Unorm,
                    generation: lease.allocation,
                },
                OwnedNativeImage::IoSurface(surface),
                FrameSync::None,
            ),
            alpha: Alpha::Premultiplied,
            encoding: Encoding::Srgb,
        }))
    }
    /// Accept stamps only from GraftIoSurfaceImporter's completed release list.
    pub fn release(&mut self, stamp: FrameStamp) -> io::Result<usize> {
        self.release_after_poll_delay(stamp, Duration::ZERO)
    }
    /// Qualification control: let Released and the next Offer batch together.
    #[doc(hidden)]
    pub fn release_after_poll_delay(
        &mut self,
        stamp: FrameStamp,
        delay: Duration,
    ) -> io::Result<usize> {
        if self.releasing.is_some() {
            return Err(io::Error::other("Metal release already pending"));
        }
        if stamp.scope != self.scope {
            return Err(io::Error::other("stale release scope"));
        }
        let offer = self
            .offers
            .values()
            .find(|o| o.taken && o.lease.stamp(self.scope) == stamp)
            .ok_or_else(|| io::Error::other("stale release"))?;
        let lease = offer.lease;
        self.releasing = Some(lease);
        self.send(Message::Release(lease))?;
        if !delay.is_zero() {
            std::thread::sleep(delay)
        }
        self.wait(|p| !p.responses.is_empty())?;
        match self.responses.remove(0) {
            Message::Released {
                lease: ack,
                occupied,
            } if lease.matches(ack) => Ok(occupied),
            other => Err(io::Error::other(format!("release refused: {other:?}"))),
        }
    }
    /// Fault injection for qualification: kill and reap the producer only.
    pub fn kill(&mut self) -> io::Result<()> {
        self.tx.take();
        self.child.kill()?;
        self.forced = true;
        self.child.wait()?;
        self.lost = true;
        self.offers.clear();
        Ok(())
    }
}
impl Drop for MetalProcess {
    fn drop(&mut self) {
        self.tx.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

struct Pending {
    lease: Lease,
    job: Option<RenderJob>,
    rendered: Option<TextureFrame>,
    allow_copy: bool,
    done: Option<Arc<AtomicBool>>,
    published: bool,
}
impl RenderTransport for MetalProcess {
    type Frame = NativeFrame;
    fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus> {
        if self.lost || self.stopping {
            return Err(Error::Retired);
        }
        if request.execution != self.execution()
            || request.job.stamp.scope != self.scope
            || request.job.stamp.content_revision <= self.last_revision
        {
            return Err(Error::Stale);
        }
        if self.submitted.len() >= 2 {
            return Ok(SendStatus::Busy);
        }
        let (wire, next_resources) = WireExecutionJob::prepare(request, &self.resources)
            .map_err(|e| Error::Backend(e.to_string()))?;
        let bytes = encode_with_limit(&wire, LIMIT).map_err(|e| Error::Backend(e.to_string()))?;
        match self.tx.as_ref().ok_or(Error::Retired)?.try_send(bytes) {
            Ok(()) => {
                if let Some(next) = next_resources {
                    self.resources = next;
                }
                self.last_revision = request.job.stamp.content_revision;
                self.submitted.insert(
                    request.job.stamp.content_revision,
                    (request.job.stamp, request.job.snapshot.size()),
                );
                Ok(SendStatus::Sent)
            }
            Err(TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(TrySendError::Disconnected(_)) => {
                Err(Error::Backend("Metal input writer lost".into()))
            }
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<NativeFrame>>> {
        let observed = self.poll();
        let ready = self
            .offers
            .values()
            .find(|o| o.ready && !o.taken && self.submitted.contains_key(&o.lease.revision))
            .map(|o| o.lease);
        if let Some(lease) = ready {
            let frame = self
                .take_frame(lease)
                .map_err(|e| Error::Backend(e.to_string()))?
                .ok_or(Error::NotPending)?;
            self.submitted.remove(&lease.revision);
            return Ok(Some(RenderReply {
                execution: self.execution(),
                stamp: frame.stamp,
                result: Ok(frame),
            }));
        }
        observed.map_err(|e| Error::Backend(e.to_string()))?;
        Ok(None)
    }
    fn request_stop(&mut self) -> Result<()> {
        self.stopping = true;
        self.tx.take();
        self.offers.clear();
        if self
            .child
            .try_wait()
            .map_err(|e| Error::Backend(e.to_string()))?
            .is_none()
        {
            self.child
                .kill()
                .map_err(|e| Error::Backend(e.to_string()))?;
            self.forced = true;
        }
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        self.child
            .try_wait()
            .map_err(|e| Error::Backend(e.to_string()))
            .map(|s| {
                s.map(|status| {
                    if self.forced && !status.success() {
                        StopOutcome::Terminated
                    } else if status.success() {
                        StopOutcome::Exited
                    } else {
                        StopOutcome::Crashed
                    }
                })
            })
    }
}
pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let init: Init =
        read_frame_with_limit(&mut io::stdin().lock(), LIMIT)?.ok_or("missing Metal job")?;
    if init.schema != SCHEMA || init.incarnation == 0 {
        return Err("Metal schema/incarnation mismatch".into());
    }
    let scope = Scope {
        owner: init.owner,
        generation: init.generation,
    };
    let execution = ExecutionId {
        scope,
        incarnation: init.incarnation,
    };
    let legacy_snapshot = init
        .scene
        .map(|bytes| SceneSnapshot::from_bytes(bytes.into_bytes(), RenderLimits::default()))
        .transpose()?;
    let size = FrameSize::new(init.width, init.height);
    if legacy_snapshot.as_ref().is_some_and(|s| s.size() != size) {
        return Err("legacy init extent mismatch".into());
    }
    let mut viewport = Viewport {
        width: size.width,
        height: size.height,
        scale: 1.0,
    };
    let core = genet_render_host::RenderCore::boot(Default::default())?;
    let info = core.renderer().wgpu_device.core.adapter.get_info();
    if info.backend != wgpu::Backend::Metal || info.device_type == wgpu::DeviceType::Cpu {
        return Err("native Metal adapter required".into());
    }
    let host = GpuHost::new(core.device().clone(), core.queue().clone())?;
    let native_host = grafting::HostWgpuContext::new(core.device().clone(), core.queue().clone());
    let mut surfaces = [
        (
            size,
            SharedMetalSurface::new(&native_host, size, PixelFormat::Rgba8Unorm)?,
        ),
        (
            size,
            SharedMetalSurface::new(&native_host, size, PixelFormat::Rgba8Unorm)?,
        ),
    ];
    let mut source =
        HybridSceneSource::new(scope, &host.context(viewport)?, RenderLimits::default())?;
    let warmup = Instant::now() + TIMEOUT;
    while !source.poll_idle()? {
        if Instant::now() >= warmup {
            return Err("Hybrid initialization deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let remote = lookup(&init.endpoint)?;
    let inbox = Inbox::new(None)?;
    let reply = unsafe { MachSendRight::from_raw(inbox.port) };
    send(
        remote.as_raw(),
        &wire(init.nonce, execution, Message::Hello { adapter: info.name }),
        Some(&reply),
    )?;
    let mut resources = ResourceReceiver::new(execution, RenderLimits::default())?;
    let (incoming, jobs) = mpsc::sync_channel::<io::Result<RenderRequest>>(2);
    std::thread::Builder::new()
        .name("cue-metal-job-reader".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            loop {
                match read_frame_with_limit::<WireExecutionJob>(&mut input, LIMIT) {
                    Ok(Some(job)) => {
                        if incoming.send(job.request_with(&mut resources)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = incoming.send(Err(error));
                        break;
                    }
                }
            }
        })?;
    let mut pool = FrameSlotPool::new(scope, 2)?;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut waiting: Option<RenderRequest> = None;
    let mut active: Option<u64> = None;
    let mut revision = 0;
    let mut last_activity = Instant::now();
    loop {
        core.device().poll(wgpu::PollType::Poll)?;
        if let Some(epoch) = active
            && let Rendered::Changed(frame) = source.render(&host.context(viewport)?, true)?
        {
            pending
                .get_mut(&epoch)
                .ok_or("missing active render")?
                .rendered = Some(frame);
            active = None;
        }
        for state in pending.values_mut() {
            if state.allow_copy
                && state.done.is_none()
                && let Some(frame) = state.rendered.as_ref()
            {
                let mut encoder = core.device().create_command_encoder(&Default::default());
                encoder.copy_texture_to_texture(
                    frame.texture.as_image_copy(),
                    surfaces[state.lease.slot as usize]
                        .1
                        .texture()
                        .as_image_copy(),
                    frame.texture.size(),
                );
                core.queue().submit([encoder.finish()]);
                let done = Arc::new(AtomicBool::new(false));
                let callback = done.clone();
                core.queue()
                    .on_submitted_work_done(move || callback.store(true, Ordering::Release));
                state.done = Some(done);
            }
            if !state.published
                && state
                    .done
                    .as_ref()
                    .is_some_and(|d| d.load(Ordering::Acquire))
            {
                pool.producer_done(state.lease.slot(scope))?;
                state.rendered = None;
                send(
                    remote.as_raw(),
                    &wire(init.nonce, execution, Message::Ready(state.lease)),
                    None,
                )?;
                state.published = true;
            }
        }
        if active.is_none()
            && let Some((&epoch, state)) = pending.iter_mut().find(|(_, p)| p.job.is_some())
        {
            let job = state.job.take().ok_or("missing render job")?;
            let extent = job.snapshot.size();
            viewport = Viewport {
                width: extent.width,
                height: extent.height,
                scale: 1.0,
            };
            source.set_viewport(viewport)?;
            source.submit(
                &host.context(viewport)?,
                job.stamp.content_revision,
                &job.snapshot,
            )?;
            active = Some(epoch);
        }
        if waiting.is_none() {
            match jobs.try_recv() {
                Ok(request) => {
                    waiting = Some(request?);
                    last_activity = Instant::now();
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if pool.occupied() < 2
            && let Some(request) = waiting.take()
        {
            if request.execution != execution
                || request.job.stamp.scope != scope
                || request.job.stamp.content_revision <= revision
            {
                return Err("stale Metal render request".into());
            }
            revision = request.job.stamp.content_revision;
            let slot = pool.acquire()?;
            let extent = request.job.snapshot.size();
            if surfaces[slot.slot as usize].0 != extent {
                surfaces[slot.slot as usize] = (
                    extent,
                    SharedMetalSurface::new(&native_host, extent, PixelFormat::Rgba8Unorm)?,
                );
            }
            let lease = Lease {
                slot: slot.slot,
                epoch: slot.epoch,
                revision,
                allocation: request.job.stamp.allocation_epoch,
            };
            let right = surfaces[lease.slot as usize].1.export_right()?;
            send(
                remote.as_raw(),
                &wire(
                    init.nonce,
                    execution,
                    Message::Offer {
                        lease,
                        width: extent.width,
                        height: extent.height,
                    },
                ),
                Some(&right),
            )?;
            pending.insert(
                lease.epoch,
                Pending {
                    lease,
                    job: Some(request.job),
                    rendered: None,
                    allow_copy: true,
                    done: None,
                    published: false,
                },
            );
        }
        if let Some((packet, right, pid)) = inbox.receive()? {
            last_activity = Instant::now();
            if pid != init.parent || right.is_some() {
                return Err("unexpected helper sender/capability".into());
            }
            check(&packet, init.nonce, execution)?;
            match packet.message {
                Message::Acquire { revision: next } => {
                    let slot = if next <= revision {
                        Err(Error::Stale)
                    } else {
                        pool.acquire()
                    };
                    match slot {
                        Ok(slot) => {
                            revision = next;
                            if surfaces[slot.slot as usize].0 != size {
                                surfaces[slot.slot as usize] = (
                                    size,
                                    SharedMetalSurface::new(
                                        &native_host,
                                        size,
                                        PixelFormat::Rgba8Unorm,
                                    )?,
                                );
                            }
                            let lease = Lease {
                                slot: slot.slot,
                                epoch: slot.epoch,
                                revision,
                                allocation: revision,
                            };
                            let right = surfaces[lease.slot as usize].1.export_right()?;
                            send(
                                remote.as_raw(),
                                &wire(
                                    init.nonce,
                                    execution,
                                    Message::Offer {
                                        lease,
                                        width: size.width,
                                        height: size.height,
                                    },
                                ),
                                Some(&right),
                            )?;
                            pending.insert(
                                lease.epoch,
                                Pending {
                                    lease,
                                    job: Some(RenderJob {
                                        stamp: lease.stamp(scope),
                                        snapshot: legacy_snapshot
                                            .as_ref()
                                            .ok_or("Acquire requires legacy init snapshot")?
                                            .clone(),
                                    }),
                                    rendered: None,
                                    allow_copy: false,
                                    done: None,
                                    published: false,
                                },
                            );
                        }
                        Err(error) => send(
                            remote.as_raw(),
                            &wire(init.nonce, execution, Message::Refused(error.to_string())),
                            None,
                        )?,
                    }
                }
                Message::Copy(lease) => {
                    let p = pending
                        .get_mut(&lease.epoch)
                        .filter(|p| p.lease.matches(lease) && !p.allow_copy)
                        .ok_or("stale copy lease")?;
                    p.allow_copy = true;
                }
                Message::Release(lease) => {
                    let result = pending
                        .get(&lease.epoch)
                        .filter(|p| p.lease.matches(lease) && p.published)
                        .ok_or(Error::Stale)
                        .and_then(|_| pool.consumer_released(lease.slot(scope)));
                    match result {
                        Ok(()) => {
                            pending.remove(&lease.epoch);
                            send(
                                remote.as_raw(),
                                &wire(
                                    init.nonce,
                                    execution,
                                    Message::Released {
                                        lease,
                                        occupied: pool.occupied(),
                                    },
                                ),
                                None,
                            )?;
                        }
                        Err(e) => send(
                            remote.as_raw(),
                            &wire(init.nonce, execution, Message::Refused(e.to_string())),
                            None,
                        )?,
                    }
                }
                Message::Shutdown => {
                    if pool.occupied() != 0 {
                        return Err("shutdown with outstanding leases".into());
                    }
                    pool.retire();
                    send(
                        remote.as_raw(),
                        &wire(init.nonce, execution, Message::Bye),
                        None,
                    )?;
                    return Ok(());
                }
                _ => return Err("unexpected helper command".into()),
            }
        }
        if last_activity.elapsed() > TIMEOUT {
            return Err("Metal helper idle deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
