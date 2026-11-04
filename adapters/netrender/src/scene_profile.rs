// SPDX-License-Identifier: MPL-2.0
//! Opt-in wall-clock observations. The host owns collection and persistence.
//! Nested totals overlap their children; the sink runs after each measured span.
//! Without `scene-profile`, instrumentation is an inlined direct call.
#[cfg(feature = "scene-profile")]
use std::sync::{Arc, OnceLock};

#[cfg(feature = "scene-profile")]
pub struct Sample {
    pub stage: &'static str,
    pub elapsed: std::time::Duration,
    pub bytes: usize,
}
#[cfg(feature = "scene-profile")]
type Sink = Arc<dyn Fn(Sample) + Send + Sync>;
#[cfg(feature = "scene-profile")]
static SINK: OnceLock<Sink> = OnceLock::new();

/// Install once per process before starting work. No origin/content bytes are
/// supplied. The callback must be bounded and must not reenter scene operations.
#[cfg(feature = "scene-profile")]
pub fn install(sink: Sink) -> Result<(), &'static str> {
    SINK.set(sink)
        .map_err(|_| "scene profiler already installed")
}

#[inline]
pub fn measure<T>(stage: &'static str, bytes: usize, operation: impl FnOnce() -> T) -> T {
    #[cfg(feature = "scene-profile")]
    if let Some(sink) = SINK.get() {
        let start = std::time::Instant::now();
        let result = operation();
        sink(Sample {
            stage,
            bytes,
            elapsed: start.elapsed(),
        });
        return result;
    }
    let _ = (stage, bytes);
    operation()
}
