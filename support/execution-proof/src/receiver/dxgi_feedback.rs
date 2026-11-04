// SPDX-License-Identifier: MPL-2.0
//! Preserve DXGI display counters. QPC timestamps below are reconstructed from
//! the reported refresh counts, not treated as Metal-style drawable timestamps.
use super::*;
use serde::Serialize;
use std::cell::{Cell, RefCell};
use windows::Win32::{
    Graphics::Dxgi::{DXGI_FRAME_STATISTICS, IDXGISwapChain3},
    System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
};
#[derive(Serialize)]
struct Sample {
    epoch: u64,
    present: u32,
    display_refresh: u32,
    sync_refresh: u32,
    sync_qpc: i64,
}
pub struct Dxgi {
    origin: i64,
    frequency: i64,
    epoch: u64,
    last: Cell<(u64, u32)>,
    errors: Cell<u64>,
    samples: RefCell<Vec<Sample>>,
}
fn chain(surface: &wgpu::Surface<'_>) -> AnyResult<IDXGISwapChain3> {
    // SAFETY: only clone an owned COM handle. Never mutate or destroy wgpu's
    // swapchain, and release the hal guard before querying DXGI statistics.
    unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
        .and_then(|s| s.swap_chain())
        .ok_or_else(|| "DX12 swapchain unavailable".into())
}
impl Dxgi {
    pub fn new() -> AnyResult<Self> {
        let mut frequency = 0;
        unsafe { QueryPerformanceFrequency(&mut frequency)? };
        if frequency <= 0 {
            return Err("invalid QPC frequency".into());
        }
        Ok(Self {
            origin: 0,
            frequency,
            epoch: 1,
            last: Cell::new((0, 0)),
            errors: Cell::new(0),
            samples: RefCell::new(vec![]),
        })
    }
    pub fn begin(&mut self) -> AnyResult<()> {
        unsafe { QueryPerformanceCounter(&mut self.origin)? };
        Ok(())
    }
    pub fn resized(&mut self) {
        self.epoch += 1;
    }
    pub fn id(&self, surface: &wgpu::Surface<'_>) -> AnyResult<u64> {
        let id = unsafe { chain(surface)?.GetLastPresentCount()? };
        Ok((self.epoch << 32) | u64::from(id))
    }
    pub fn drain(
        &self,
        surface: &wgpu::Surface<'_>,
        frames: &mut [metrics::Frame],
        refresh: f64,
    ) -> AnyResult<()> {
        let mut stats = DXGI_FRAME_STATISTICS::default();
        if unsafe { chain(surface)?.GetFrameStatistics(&mut stats) }.is_err() {
            self.errors.set(self.errors.get() + 1);
            return Ok(());
        }
        if stats.PresentCount == 0 || self.last.get() == (self.epoch, stats.PresentCount) {
            return Ok(());
        }
        self.last.set((self.epoch, stats.PresentCount));
        let id = (self.epoch << 32) | u64::from(stats.PresentCount);
        if let Some(frame) = frames.iter_mut().rev().find(|f| f.id == id) {
            let ticks = stats
                .PresentRefreshCount
                .wrapping_sub(stats.SyncRefreshCount) as i32;
            // Wrap-safe delta, bounded to one second; discontinuities are
            // retained as unavailable rather than manufacturing a timestamp.
            if f64::from(ticks.abs()) <= refresh && stats.SyncQPCTime > 0 {
                frame.os_present_s = Some(
                    (stats.SyncQPCTime - self.origin) as f64 / self.frequency as f64
                        + f64::from(ticks) / refresh,
                );
            }
        }
        let mut samples = self.samples.borrow_mut();
        if samples.len() >= 8000 {
            return Err("DXGI feedback receipt limit".into());
        }
        samples.push(Sample {
            epoch: self.epoch,
            present: stats.PresentCount,
            display_refresh: stats.PresentRefreshCount,
            sync_refresh: stats.SyncRefreshCount,
            sync_qpc: stats.SyncQPCTime,
        });
        Ok(())
    }
    pub fn evidence(&self) -> serde_json::Value {
        let samples = self.samples.borrow();
        let mut unobserved_presents = 0u64;
        let mut late_refreshes = 0u64;
        for pair in samples.windows(2) {
            if pair[0].epoch != pair[1].epoch {
                continue;
            }
            let calls = pair[1].present.wrapping_sub(pair[0].present);
            let refreshes = pair[1]
                .display_refresh
                .wrapping_sub(pair[0].display_refresh);
            unobserved_presents += u64::from(calls.saturating_sub(1));
            late_refreshes += u64::from(refreshes.saturating_sub(calls));
        }

        serde_json::json!({"basis":"DXGI PresentCount and PresentRefreshCount; timestamps reconstructed from SyncQPCTime and display refresh","qpc_origin":self.origin,"qpc_frequency":self.frequency,"unavailable_statistics_polls":self.errors.get(),"unobserved_present_ids":unobserved_presents,"estimated_late_display_refreshes_from_counters":late_refreshes,"sampling_note":"Observed timestamp intervals can include polling gaps. Raw display counters distinguish these from late refreshes; incomplete coverage never passes the pacing gate.","statistics":*self.samples.borrow()})
    }
}
