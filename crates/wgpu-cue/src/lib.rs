// SPDX-License-Identifier: MPL-2.0
//! Both presentation directions, with renderer-free controls re-exported.
#![forbid(unsafe_code)]
use std::sync::atomic::{AtomicU64, Ordering};
pub use webcue;
use webcue::*;
pub use wgpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceEpoch(u64);
/// Owns the identity of a device lifetime. Rebuilding a device means a new host.
/// Queue and device must belong together; implementations never manufacture them.
pub struct GpuHost {
    device: wgpu::Device,
    queue: wgpu::Queue,
    epoch: DeviceEpoch,
}
impl GpuHost {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        #[allow(deprecated)] // MSRV-compatible spelling.
        let epoch = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| x.checked_add(1))
            .map_err(|_| Error::Exhausted)?;
        Ok(Self {
            device,
            queue,
            epoch: DeviceEpoch(epoch),
        })
    }
    pub fn context(&self, viewport: Viewport) -> Result<RenderContext<'_>> {
        let viewport = viewport.validate()?;
        let limit = self.device.limits().max_texture_dimension_2d;
        if viewport.width > limit || viewport.height > limit {
            return Err(Error::Invalid(
                "viewport exceeds the device texture limit".into(),
            ));
        }
        Ok(RenderContext {
            device: &self.device,
            queue: &self.queue,
            device_epoch: self.epoch,
            viewport: viewport.validate()?,
        })
    }
    pub fn epoch(&self) -> DeviceEpoch {
        self.epoch
    }
}
pub struct RenderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub device_epoch: DeviceEpoch,
    pub viewport: Viewport,
}
pub struct TextureFrame {
    pub texture: wgpu::Texture,
    pub stamp: FrameStamp,
    pub device_epoch: DeviceEpoch,
    pub alpha: Alpha,
    pub encoding: Encoding,
}
pub enum Rendered {
    Changed(TextureFrame),
    Unchanged,
    Pending,
}
/// Renderer adapters can retain private renderer/core state; that state is not
/// required of other implementations. Render submits work and may poll without waiting
/// for GPU completion. Ordinary event and semantic reads do not trigger rendering.
pub trait TextureSource: Surface {
    fn render(&mut self, context: &RenderContext<'_>, needs_frame: bool) -> Result<Rendered>;
}
pub enum Imported {
    Ready(TextureFrame),
    Pending(NativeFrame),
}
/// Imports and synchronization remain with Graft/platform adapters. Pending must
/// return the entire move-owned frame; no consumed handle can be retried.
pub trait NativeImporter {
    fn try_import(&mut self, context: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported>;
}
pub enum Source<'a> {
    Native(&'a mut dyn FrameSource),
    HostDevice(&'a mut dyn TextureSource),
}
impl Source<'_> {
    fn scope(&self) -> Scope {
        match self {
            Self::Native(s) => s.scope(),
            Self::HostDevice(s) => s.scope(),
        }
    }
    fn mode(&self) -> PresentationMode {
        match self {
            Self::Native(s) => s.capabilities().presentation,
            Self::HostDevice(s) => s.capabilities().presentation,
        }
    }
}
pub struct Presented<'a> {
    pub view: wgpu::TextureView,
    pub texture: &'a wgpu::Texture,
    pub stamp: FrameStamp,
    pub alpha: Alpha,
    pub encoding: Encoding,
}
#[derive(Default)]
pub struct Presenter {
    current: Option<TextureFrame>,
    pending: Option<NativeFrame>,
    last: Option<FrameStamp>,
    pending_epoch: Option<DeviceEpoch>,
}
impl Presenter {
    pub fn retire(&mut self) {
        self.current = None;
        self.pending = None;
        self.last = None;
        self.pending_epoch = None;
    }
    pub fn present<'a>(
        &'a mut self,
        host: &GpuHost,
        mut source: Source<'_>,
        viewport: Viewport,
        importer: &mut dyn NativeImporter,
    ) -> Result<Option<Presented<'a>>> {
        let context = host.context(viewport)?;
        let scope = source.scope();
        if self
            .pending_epoch
            .is_some_and(|epoch| epoch != host.epoch())
        {
            self.pending = None;
            self.pending_epoch = None;
        }
        if self.last.is_some_and(|last| last.scope != scope) {
            self.retire();
        }
        if self.current.as_ref().is_some_and(|f| {
            f.stamp.scope != scope
                || f.device_epoch != host.epoch()
                || f.texture.width() != viewport.width
                || f.texture.height() != viewport.height
        }) {
            self.current = None;
            self.pending = None;
        }
        if self.pending.as_ref().is_some_and(|f| {
            f.stamp.scope != scope
                || f.frame.metadata().size != FrameSize::new(viewport.width, viewport.height)
        }) {
            self.pending = None;
        }
        let needs_frame = self.current.is_none();
        let frame = match &mut source {
            Source::HostDevice(s) => {
                if s.capabilities().presentation != PresentationMode::HostDevice {
                    return Err(Error::Invalid("host-device capability mismatch".into()));
                }
                match s.render(&context, needs_frame)? {
                    Rendered::Changed(f) => Some(f),
                    Rendered::Pending => None,
                    Rendered::Unchanged if needs_frame => {
                        return Err(Error::Invalid("unchanged without a reusable frame".into()));
                    }
                    Rendered::Unchanged => None,
                }
            }
            Source::Native(s) => {
                if !matches!(
                    s.capabilities().presentation,
                    PresentationMode::NativeFrame | PresentationMode::CpuRgba
                ) {
                    return Err(Error::Unsupported(
                        "presentation is not a compositable frame".into(),
                    ));
                }
                let next = match s.try_acquire_frame()? {
                    Some(frame) => {
                        self.pending = None;
                        Some(frame)
                    }
                    None => self.pending.take().map(Frame::Native),
                };
                match next {
                    Some(Frame::Native(f)) => {
                        if source.mode() != PresentationMode::NativeFrame {
                            return Err(Error::Invalid("unadvertised native frame".into()));
                        }
                        if f.stamp.scope != scope {
                            return Err(Error::Stale);
                        }
                        if f.frame.metadata().size
                            != FrameSize::new(viewport.width, viewport.height)
                        {
                            return Err(Error::Invalid(
                                "native extent differs from viewport".into(),
                            ));
                        }
                        match importer.try_import(&context, f)? {
                            Imported::Ready(f) => Some(f),
                            Imported::Pending(f) => {
                                self.pending = Some(f);
                                self.pending_epoch = Some(host.epoch());
                                None
                            }
                        }
                    }
                    Some(Frame::Cpu(f)) => {
                        if source.mode() != PresentationMode::CpuRgba {
                            return Err(Error::Invalid("unadvertised CPU frame".into()));
                        }
                        if f.stamp.scope != scope {
                            return Err(Error::Stale);
                        }
                        if f.size != FrameSize::new(viewport.width, viewport.height) {
                            return Err(Error::Invalid("CPU extent differs from viewport".into()));
                        }
                        Some(upload_cpu(&context, f)?)
                    }
                    Some(Frame::Overlay) => {
                        return Err(Error::Unsupported(
                            "native overlays require a native host".into(),
                        ));
                    }
                    None => None,
                }
            }
        };
        if let Some(frame) = frame {
            validate_texture(&context, scope, &frame)?;
            if let Some(last) = self.last {
                if frame.stamp.content_revision <= last.content_revision
                    || frame.stamp.allocation_epoch < last.allocation_epoch
                {
                    return Err(Error::Stale);
                }
                if self.current.is_none() && frame.stamp.allocation_epoch <= last.allocation_epoch {
                    return Err(Error::Invalid(
                        "replacement after invalidation must advance allocation epoch".into(),
                    ));
                }
            }
            if let Some(old) = &self.current {
                if frame.stamp.content_revision <= old.stamp.content_revision {
                    return Err(Error::Stale);
                }
                if frame.texture != old.texture
                    && frame.stamp.allocation_epoch <= old.stamp.allocation_epoch
                {
                    return Err(Error::Invalid(
                        "replacement texture must advance allocation epoch".into(),
                    ));
                }
            }
            self.last = Some(frame.stamp);
            self.current = Some(frame);
        }
        Ok(self.current.as_ref().map(|f| Presented {
            view: f.texture.create_view(&Default::default()),
            texture: &f.texture,
            stamp: f.stamp,
            alpha: f.alpha,
            encoding: f.encoding,
        }))
    }
}
fn validate_texture(cx: &RenderContext<'_>, scope: Scope, f: &TextureFrame) -> Result<()> {
    if f.stamp.scope != scope || f.device_epoch != cx.device_epoch {
        return Err(Error::Stale);
    }
    if f.texture.dimension() != wgpu::TextureDimension::D2
        || f.texture.depth_or_array_layers() != 1
        || f.texture.sample_count() != 1
        || f.texture.width() != cx.viewport.width
        || f.texture.height() != cx.viewport.height
        || !f
            .texture
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
    {
        return Err(Error::Invalid(
            "texture must be a sampleable single-layer target matching the viewport".into(),
        ));
    }
    Ok(())
}
fn upload_cpu(cx: &RenderContext<'_>, f: CpuFrame) -> Result<TextureFrame> {
    f.validate()?;
    let texture = cx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("webcue CPU frame"),
        size: wgpu::Extent3d {
            width: f.size.width,
            height: f.size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    cx.queue.write_texture(
        texture.as_image_copy(),
        &f.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(f.stride),
            rows_per_image: Some(f.size.height),
        },
        texture.size(),
    );
    Ok(TextureFrame {
        texture,
        stamp: f.stamp,
        device_epoch: cx.device_epoch,
        alpha: f.alpha,
        encoding: f.encoding,
    })
}

#[cfg(target_os = "macos")]
mod metal;
#[cfg(target_os = "macos")]
pub use metal::GraftMetalImporter;

#[cfg(target_os = "linux")]
mod vulkan;
#[cfg(target_os = "linux")]
pub use vulkan::GraftVulkanImporter;
