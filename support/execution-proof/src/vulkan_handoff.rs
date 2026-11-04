// SPDX-License-Identifier: MPL-2.0
//! Private Linux proof: persistent Hybrid renderer, owned SCM_RIGHTS DMA-BUFs,
//! kernel sender credentials, and two leased native slots. Not a public wire ABI.
use crate::render_wire::WireExecutionJob;
use crate::{Identity, encode_with_limit, read_frame_with_limit, write_frame_with_limit};
use grafting::vulkan_surface::SharedVulkanSurface;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
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
    DmaBufImage, DmaBufPlane, FrameMetadata, FrameSync, OwnedNativeFrame, OwnedNativeImage,
};
use webcue::{
    execution::*,
    frame_slots::{FrameSlotPool, SlotLease},
    *,
};
use wgpu_cue::*;
use wgpu_cue_netrender::resource_table::{ResourceReceiver, ResourceSender};
use wgpu_cue_netrender::{hybrid::HybridSceneSource, render_session::*, scene_job::*};
const LIMIT: usize = 12 * 1024 * 1024;
const CONTROL: usize = 4096;
const SCHEMA: &str = "cue-vulkan-dmabuf-v1";
const TIMEOUT: Duration = Duration::from_secs(30);
fn bad(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
fn backend(error: impl std::fmt::Display) -> Error {
    Error::Backend(error.to_string())
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
    parent: u32,
}
#[derive(Serialize, Deserialize)]
enum Message {
    Hello {
        adapter: String,
    },
    Ready {
        lease: Lease,
        width: u32,
        height: u32,
        offset: u64,
        stride: u64,
    },
    Release(Lease),
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
            return Err(bad("Vulkan packet identity/schema mismatch"));
        }
        Ok(())
    }
}
// Aligned ancillary storage. Parse all installed rights before rejecting a
// truncated or malformed packet, so refusal cannot leak descriptor custody.
struct Socket(OwnedFd);
impl Socket {
    fn pair() -> io::Result<(Self, Self)> {
        let mut fds = [-1; 2];
        if unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let a = Self(unsafe { OwnedFd::from_raw_fd(fds[0]) });
        let b = Self(unsafe { OwnedFd::from_raw_fd(fds[1]) });
        for socket in [&a, &b] {
            let on: libc::c_int = 1;
            if unsafe {
                libc::setsockopt(
                    socket.0.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&on as *const libc::c_int).cast(),
                    std::mem::size_of_val(&on) as _,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok((a, b))
    }
    fn send(&self, packet: &Packet, fd: Option<&OwnedFd>) -> io::Result<()> {
        let bytes = encode_with_limit(packet, CONTROL)?;
        let mut vector = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0usize; 16];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut vector;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            unsafe {
                msg.msg_control = control.as_mut_ptr().cast();
                msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as _) as _;
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as _) as _;
                std::ptr::write_unaligned(
                    libc::CMSG_DATA(cmsg).cast::<libc::c_int>(),
                    fd.as_raw_fd(),
                );
            }
        }
        let sent = unsafe {
            libc::sendmsg(
                self.0.as_raw_fd(),
                &msg,
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent as usize != bytes.len() {
            return Err(bad("short seqpacket send"));
        }
        Ok(())
    }
    fn receive(&self, expected: u32) -> io::Result<Option<(Packet, Option<OwnedFd>)>> {
        let mut bytes = [0u8; CONTROL];
        let mut control = [0usize; 32];
        let mut vector = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut vector;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&control);
        let count = unsafe {
            libc::recvmsg(
                self.0.as_raw_fd(),
                &mut msg,
                libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC,
            )
        };
        if count < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(e)
            };
        }
        let mut rights = Vec::new();
        let mut sender = None;
        let mut malformed = false;
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                let header = libc::CMSG_LEN(0) as usize;
                if (*cmsg).cmsg_len < header {
                    malformed = true;
                    break;
                }
                let length = (*cmsg).cmsg_len - header;
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    if !length.is_multiple_of(std::mem::size_of::<libc::c_int>()) {
                        malformed = true;
                    }
                    for index in 0..length / std::mem::size_of::<libc::c_int>() {
                        rights.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                            libc::CMSG_DATA(cmsg).cast::<libc::c_int>().add(index),
                        )));
                    }
                } else if (*cmsg).cmsg_level == libc::SOL_SOCKET
                    && (*cmsg).cmsg_type == libc::SCM_CREDENTIALS
                    && length == std::mem::size_of::<libc::ucred>()
                {
                    let credential =
                        std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::ucred>());
                    if sender.replace(credential.pid as u32).is_some()
                        || credential.uid != libc::geteuid()
                    {
                        malformed = true;
                    }
                } else {
                    malformed = true;
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Vulkan socket closed",
            ));
        }
        if malformed
            || msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
            || sender != Some(expected)
            || rights.len() > 1
        {
            return Err(bad("invalid/truncated Vulkan packet or sender credentials"));
        }
        let packet = serde_json::from_slice(&bytes[..count as usize]).map_err(bad)?;
        Ok(Some((packet, rights.pop())))
    }
}

/// Hardware-only Vulkan device with exact DMA-BUF extensions enabled. Both
/// local and helper Hybrid renderers use this same device construction path.
pub fn gpu() -> io::Result<(wgpu::Adapter, GpuHost, grafting::HostWgpuContext)> {
    let (_, adapter, host, native) = gpu_for_window(false)?;
    Ok((adapter, host, native))
}
pub fn gpu_with_instance() -> io::Result<(
    wgpu::Instance,
    wgpu::Adapter,
    GpuHost,
    grafting::HostWgpuContext,
)> {
    gpu_for_window(true)
}
fn gpu_for_window(
    presentation: bool,
) -> io::Result<(
    wgpu::Instance,
    wgpu::Adapter,
    GpuHost,
    grafting::HostWgpuContext,
)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .into_iter()
        .find(|adapter| {
            matches!(
                adapter.get_info().device_type,
                wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu
            )
        })
        .ok_or_else(|| bad("hardware Vulkan adapter required"))?;
    let info = adapter.get_info();
    eprintln!("Vulkan pid={} adapter={info:?}", std::process::id());
    let timing = wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING;
    let descriptor = wgpu::DeviceDescriptor {
        required_features: if presentation && adapter.features().contains(timing) {
            timing
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    };
    let native =
        grafting::vulkan_dmabuf::create_dmabuf_host_context(&adapter, &descriptor).map_err(bad)?;
    let host = GpuHost::new(native.device.clone(), native.queue.clone()).map_err(bad)?;
    Ok((instance, adapter, host, native))
}
struct Offered {
    lease: Lease,
    taken: bool,
}
pub struct VulkanProcess {
    resources: ResourceSender,
    child: Child,
    socket: Socket,
    execution: ExecutionId,
    nonce: u128,
    tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
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
impl VulkanProcess {
    pub fn spawn_execution(
        executable: &Path,
        execution: ExecutionId,
        snapshot: &SceneSnapshot,
    ) -> io::Result<Self> {
        if !executable.is_absolute() || execution.incarnation == 0 {
            return Err(bad("absolute helper path/nonzero incarnation required"));
        }
        let (socket, remote) = Socket::pair()?;
        let inherited = remote.0.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--vulkan-handoff")
            .env("CUE_VULKAN_FD", inherited.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(inherited, libc::F_SETFD, 0) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn()?;
        drop(remote);
        let mut process = Self {
            resources: ResourceSender::new(execution, RenderLimits::default())
                .map_err(io::Error::other)?,
            child,
            socket,
            execution,
            nonce: 0,
            tx: None,
            writer: None,
            failed: Arc::new(AtomicBool::new(false)),
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
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        process.nonce = u128::from_ne_bytes(random);
        let size = snapshot.size();
        let init = Init {
            schema: SCHEMA.into(),
            identity: execution.into(),
            nonce: process.nonce,
            width: size.width,
            height: size.height,
            parent: std::process::id(),
        };
        let mut input = process
            .child
            .stdin
            .take()
            .ok_or_else(|| bad("helper stdin"))?;
        write_frame_with_limit(&mut input, &encode_with_limit(&init, LIMIT)?, LIMIT)?;
        let (tx, jobs) = mpsc::sync_channel::<Vec<u8>>(2);
        process.tx = Some(tx);
        let failed = process.failed.clone();
        process.writer = Some(
            thread::Builder::new()
                .name("cue-vulkan-writer".into())
                .spawn(move || {
                    while let Ok(bytes) = jobs.recv() {
                        if write_frame_with_limit(&mut input, &bytes, LIMIT).is_err() {
                            failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                })?,
        );
        let deadline = Instant::now() + TIMEOUT;
        while process.adapter.is_none() {
            process.poll()?;
            if process.peer_closed {
                return Err(bad("Vulkan helper exited before startup"));
            }
            if Instant::now() >= deadline {
                return Err(bad("Vulkan helper startup deadline"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(process)
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
    pub fn adapter(&self) -> &str {
        self.adapter.as_deref().unwrap_or("")
    }
    pub fn lease(&self, stamp: FrameStamp) -> Option<Lease> {
        self.offered
            .get(&stamp.content_revision)
            .filter(|offer| offer.lease.stamp(self.execution.scope) == stamp)
            .map(|offer| offer.lease)
    }
    pub fn release(&mut self, stamp: FrameStamp) -> io::Result<()> {
        let offer = self
            .offered
            .get(&stamp.content_revision)
            .filter(|offer| offer.lease.stamp(self.execution.scope) == stamp && offer.taken)
            .ok_or_else(|| bad("unknown/untaken Vulkan release"))?;
        self.socket.send(
            &Packet::new(self.execution, self.nonce, Message::Release(offer.lease)),
            None,
        )
    }
    /// Fault injection: deliberately send a stale lease to a replacement.
    pub fn send_release(&self, lease: Lease) -> io::Result<()> {
        self.socket.send(
            &Packet::new(self.execution, self.nonce, Message::Release(lease)),
            None,
        )
    }
    pub fn released(&self) -> &[Lease] {
        &self.released
    }
    pub fn stale(&self) -> &[Lease] {
        &self.stale
    }
    pub fn poll(&mut self) -> io::Result<()> {
        loop {
            if self.peer_closed {
                break;
            }
            let received = match self.socket.receive(self.child.id()) {
                Ok(received) => received,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.peer_closed = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let Some((packet, fd)) = received else {
                break;
            };
            packet.check(self.execution, self.nonce)?;
            match packet.message {
                Message::Hello { adapter } if fd.is_none() && self.adapter.is_none() => {
                    self.adapter = Some(adapter)
                }
                Message::Ready {
                    lease,
                    width,
                    height,
                    offset,
                    stride,
                } => {
                    let fd = fd.ok_or_else(|| bad("missing DMA-BUF capability"))?;
                    let (stamp, size) = self
                        .submitted
                        .remove(&lease.revision)
                        .ok_or_else(|| bad("unsolicited/duplicate Vulkan frame"))?;
                    if stamp != lease.stamp(self.execution.scope)
                        || size != FrameSize::new(width, height)
                        || lease.slot >= 2
                        || lease.epoch == 0
                    {
                        return Err(bad("Vulkan frame differs from submitted job"));
                    }
                    let image = DmaBufImage::new(
                        vec![fd],
                        vec![DmaBufPlane {
                            buffer_index: 0,
                            offset,
                            stride,
                        }],
                        u32::from_le_bytes(*b"AB24"),
                        0,
                    )
                    .map_err(bad)?;
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
                            OwnedNativeImage::DmaBufImage(image),
                            FrameSync::None,
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
                Message::Released(lease) if fd.is_none() => {
                    let offer = self
                        .offered
                        .get(&lease.revision)
                        .filter(|o| o.lease == lease)
                        .ok_or_else(|| bad("unsolicited Vulkan release receipt"))?;
                    let _ = offer;
                    self.offered.remove(&lease.revision);
                    if self.released.len() == 64 {
                        self.released.remove(0);
                    }
                    self.released.push(lease);
                }
                Message::Stale(lease) if fd.is_none() => {
                    if self.stale.len() == 64 {
                        self.stale.remove(0);
                    }
                    self.stale.push(lease);
                }
                _ => return Err(bad("unexpected Vulkan message/capability")),
            }
        }
        Ok(())
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.tx.take();
        if self.forced {
            return Ok(());
        }
        if self.stopped.is_none() {
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
impl RenderTransport for VulkanProcess {
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
            return Err(backend("Vulkan job writer lost"));
        }
        let (wire, next_resources) =
            WireExecutionJob::prepare(request, &self.resources).map_err(backend)?;
        let bytes = encode_with_limit(&wire, LIMIT).map_err(backend)?;
        if self.submitted.len() >= 2 {
            return Ok(SendStatus::Busy);
        }
        match self.tx.as_ref().ok_or(Error::Retired)?.try_send(bytes) {
            Ok(()) => {
                if let Some(next) = next_resources {
                    self.resources = next;
                }
                self.submitted.insert(
                    request.job.stamp.content_revision,
                    (request.job.stamp, request.job.snapshot.size()),
                );
                Ok(SendStatus::Sent)
            }
            Err(mpsc::TrySendError::Full(_)) => Ok(SendStatus::Busy),
            Err(_) => Err(backend("Vulkan writer disconnected")),
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<Self::Frame>>> {
        if let Some(reply) = self.replies.pop_front() {
            if let Some(offer) = self.offered.get_mut(&reply.stamp.content_revision) {
                offer.taken = true;
            }
            return Ok(Some(reply));
        }
        self.poll().map_err(backend)?;
        if let Some(reply) = self.replies.pop_front() {
            if let Some(offer) = self.offered.get_mut(&reply.stamp.content_revision) {
                offer.taken = true;
            }
            return Ok(Some(reply));
        }
        if self.peer_closed || self.poll_stopped()?.is_some() {
            return Err(backend("Vulkan renderer exited"));
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
impl Drop for VulkanProcess {
    fn drop(&mut self) {
        let _ = self.kill();
        let _ = self.child.wait();
        self.tx.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
struct Pending {
    lease: Lease,
    job: Option<RenderJob>,
    rendered: Option<TextureFrame>,
    copying: bool,
    published: bool,
}
pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let inherited: i32 = std::env::var("CUE_VULKAN_FD")?.parse()?;
    if inherited < 3 {
        return Err("invalid inherited socket".into());
    }
    let socket = Socket(unsafe { OwnedFd::from_raw_fd(inherited) });
    // Restore close-on-exec immediately; only this explicitly passed fd was inherited.
    if unsafe { libc::fcntl(inherited, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let init: Init =
        read_frame_with_limit(&mut io::stdin().lock(), LIMIT)?.ok_or("missing Vulkan init")?;
    let execution = ExecutionId::from(init.identity);
    if init.schema != SCHEMA
        || execution.incarnation == 0
        || init.parent != unsafe { libc::getppid() } as u32
    {
        return Err("invalid Vulkan init".into());
    }
    let (adapter, host, native) = gpu()?;
    let mut viewport = Viewport {
        width: init.width,
        height: init.height,
        scale: 1.0,
    };
    let mut source = HybridSceneSource::new(
        execution.scope,
        &host.context(viewport)?,
        RenderLimits::default(),
    )?;
    let deadline = Instant::now() + TIMEOUT;
    while !source.poll_idle()? {
        if Instant::now() > deadline {
            return Err("Hybrid startup deadline".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let size = FrameSize::new(init.width, init.height);
    let mut surfaces = [
        SharedVulkanSurface::new(&native, size)?,
        SharedVulkanSurface::new(&native, size)?,
    ];
    socket.send(
        &Packet::new(
            execution,
            init.nonce,
            Message::Hello {
                adapter: adapter.get_info().name,
            },
        ),
        None,
    )?;
    let mut resources = ResourceReceiver::new(execution, RenderLimits::default())?;
    let (tx, jobs) = mpsc::sync_channel::<io::Result<RenderRequest>>(2);
    thread::Builder::new()
        .name("cue-vulkan-job-reader".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            loop {
                match read_frame_with_limit::<WireExecutionJob>(&mut input, LIMIT) {
                    Ok(Some(wire)) => {
                        if tx.send(wire.request_with(&mut resources)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        })?;
    let mut pool = FrameSlotPool::new(execution.scope, 2)?;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut waiting = None;
    let mut active = None;
    let mut revision = 0;
    let mut allocation = 0;
    let mut last_activity = Instant::now();
    loop {
        native.device.poll(wgpu::PollType::Poll)?;
        if let Some(epoch) = active
            && let Rendered::Changed(frame) = source.render(&host.context(viewport)?, true)?
        {
            pending
                .get_mut(&epoch)
                .ok_or("missing Vulkan render")?
                .rendered = Some(frame);
            active = None;
        }
        for state in pending.values_mut() {
            let surface = &mut surfaces[state.lease.slot as usize];
            if !state.copying
                && let Some(frame) = state.rendered.as_ref()
            {
                // SAFETY: Hybrid renders on this exact host device/queue.
                unsafe {
                    surface.copy_from(&frame.texture)?;
                }
                state.copying = true;
            }
            if state.copying && !state.published && surface.poll_ready()? {
                let fd = surface.export_fd()?;
                let size = surface.size();
                match socket.send(
                    &Packet::new(
                        execution,
                        init.nonce,
                        Message::Ready {
                            lease: state.lease,
                            width: size.width,
                            height: size.height,
                            offset: surface.offset(),
                            stride: surface.stride(),
                        },
                    ),
                    Some(&fd),
                ) {
                    Ok(()) => {
                        pool.producer_done(state.lease.slot(execution.scope))?;
                        state.published = true;
                        state.rendered = None;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if active.is_none()
            && let Some((&epoch, state)) = pending.iter_mut().find(|(_, state)| state.job.is_some())
        {
            let job = state.job.take().ok_or("missing Vulkan job")?;
            let size = job.snapshot.size();
            viewport = Viewport {
                width: size.width,
                height: size.height,
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
                Ok(job) => {
                    waiting = Some(job?);
                    last_activity = Instant::now();
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if pool.occupied() < 2
            && let Some(request) = waiting.take()
        {
            if request.execution != execution
                || request.job.stamp.scope != execution.scope
                || request.job.stamp.content_revision <= revision
                || request.job.stamp.allocation_epoch <= allocation
            {
                return Err("stale Vulkan job".into());
            }
            revision = request.job.stamp.content_revision;
            allocation = request.job.stamp.allocation_epoch;
            let slot = pool.acquire()?;
            let size = request.job.snapshot.size();
            if surfaces[slot.slot as usize].size() != size {
                surfaces[slot.slot as usize] = SharedVulkanSurface::new(&native, size)?;
            }
            let lease = Lease {
                slot: slot.slot,
                epoch: slot.epoch,
                revision,
                allocation: request.job.stamp.allocation_epoch,
            };
            pending.insert(
                lease.epoch,
                Pending {
                    lease,
                    job: Some(request.job),
                    rendered: None,
                    copying: false,
                    published: false,
                },
            );
        }
        if let Some((packet, fd)) = socket.receive(init.parent)? {
            last_activity = Instant::now();
            packet.check(execution, init.nonce)?;
            if fd.is_some() {
                return Err("unexpected helper capability".into());
            }
            let Message::Release(lease) = packet.message else {
                return Err("unexpected Vulkan helper message".into());
            };
            if pending
                .get(&lease.epoch)
                .is_some_and(|state| state.lease == lease && state.published)
            {
                pool.consumer_released(lease.slot(execution.scope))?;
                // SAFETY: trusted host's exact current lease acknowledges the
                // importer's completed ownership release and final reference drop.
                unsafe {
                    surfaces[lease.slot as usize].acknowledge_release()?;
                }
                pending.remove(&lease.epoch);
                socket.send(
                    &Packet::new(execution, init.nonce, Message::Released(lease)),
                    None,
                )?;
            } else {
                socket.send(
                    &Packet::new(execution, init.nonce, Message::Stale(lease)),
                    None,
                )?;
            }
        }
        if Instant::now().duration_since(last_activity) > TIMEOUT {
            return Err("Vulkan helper idle deadline".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> ExecutionId {
        ExecutionId {
            scope: Scope {
                owner: 71,
                generation: 1,
            },
            incarnation: 1,
        }
    }
    #[test]
    fn descriptors_arrive_owned_close_on_exec_and_bound_to_kernel_sender() {
        let (sender, receiver) = Socket::pair().unwrap();
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        sender
            .send(
                &Packet::new(
                    id(),
                    19,
                    Message::Hello {
                        adapter: "fixture".into(),
                    },
                ),
                Some(&read),
            )
            .unwrap();
        drop(read);
        let (packet, fd) = receiver.receive(std::process::id()).unwrap().unwrap();
        packet.check(id(), 19).unwrap();
        let fd = fd.unwrap();
        assert_ne!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let byte = [71u8];
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), byte.as_ptr().cast(), 1) },
            1
        );
        let mut got = [0u8];
        assert_eq!(
            unsafe { libc::read(fd.as_raw_fd(), got.as_mut_ptr().cast(), 1) },
            1
        );
        assert_eq!(got, byte);
        sender
            .send(
                &Packet::new(
                    id(),
                    19,
                    Message::Hello {
                        adapter: "fixture".into(),
                    },
                ),
                None,
            )
            .unwrap();
        assert!(receiver.receive(std::process::id() + 1).is_err());
        assert!(receiver.receive(std::process::id()).unwrap().is_none());
        assert!(
            packet
                .check(
                    ExecutionId {
                        incarnation: 2,
                        ..id()
                    },
                    19
                )
                .is_err()
        );
        assert!(packet.check(id(), 20).is_err());
    }
    #[test]
    fn oversized_control_is_refused_before_deserialization() {
        let (sender, receiver) = Socket::pair().unwrap();
        let bytes = vec![b'x'; CONTROL + 1];
        assert_eq!(
            unsafe {
                libc::send(
                    sender.0.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL,
                )
            },
            bytes.len() as isize
        );
        assert!(receiver.receive(std::process::id()).is_err());
    }
}
