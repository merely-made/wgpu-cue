// SPDX-License-Identifier: MPL-2.0
//! Observe commits on the host-owned surface through an independent event
//! queue. Winit continues to own the display, surface and socket read loop.
use super::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use serde::Serialize;
use std::{cell::RefCell, collections::BTreeMap};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    protocol::{wl_registry, wl_surface},
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
#[derive(Serialize)]
struct Sample {
    id: u64,
    time_ns: Option<u64>,
    refresh_ns: u32,
    sequence: u64,
    flags: String,
}
#[derive(Default)]
struct State {
    presentation: Option<wp_presentation::WpPresentation>,
    clock: Option<libc::clockid_t>,
    pending: BTreeMap<u64, wp_presentation_feedback::WpPresentationFeedback>,
    received: Vec<Sample>,
    raw: Vec<Sample>,
    error: Option<String>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
            && interface == "wp_presentation"
            && state.presentation.is_none()
        {
            state.presentation = Some(registry.bind(name, 1, qh, ()));
        }
    }
}
impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock = Some(clk_id as libc::clockid_t);
        }
    }
}
impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, u64> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let sample = match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let Some(time) = seconds
                    .checked_mul(1_000_000_000)
                    .and_then(|v| v.checked_add(u64::from(tv_nsec)))
                else {
                    state.error = Some("presentation timestamp overflow".into());
                    return;
                };
                if tv_nsec >= 1_000_000_000 {
                    state.error = Some("invalid presentation nanoseconds".into());
                    return;
                }
                Sample {
                    id: *id,
                    time_ns: Some(time),
                    refresh_ns: refresh,
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                    flags: format!("{flags:?}"),
                }
            }
            wp_presentation_feedback::Event::Discarded => Sample {
                id: *id,
                time_ns: None,
                refresh_ns: 0,
                sequence: 0,
                flags: "discarded".into(),
            },
            _ => return,
        };
        if state.pending.remove(id).is_none() {
            state.error = Some("duplicate Wayland feedback".into());
            return;
        }
        if state.received.len() >= 64 {
            state.error = Some("Wayland feedback queue bound exceeded".into());
            return;
        }
        state.received.push(sample);
    }
}
struct Inner {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
    registry: wl_registry::WlRegistry,
    origin: u64,
}
pub struct Wayland {
    inner: RefCell<Inner>,
}
impl Wayland {
    pub fn new(window: &Window) -> AnyResult<Option<Self>> {
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) = (
            window.display_handle()?.as_raw(),
            window.window_handle()?.as_raw(),
        ) else {
            return Ok(None);
        };
        // SAFETY: raw-window-handle supplies live libwayland pointers. The
        // borrowed backend never disconnects winit's display. Never destroy
        // or replace the borrowed wl_surface or its dispatch callbacks.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                surface.surface.as_ptr().cast(),
            )
        }?;
        let surface = wl_surface::WlSurface::from_id(&connection, id)?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        if state.presentation.is_none() || state.clock.is_none() {
            let _ = connection.backend().destroy_object(&registry.id());
            return Ok(None);
        }
        Ok(Some(Self {
            inner: RefCell::new(Inner {
                connection,
                queue,
                state,
                surface,
                registry,
                origin: 0,
            }),
        }))
    }
    pub fn begin(&self) -> AnyResult<()> {
        let mut inner = self.inner.borrow_mut();
        let clock = inner.state.clock.ok_or("presentation clock unavailable")?;
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(clock, &mut t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        inner.origin = t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64;
        Ok(())
    }
    pub fn prepare(&self, id: u64) -> AnyResult<()> {
        let mut inner = self.inner.borrow_mut();
        if inner.state.pending.len() >= 32 {
            return Err("Wayland pending feedback bound exceeded".into());
        }
        let feedback = inner
            .state
            .presentation
            .as_ref()
            .ok_or("presentation global unavailable")?
            .feedback(&inner.surface, &inner.queue.handle(), id);
        if inner.state.pending.insert(id, feedback).is_some() {
            return Err("duplicate Wayland present ID".into());
        }
        inner.connection.flush()?;
        Ok(())
    }
    pub fn drain(&self, frames: &mut [metrics::Frame]) -> AnyResult<()> {
        let mut inner = self.inner.borrow_mut();
        let Inner { queue, state, .. } = &mut *inner;
        // Nonblocking: winit or Vulkan WSI reads the shared socket. This only
        // dispatches our private pending queue, never winit's event handlers.
        queue.dispatch_pending(state)?;
        if let Some(error) = &state.error {
            return Err(error.clone().into());
        }
        let received = std::mem::take(&mut inner.state.received);
        for sample in received {
            let f = frames
                .iter_mut()
                .rev()
                .find(|f| f.id == sample.id)
                .ok_or("uncorrelated Wayland presentation")?;
            if f.os_present_s.is_some() {
                return Err("duplicate Wayland timestamp".into());
            }
            if let Some(time) = sample.time_ns {
                f.os_present_s = Some((i128::from(time) - i128::from(inner.origin)) as f64 / 1e9);
            }
            if inner.state.raw.len() >= 8000 {
                return Err("Wayland receipt bound exceeded".into());
            }
            inner.state.raw.push(sample);
        }
        Ok(())
    }
    pub fn evidence(&self) -> serde_json::Value {
        let i = self.inner.borrow();
        serde_json::json!({"basis":"Wayland wp_presentation feedback for the same winit wl_surface commit","clock_id":i.state.clock,"clock_origin_ns":i.origin,"pending":i.state.pending.len(),"statistics":i.state.raw})
    }
}
impl Drop for Wayland {
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        if let Some(p) = inner.state.presentation.take() {
            p.destroy();
        }
        // These proxies were created by this observer. The borrowed surface
        // and display belong to winit and must remain untouched.
        let backend = inner.connection.backend();
        for pending in inner.state.pending.values() {
            let _ = backend.destroy_object(&pending.id());
        }
        let _ = backend.destroy_object(&inner.registry.id());
        let _ = inner.connection.flush();
    }
}
