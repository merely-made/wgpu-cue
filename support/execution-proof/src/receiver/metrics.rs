// SPDX-License-Identifier: MPL-2.0
use serde::Serialize;
use serde_json::{Value, json};
#[derive(Serialize)]
pub struct Frame {
    pub id: u64,
    pub revision: u64,
    pub content_sequence: u64,
    pub phase: &'static str,
    pub present_call_s: f64,
    pub os_present_s: Option<f64>,
    pub input_s: Option<f64>,
    pub acquire_ms: f64,
    pub prepare_ms: f64,
    pub update_ms: f64,
}
pub fn distribution(mut values: Vec<f64>) -> Value {
    values.retain(|v| v.is_finite());
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return Value::Null;
    }
    let percentile = |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"samples":values.len(),"p50":percentile(0.5),"p95":percentile(0.95),"p99":percentile(0.99),"max":values.last()})
}
pub fn summarize(frames: &[Frame], phase: &str, refresh_hz: f64, duration_s: f64) -> Value {
    let frames: Vec<_> = frames.iter().filter(|f| f.phase == phase).collect();
    let mut shown: Vec<_> = frames
        .iter()
        .filter_map(|f| f.os_present_s.filter(|t| *t > 0.).map(|t| (t, *f)))
        .collect();
    shown.sort_by(|a, b| a.0.total_cmp(&b.0));
    let intervals: Vec<_> = shown
        .windows(2)
        .map(|w| (w[1].0 - w[0].0) * 1000.)
        .collect();
    let mut previous = None;
    let content: Vec<_> = shown
        .iter()
        .filter(|(_, f)| {
            let changed = previous != Some(f.content_sequence);
            previous = Some(f.content_sequence);
            changed
        })
        .collect();
    let content_intervals: Vec<_> = content
        .windows(2)
        .map(|w| (w[1].0 - w[0].0) * 1000.)
        .collect();
    let refresh_ms = 1000. / refresh_hz;
    let missed = |v: &[f64]| {
        v.iter()
            .map(|dt| ((dt / refresh_ms).round() as u64).saturating_sub(1))
            .sum::<u64>()
    };
    let latency: Vec<_> = content
        .iter()
        .filter_map(|(t, f)| f.input_s.map(|input| (t - input) * 1000.))
        .collect();
    json!({"present_calls":frames.len(),"os_presented":shown.len(),"distinct_content":content.len(),
      "os_frame_interval_ms":distribution(intervals.clone()),"distinct_content_interval_ms":distribution(content_intervals.clone()),
      "estimated_missed_display_refreshes":missed(&intervals),"estimated_missed_content_refreshes":missed(&content_intervals),
      "input_to_os_presentation_ms":distribution(latency),
      "acquire_ms":distribution(frames.iter().map(|f|f.acquire_ms).collect()),
      "prepare_ms":distribution(frames.iter().map(|f|f.prepare_ms).filter(|t|*t>0.).collect()),
      "update_ms":distribution(frames.iter().map(|f|f.update_ms).collect()),
      "expected_content_frames":(duration_s*60.).floor() as u64,
      "distinct_content_60hz_target_met":content.len() >= ((duration_s*60.).floor() as usize).saturating_sub(2).max(2) && content_intervals.iter().all(|dt| *dt <= 25.)})
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(id: u64, revision: u64, t: f64) -> Frame {
        Frame {
            id,
            revision,
            content_sequence: revision,
            phase: "steady",
            present_call_s: t,
            os_present_s: Some(t),
            input_s: Some(t - 0.01),
            acquire_ms: 0.,
            prepare_ms: 0.,
            update_ms: 0.,
        }
    }
    #[test]
    fn retained_redraws_do_not_prove_fresh_content_sixty_fps() {
        let v = summarize(
            &[
                frame(1, 1, 1.),
                frame(2, 1, 1. + 1. / 60.),
                frame(3, 2, 1. + 2. / 60.),
            ],
            "steady",
            60.,
            0.05,
        );
        assert_eq!(v["estimated_missed_display_refreshes"], 0);
        assert_eq!(v["estimated_missed_content_refreshes"], 1);
        assert_eq!(v["distinct_content_60hz_target_met"], false);
    }
    #[test]
    fn out_of_order_callbacks_are_sorted_by_presentation_time() {
        let v = summarize(
            &[frame(2, 2, 1. + 1. / 60.), frame(1, 1, 1.)],
            "steady",
            60.,
            0.05,
        );
        assert_eq!(v["distinct_content"], 2);
        assert_eq!(v["estimated_missed_display_refreshes"], 0);
    }
    #[test]
    fn short_burst_followed_by_stall_fails_full_phase_coverage() {
        let v = summarize(
            &[frame(1, 1, 1.), frame(2, 2, 1. + 1. / 60.)],
            "steady",
            60.,
            5.,
        );
        assert_eq!(v["distinct_content_60hz_target_met"], false);
    }
    #[test]
    fn missing_os_feedback_cannot_pass_pacing_gate() {
        let mut f = frame(1, 1, 1.);
        f.os_present_s = None;
        let v = summarize(&[f], "steady", 60., 1.);
        assert_eq!(v["os_presented"], 0);
        assert_eq!(v["distinct_content_60hz_target_met"], false);
    }
}
