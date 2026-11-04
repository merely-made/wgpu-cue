// SPDX-License-Identifier: MPL-2.0
//! Optional driver presentation feedback; absence never becomes a timing pass.
use super::*;
use ash::{google::display_timing, vk};
use std::cell::RefCell;
pub struct Timing {
    api: Option<display_timing::Device>,
    origin: u64,
    raw: RefCell<Vec<(u32, u64, u64, u64)>>,
}
fn now() -> AnyResult<u64> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
}
impl Timing {
    pub fn new(device: &wgpu::Device) -> AnyResult<Self> {
        let api = if device
            .features()
            .contains(wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING)
        {
            // SAFETY: immutable queries only; the host device and instance
            // outlive this extension loader. No Vulkan resource is destroyed.
            let native = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("Vulkan device required")?;
            Some(display_timing::Device::new(
                native.shared_instance().raw_instance(),
                native.raw_device(),
            ))
        } else {
            None
        };
        Ok(Self {
            api,
            origin: 0,
            raw: RefCell::new(vec![]),
        })
    }
    pub fn available(&self) -> bool {
        self.api.is_some()
    }
    pub fn begin(&mut self) -> AnyResult<()> {
        self.origin = now()?;
        Ok(())
    }
    pub fn prepare(&self, surface: &wgpu::Surface<'_>, id: u64) -> AnyResult<()> {
        if self.api.is_some() {
            let native = unsafe { surface.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("native Vulkan surface required")?;
            native.set_next_present_time(vk::PresentTimeGOOGLE {
                present_id: u32::try_from(id)?,
                desired_present_time: 0,
            });
        }
        Ok(())
    }
    pub fn drain(
        &self,
        surface: &wgpu::Surface<'_>,
        frames: &mut [metrics::Frame],
    ) -> AnyResult<()> {
        let Some(api) = &self.api else { return Ok(()) };
        let chain = unsafe { surface.as_hal::<wgpu::hal::api::Vulkan>() }
            .and_then(|s| s.raw_native_swapchain())
            .ok_or("native swapchain required")?;
        let samples = unsafe { api.get_past_presentation_timing(chain) }?;
        let mut raw = self.raw.borrow_mut();
        for sample in samples {
            if raw.len() >= 8000 {
                return Err("Vulkan feedback receipt bound exceeded".into());
            }
            let f = frames
                .iter_mut()
                .rev()
                .find(|f| f.id == u64::from(sample.present_id))
                .ok_or("uncorrelated Vulkan present ID")?;
            if f.os_present_s.is_some() {
                return Err("duplicate Vulkan feedback".into());
            }
            if sample.actual_present_time > 0 {
                f.os_present_s = Some(
                    (i128::from(sample.actual_present_time) - i128::from(self.origin)) as f64 / 1e9,
                );
            }
            raw.push((
                sample.present_id,
                sample.actual_present_time,
                sample.earliest_present_time,
                sample.present_margin,
            ));
        }
        Ok(())
    }
    pub fn evidence(&self) -> serde_json::Value {
        serde_json::json!({"basis":"VK_GOOGLE_display_timing actualPresentTime; unavailable when the device does not expose the feature","available":self.api.is_some(),"clock_monotonic_origin_ns":self.origin,"raw_fields":["present_id","actual_present_ns","earliest_present_ns","margin_ns"],"statistics":*self.raw.borrow()})
    }
}
