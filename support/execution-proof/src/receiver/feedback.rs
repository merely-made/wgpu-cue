// SPDX-License-Identifier: MPL-2.0
use super::*;
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn cue_present_layer(view: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn cue_present_layer_release(layer: *mut std::ffi::c_void);
    fn cue_present_clock() -> f64;
    fn cue_present_last_id(layer: *mut std::ffi::c_void) -> u64;
    fn cue_present_drain(
        layer: *mut std::ffi::c_void,
        out: *mut Sample,
        capacity: u64,
        overflow: *mut u64,
    ) -> u64;
    fn cue_present_refresh(view: *mut std::ffi::c_void) -> u64;
}
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Sample {
    id: u64,
    time: f64,
}
pub struct Feedback {
    #[cfg(target_os = "linux")]
    timing: super::vulkan_feedback::Timing,
    #[cfg(target_os = "linux")]
    wayland: Option<super::wayland_feedback::Wayland>,
    #[cfg(windows)]
    dxgi: super::dxgi_feedback::Dxgi,
    #[cfg(target_os = "macos")]
    layer: *mut std::ffi::c_void,
    #[cfg(target_os = "macos")]
    origin: f64,
    pub refresh: f64,
}
impl Feedback {
    pub fn surface(
        instance: &wgpu::Instance,
        window: Arc<Window>,
        host: &GpuHost,
    ) -> AnyResult<(wgpu::Surface<'static>, Self)> {
        #[cfg(not(target_os = "linux"))]
        let _ = host;
        #[cfg(target_os = "macos")]
        {
            let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
                return Err("AppKit required".into());
            };
            // SAFETY: main-thread live winit NSView; native layer is retained
            // independently by Feedback and Surface. No Rust callbacks exist.
            let layer = unsafe { cue_present_layer(handle.ns_view.as_ptr()) };
            if layer.is_null() {
                return Err("Could not attach observed CAMetalLayer".into());
            }
            let feedback = Self {
                layer,
                origin: unsafe { cue_present_clock() },
                refresh: unsafe { cue_present_refresh(handle.ns_view.as_ptr()) } as f64,
            };
            let surface = unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
            }?;
            Ok((surface, feedback))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let refresh = window
                .current_monitor()
                .and_then(|m| m.refresh_rate_millihertz())
                .unwrap_or(60_000) as f64
                / 1000.;
            #[cfg(target_os = "linux")]
            let timing =
                super::vulkan_feedback::Timing::new(host.context(viewport_for(1, 1))?.device)?;
            #[cfg(target_os = "linux")]
            let wayland = if timing.available() {
                None
            } else {
                super::wayland_feedback::Wayland::new(&window)?
            };
            Ok((
                instance.create_surface(window)?,
                Self {
                    refresh,
                    #[cfg(target_os = "linux")]
                    timing,
                    #[cfg(target_os = "linux")]
                    wayland,
                    #[cfg(windows)]
                    dxgi: super::dxgi_feedback::Dxgi::new()?,
                },
            ))
        }
    }
    pub fn begin(&mut self) -> AnyResult<()> {
        #[cfg(target_os = "macos")]
        {
            self.origin = unsafe { cue_present_clock() };
        }
        #[cfg(windows)]
        self.dxgi.begin()?;
        #[cfg(target_os = "linux")]
        {
            self.timing.begin()?;
            if let Some(w) = &self.wayland {
                w.begin()?;
            }
        }
        Ok(())
    }
    pub fn resized(&mut self) {
        #[cfg(windows)]
        self.dxgi.resized();
    }
    pub fn evidence(&self) -> serde_json::Value {
        #[cfg(windows)]
        {
            self.dxgi.evidence()
        }
        #[cfg(target_os = "macos")]
        {
            serde_json::json!({"basis":"MTLDrawable addPresentedHandler and presentedTime"})
        }
        #[cfg(target_os = "linux")]
        {
            self.wayland
                .as_ref()
                .map_or_else(|| self.timing.evidence(), |w| w.evidence())
        }
    }
    pub fn prepare(&self, surface: &wgpu::Surface<'_>, id: u64) -> AnyResult<()> {
        #[cfg(target_os = "linux")]
        {
            if let Some(w) = &self.wayland {
                w.prepare(id)?;
            } else {
                self.timing.prepare(surface, id)?;
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (surface, id);
        Ok(())
    }
    pub fn id(&self, surface: &wgpu::Surface<'_>, fallback: u64) -> AnyResult<u64> {
        #[cfg(target_os = "macos")]
        {
            let _ = (surface, fallback);
            Ok(unsafe { cue_present_last_id(self.layer) })
        }
        #[cfg(windows)]
        {
            let _ = fallback;
            self.dxgi.id(surface)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = surface;
            Ok(fallback)
        }
    }
    pub fn drain(
        &self,
        surface: &wgpu::Surface<'_>,
        frames: &mut [metrics::Frame],
    ) -> AnyResult<()> {
        #[cfg(target_os = "macos")]
        {
            let mut samples = [Sample::default(); 8192];
            let mut overflow = 0;
            let count =
                unsafe { cue_present_drain(self.layer, samples.as_mut_ptr(), 8192, &mut overflow) };
            if overflow != 0 {
                return Err("Presentation feedback overflow".into());
            }
            for s in &samples[..count as usize] {
                let f = frames
                    .iter_mut()
                    .rev()
                    .find(|f| f.id == s.id)
                    .ok_or("Uncorrelated drawable feedback")?;
                if f.os_present_s.is_some() {
                    return Err("Duplicate drawable feedback".into());
                }
                if s.time > 0. {
                    f.os_present_s = Some(s.time - self.origin)
                }
            }
        }
        #[cfg(windows)]
        self.dxgi.drain(surface, frames, self.refresh)?;
        #[cfg(target_os = "linux")]
        {
            if let Some(w) = &self.wayland {
                w.drain(frames)?;
            } else {
                self.timing.drain(surface, frames)?;
            }
        }
        #[cfg(target_os = "macos")]
        let _ = surface;
        Ok(())
    }
}
#[cfg(target_os = "macos")]
impl Drop for Feedback {
    fn drop(&mut self) {
        unsafe { cue_present_layer_release(self.layer) }
    }
}
