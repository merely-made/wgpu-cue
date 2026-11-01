// SPDX-License-Identifier: MPL-2.0
//! Private one-job raster helper protocol. Scene bytes use the exact locked
//! Netrender schema; this fixture is not a public IPC standard or sandbox.
use crate::{encode_with_limit, read_frame_with_limit, write_frame_with_limit};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use webcue::*;
use wgpu_cue_netrender::cpu::*;
const LIMIT: usize = 128 * 1024 * 1024;
const SCHEMA: &str = "netrender-66b2f72e-cpu-v1";

#[derive(Serialize, Deserialize)]
struct WireJob {
    schema: String,
    owner: u128,
    generation: u64,
    allocation: u64,
    revision: u64,
    scene: String,
}
impl WireJob {
    fn from_job(job: &RenderJob) -> io::Result<Self> {
        Ok(Self {
            schema: SCHEMA.into(),
            owner: job.stamp.scope.owner,
            generation: job.stamp.scope.generation,
            allocation: job.stamp.allocation_epoch,
            revision: job.stamp.content_revision,
            scene: String::from_utf8(job.snapshot.bytes().to_vec()).map_err(io::Error::other)?,
        })
    }
    fn job(self) -> io::Result<RenderJob> {
        if self.schema != SCHEMA {
            return Err(io::Error::other("raster schema mismatch"));
        }
        Ok(RenderJob {
            stamp: FrameStamp {
                scope: Scope {
                    owner: self.owner,
                    generation: self.generation,
                },
                allocation_epoch: self.allocation,
                content_revision: self.revision,
            },
            snapshot: SceneSnapshot::from_bytes(self.scene.into_bytes(), RenderLimits::default())
                .map_err(io::Error::other)?,
        })
    }
}
#[derive(Serialize, Deserialize)]
enum Pixels {
    PremultipliedSrgb,
}
#[derive(Serialize, Deserialize)]
enum WireReply {
    Frame {
        owner: u128,
        generation: u64,
        allocation: u64,
        revision: u64,
        width: u32,
        height: u32,
        stride: u32,
        convention: Pixels,
        rgba: Vec<u8>,
    },
    Refused(String),
}
impl WireReply {
    fn frame(self) -> io::Result<CpuFrame> {
        if let Self::Refused(reason) = self {
            return Err(io::Error::other(reason));
        }
        let Self::Frame {
            owner,
            generation,
            allocation,
            revision,
            width,
            height,
            stride,
            convention: Pixels::PremultipliedSrgb,
            rgba,
        } = self
        else {
            return Err(io::Error::other("raster helper refused job"));
        };
        let frame = CpuFrame {
            stamp: FrameStamp {
                scope: Scope { owner, generation },
                allocation_epoch: allocation,
                content_revision: revision,
            },
            size: FrameSize::new(width, height),
            stride,
            rgba,
            alpha: Alpha::Premultiplied,
            encoding: Encoding::Srgb,
        };
        frame.validate().map_err(io::Error::other)?;
        if frame.rgba.len() > RenderLimits::default().frame_bytes {
            return Err(io::Error::other("raster pixel budget"));
        }
        Ok(frame)
    }
}

pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let Some(job) = read_frame_with_limit::<WireJob>(&mut input, LIMIT)? else {
        return Ok(());
    };
    let reply = match job
        .job()
        .and_then(|job| rasterize(&job, RenderLimits::default()).map_err(io::Error::other))
    {
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
        Err(error) => WireReply::Refused(error.to_string()),
    };
    write_frame_with_limit(
        &mut io::stdout().lock(),
        &encode_with_limit(&reply, LIMIT)?,
        LIMIT,
    )?;
    Ok(())
}

struct OwnedHelper {
    child: Child,
    writer: Option<JoinHandle<io::Result<()>>>,
    reader: Option<JoinHandle<()>>,
}
impl Drop for OwnedHelper {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
/// Blocking test driver with a response/exit deadline and unconditional helper
/// custody. Product frame acquisition never invokes this fixture method.
pub fn in_process(executable: &Path, job: &RenderJob) -> io::Result<CpuFrame> {
    let bytes = encode_with_limit(&WireJob::from_job(job)?, LIMIT)?;
    let child = Command::new(executable)
        .arg("--raster")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut owned = OwnedHelper {
        child,
        writer: None,
        reader: None,
    };
    let mut input = owned
        .child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("helper stdin"))?;
    let mut output = owned
        .child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("helper stdout"))?;
    owned.writer = Some(
        thread::Builder::new()
            .name("cue-raster-input".into())
            .spawn(move || write_frame_with_limit(&mut input, &bytes, LIMIT))?,
    );
    let (tx, rx) = mpsc::sync_channel(1);
    owned.reader = Some(
        thread::Builder::new()
            .name("cue-raster-output".into())
            .spawn(move || {
                let _ = tx.send(read_frame_with_limit::<WireReply>(&mut output, LIMIT));
            })?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let reply = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(io::Error::other)??
        .ok_or_else(|| io::Error::other("helper exited without a frame"))?;
    loop {
        if let Some(status) = owned.child.try_wait()? {
            if !status.success() {
                return Err(io::Error::other("raster helper failed"));
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("raster helper exit deadline"));
        }
        thread::sleep(Duration::from_millis(1));
    }
    owned
        .writer
        .take()
        .unwrap()
        .join()
        .map_err(|_| io::Error::other("raster writer panicked"))??;
    reply.frame()
}
