// SPDX-License-Identifier: MPL-2.0
//! Private two-process Metal proof. Mach carries capabilities; the bounded
//! control protocol carries leases. Ready follows a producer GPU completion
//! callback. Release follows the consumer importer's final-use fence and drop.
//! This is not a public wire format, a sandbox, or BrowserEngineKit integration.
use crate::{encode_with_limit, read_frame_with_limit, write_frame_with_limit};
use grafting::metal_surface::{MachSendRight, SharedMetalSurface, lookup_surface};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::CString,
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use webcue::{
    frame_slots::{FrameSlotPool, SlotLease},
    grafting_frame::{FrameMetadata, FrameSync, OwnedNativeImage},
    *,
};
use wgpu_cue::{wgpu, *};
use wgpu_cue_netrender::{
    hybrid::HybridSceneSource,
    scene_job::{RenderLimits, SceneSnapshot},
};
const LIMIT: usize = 128 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const SCHEMA: &str = "cue-metal-iosurface-v1";
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
            allocation_epoch: self.epoch,
            content_revision: self.revision,
        }
    }
    fn matches(self, other: Self) -> bool {
        self.slot == other.slot && self.epoch == other.epoch && self.revision == other.revision
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
    nonce: u128,
    owner: u128,
    generation: u64,
    message: Message,
}
fn wire(nonce: u128, scope: Scope, message: Message) -> Wire {
    Wire {
        nonce,
        owner: scope.owner,
        generation: scope.generation,
        message,
    }
}
fn check(w: &Wire, nonce: u128, scope: Scope) -> io::Result<()> {
    if w.nonce != nonce || w.owner != scope.owner || w.generation != scope.generation {
        return Err(io::Error::other("stale Metal session"));
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Init {
    schema: String,
    endpoint: String,
    nonce: u128,
    owner: u128,
    generation: u64,
    parent: u32,
    scene: String,
}
struct Offer {
    lease: Lease,
    right: Option<MachSendRight>,
    ready: bool,
    taken: bool,
}
/// Host-owned transport with two outstanding native frame slots. Drop kills and
/// reaps the private helper, retires the session and drops pending capabilities.
pub struct MetalProcess {
    child: Child,
    inbox: Inbox,
    reply: Option<MachSendRight>,
    nonce: u128,
    scope: Scope,
    size: FrameSize,
    last_revision: u64,
    offers: BTreeMap<u64, Offer>,
    responses: Vec<Message>,
    lost: bool,
}
impl MetalProcess {
    pub fn spawn(executable: &Path, scope: Scope, snapshot: &SceneSnapshot) -> io::Result<Self> {
        if !executable.is_absolute() {
            return Err(io::Error::other("helper path must be absolute"));
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
            child,
            inbox,
            reply: None,
            nonce,
            scope,
            size: snapshot.size(),
            last_revision: 0,
            offers: BTreeMap::new(),
            responses: Vec::new(),
            lost: false,
        };
        let init = Init {
            schema: SCHEMA.into(),
            endpoint,
            nonce,
            owner: scope.owner,
            generation: scope.generation,
            parent: std::process::id(),
            scene: String::from_utf8(snapshot.bytes().to_vec()).map_err(io::Error::other)?,
        };
        write_frame_with_limit(
            process.child.stdin.as_mut().unwrap(),
            &encode_with_limit(&init, LIMIT)?,
            LIMIT,
        )?;
        process.child.stdin.take();
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
                check(&packet, self.nonce, self.scope)?;
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
    pub fn scope(&self) -> Scope {
        self.scope
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
            &wire(self.nonce, self.scope, message),
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
        let _ = self.child.kill();
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
            check(&packet, self.nonce, self.scope)?;
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
                        || FrameSize::new(width, height) != self.size
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
            self.offers.clear();
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
                    size: self.size,
                    format: PixelFormat::Rgba8Unorm,
                    generation: lease.epoch,
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
        if stamp.scope != self.scope {
            return Err(io::Error::other("stale release scope"));
        }
        let offer = self
            .offers
            .get(&stamp.allocation_epoch)
            .filter(|o| o.taken && o.lease.stamp(self.scope) == stamp)
            .ok_or_else(|| io::Error::other("stale release"))?;
        let lease = offer.lease;
        self.send(Message::Release(lease))?;
        self.wait(|p| !p.responses.is_empty())?;
        match self.responses.remove(0) {
            Message::Released {
                lease: ack,
                occupied,
            } if lease.matches(ack) => {
                self.offers.remove(&lease.epoch);
                Ok(occupied)
            }
            other => Err(io::Error::other(format!("release refused: {other:?}"))),
        }
    }
    /// Fault injection for qualification: kill and reap the producer only.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        self.lost = true;
        self.offers.clear();
        Ok(())
    }
}
impl Drop for MetalProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Pending {
    lease: Lease,
    done: Option<Arc<AtomicBool>>,
    published: bool,
}
pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let init: Init =
        read_frame_with_limit(&mut io::stdin().lock(), LIMIT)?.ok_or("missing Metal job")?;
    if init.schema != SCHEMA {
        return Err("Metal schema mismatch".into());
    }
    let scope = Scope {
        owner: init.owner,
        generation: init.generation,
    };
    let snapshot = SceneSnapshot::from_bytes(init.scene.into_bytes(), RenderLimits::default())?;
    let size = snapshot.size();
    let viewport = Viewport {
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
    let cx = host.context(viewport)?;
    let native_host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
    let surfaces = [
        SharedMetalSurface::new(&native_host, size, PixelFormat::Rgba8Unorm)?,
        SharedMetalSurface::new(&native_host, size, PixelFormat::Rgba8Unorm)?,
    ];
    let mut source = HybridSceneSource::new(scope, &cx, RenderLimits::default())?;
    let warmup = Instant::now() + TIMEOUT;
    while !source.poll_idle()? {
        if Instant::now() >= warmup {
            return Err("Hybrid initialization deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    source.submit(&cx, 1, &snapshot)?;
    let deadline = Instant::now() + TIMEOUT;
    let rendered = loop {
        if let Rendered::Changed(frame) = source.render(&cx, true)? {
            break frame;
        }
        if Instant::now() >= deadline {
            return Err("Hybrid render deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let remote = lookup(&init.endpoint)?;
    let inbox = Inbox::new(None)?;
    // COPY_SEND duplicates a send right from our receive endpoint for the host.
    let reply = lookup_right_for_receive(inbox.port);
    send(
        remote.as_raw(),
        &wire(init.nonce, scope, Message::Hello { adapter: info.name }),
        Some(&reply),
    )?;
    let mut pool = FrameSlotPool::new(scope, 2)?;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut revision = 0;
    let mut last_activity = Instant::now();
    loop {
        cx.device.poll(wgpu::PollType::Poll)?;
        for state in pending.values_mut() {
            if !state.published
                && state
                    .done
                    .as_ref()
                    .is_some_and(|d| d.load(Ordering::Acquire))
            {
                pool.producer_done(state.lease.slot(scope))?;
                send(
                    remote.as_raw(),
                    &wire(init.nonce, scope, Message::Ready(state.lease)),
                    None,
                )?;
                state.published = true;
            }
        }
        if let Some((packet, right, pid)) = inbox.receive()? {
            last_activity = Instant::now();
            if pid != init.parent || right.is_some() {
                return Err("unexpected helper sender or capability".into());
            }
            check(&packet, init.nonce, scope)?;
            match packet.message {
                Message::Acquire { revision: next } => {
                    let lease = if next <= revision {
                        Err(Error::Stale)
                    } else {
                        pool.acquire()
                    };
                    match lease {
                        Ok(slot) => {
                            revision = next;
                            let lease = Lease {
                                slot: slot.slot,
                                epoch: slot.epoch,
                                revision,
                            };
                            let right = surfaces[lease.slot as usize].export_right()?;
                            send(
                                remote.as_raw(),
                                &wire(
                                    init.nonce,
                                    scope,
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
                                    done: None,
                                    published: false,
                                },
                            );
                        }
                        Err(error) => send(
                            remote.as_raw(),
                            &wire(init.nonce, scope, Message::Refused(error.to_string())),
                            None,
                        )?,
                    }
                }
                Message::Copy(lease) => {
                    let p = pending
                        .get_mut(&lease.epoch)
                        .filter(|p| p.lease.matches(lease) && p.done.is_none())
                        .ok_or("stale copy lease")?;
                    let mut encoder = cx.device.create_command_encoder(&Default::default());
                    encoder.copy_texture_to_texture(
                        rendered.texture.as_image_copy(),
                        surfaces[lease.slot as usize].texture().as_image_copy(),
                        rendered.texture.size(),
                    );
                    cx.queue.submit([encoder.finish()]);
                    let done = Arc::new(AtomicBool::new(false));
                    let callback = done.clone();
                    cx.queue
                        .on_submitted_work_done(move || callback.store(true, Ordering::Release));
                    p.done = Some(done);
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
                                    scope,
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
                            &wire(init.nonce, scope, Message::Refused(e.to_string())),
                            None,
                        )?,
                    }
                }
                Message::Shutdown => {
                    if pool.occupied() != 0 {
                        return Err("shutdown with outstanding frame leases".into());
                    }
                    pool.retire();
                    send(
                        remote.as_raw(),
                        &wire(init.nonce, scope, Message::Bye),
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
fn lookup_right_for_receive(port: u32) -> MachSendRight {
    // cue_receive_port created a receive right plus exactly one send reference.
    // Transfer only that send reference; Inbox independently owns receive right.
    unsafe { MachSendRight::from_raw(port) }
}
