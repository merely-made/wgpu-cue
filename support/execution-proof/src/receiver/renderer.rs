// SPDX-License-Identifier: MPL-2.0
use super::*;
#[cfg(target_os = "windows")]
use crate::dx12_handoff::Dx12Process as Process;
#[cfg(target_os = "macos")]
use crate::metal_handoff::MetalProcess as Process;
#[cfg(target_os = "linux")]
use crate::vulkan_handoff::VulkanProcess as Process;
use wgpu_cue_netrender::{hybrid::HybridSceneSource, render_session::*, scene_job::*};
#[cfg(target_os = "macos")]
type Importer = GraftIoSurfaceImporter;
#[cfg(target_os = "linux")]
type Importer = GraftVulkanSurfaceImporter;
#[cfg(target_os = "windows")]
type Importer = GraftDx12SurfaceImporter;
pub enum Renderer {
    Local(Box<HybridSceneSource>),
    Process {
        session: Box<RenderSession<Process>>,
        importer: Importer,
    },
}
pub fn gpu() -> AnyResult<(wgpu::Instance, wgpu::Adapter, GpuHost)> {
    #[cfg(target_os = "macos")]
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        let (d, q) = pollster::block_on(adapter.request_device(&Default::default()))?;
        Ok((instance, adapter, GpuHost::new(d, q)?))
    }
    #[cfg(target_os = "linux")]
    {
        let (i, a, h, _) = crate::vulkan_handoff::gpu_with_instance()?;
        Ok((i, a, h))
    }
    #[cfg(target_os = "windows")]
    {
        let (i, a, h, _) = crate::dx12_handoff::gpu_with_instance()?;
        Ok((i, a, h))
    }
}
impl Renderer {
    pub fn new(
        local: bool,
        helper: &std::path::Path,
        host: &GpuHost,
        viewport: Viewport,
        snapshot: &SceneSnapshot,
    ) -> AnyResult<Self> {
        if local {
            return Ok(Self::Local(Box::new(HybridSceneSource::new(
                scope(),
                &host.context(viewport)?,
                RenderLimits::default(),
            )?)));
        }
        let execution = webcue::execution::ExecutionId {
            scope: scope(),
            incarnation: 1,
        };
        #[cfg(not(target_os = "windows"))]
        let process = Process::spawn_execution(helper, execution, snapshot)?;
        #[cfg(target_os = "windows")]
        let process = {
            let cx = host.context(viewport)?;
            let native = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
            Process::spawn_execution(
                helper,
                execution,
                snapshot,
                grafting::dx12_surface::adapter_luid(&native)?,
            )?
        };
        eprintln!(
            "receiving host={} helper={}",
            std::process::id(),
            process.pid()
        );
        Ok(Self::Process {
            session: Box::new(RenderSession::new(
                execution,
                process,
                2,
                64 * 1024 * 1024,
                RenderLimits::default(),
            )?),
            importer: Importer::new(scope(), host.epoch()),
        })
    }
    pub fn pending(&self) -> bool {
        match self {
            Self::Local(s) => s.in_flight(),
            Self::Process { session, .. } => session.pending() > 0,
        }
    }
    pub fn submit(&mut self, host: &GpuHost, viewport: Viewport, job: RenderJob) -> AnyResult<()> {
        match self {
            Self::Local(s) => {
                s.set_viewport(viewport)?;
                s.submit(
                    &host.context(viewport)?,
                    job.stamp.content_revision,
                    &job.snapshot,
                )?;
            }
            Self::Process { session, .. } => {
                session.submit(job)?;
            }
        }
        Ok(())
    }
    pub fn poll(
        &mut self,
        host: &GpuHost,
        viewport: Viewport,
        held: &[FrameStamp],
    ) -> AnyResult<Option<TextureFrame>> {
        match self {
            Self::Local(s) => {
                if let Rendered::Changed(f) = s.render(&host.context(viewport)?, true)? {
                    Ok(Some(f))
                } else {
                    Ok(None)
                }
            }
            Self::Process { session, importer } => {
                for stamp in importer.poll_releases()? {
                    if held.contains(&stamp) {
                        return Err("frame slot released while retained view lives".into());
                    }
                    session.transport_mut().release(stamp)?;
                }
                session.advance()?;
                if let Some(reply) = session.poll_reply() {
                    let frame = reply.result.map_err(|e| format!("renderer reply: {e:?}"))?;
                    let size = frame.frame.metadata().size;
                    match importer
                        .try_import(&host.context(viewport_for(size.width, size.height))?, frame)?
                    {
                        Imported::Ready(f) => Ok(Some(f)),
                        Imported::Pending(_) => Err("ready process frame imported pending".into()),
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }
    pub fn retire_frame(&mut self, stamp: FrameStamp) -> AnyResult<()> {
        if let Self::Process { importer, .. } = self {
            importer.seal_use(stamp)?;
        }
        Ok(())
    }
    pub fn imports(&self) -> usize {
        match self {
            Self::Process { importer, .. } => importer.in_flight(),
            _ => 0,
        }
    }
    pub fn stop(&mut self) -> AnyResult<()> {
        match self {
            Self::Local(s) => s.retire()?,
            Self::Process { session, .. } => session.retire()?,
        };
        Ok(())
    }
}

pub fn validate(host: &GpuHost, viewport: Viewport) -> AnyResult<()> {
    #[cfg(windows)]
    {
        let cx = host.context(viewport)?;
        crate::dx12_handoff::check_validation(&grafting::HostWgpuContext::new(
            cx.device.clone(),
            cx.queue.clone(),
        ))?;
    }
    #[cfg(not(windows))]
    let _ = (host, viewport);
    Ok(())
}
