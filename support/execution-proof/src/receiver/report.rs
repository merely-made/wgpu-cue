// SPDX-License-Identifier: MPL-2.0
use super::*;
impl Receiver {
    pub(super) fn save(&mut self, cfg: &Config, error: Option<&str>) -> AnyResult<bool> {
        self.feedback.drain(&self.surface, &mut self.frames)?;
        let steady = metrics::summarize(&self.frames, "steady", self.feedback.refresh, cfg.phase);
        let loaded = metrics::summarize(&self.frames, "loaded", self.feedback.refresh, cfg.phase);
        let pacing = steady["distinct_content_60hz_target_met"] == true
            && loaded["distinct_content_60hz_target_met"] == true;
        let feedback = self
            .frames
            .iter()
            .filter(|f| f.os_present_s.is_some())
            .count();
        let covered = feedback >= 100 && feedback as f64 / self.frames.len().max(1) as f64 > 0.95;
        let complete =
            error.is_none() && self.handled > 100 && self.sizes.len() >= 2 && self.max_imports <= 2;
        let summary = serde_json::json!({
            "schema": "cue-window-pacing-v1",
            "mode": if cfg.local {"local"} else {"process"},
            "adapter": self.adapter,
            "display_refresh_hz": self.feedback.refresh,
            "target_hz": 60,
            "fixture_font": cfg.font,
            "feedback": self.feedback.evidence(),
            "steady": steady,
            "loaded": loaded,
            "custody": metrics::summarize(&self.frames,"custody",self.feedback.refresh,3.),
            "viewport_history": self.sizes,
            "inputs": self.inputs,
            "handled_inputs": self.handled,
            "max_imports": self.max_imports,
            "final_imports": self.renderer.imports(),
            "surface_timeouts_or_occlusions": self.skipped,
            "os_feedback_coverage": covered,
            "workload_completed": complete,
            "pacing_target_met": covered && complete && pacing,
            "validation_errors": *self.errors.lock().unwrap(),
            "error": error,
            "observation_limit": "Host-dispatched scroll to OS presentation feedback where available. No physical input, photon or scanout instrumentation. Windows timestamps are reconstructed from DXGI counters."
        });
        std::fs::write(
            cfg.output.join("frames.json"),
            serde_json::to_vec(&self.frames)?,
        )?;
        std::fs::write(
            cfg.output.join("summary.json"),
            serde_json::to_vec_pretty(&summary)?,
        )?;
        eprintln!("{}", serde_json::to_string_pretty(&summary)?);
        Ok(complete && covered && pacing)
    }
}

pub(super) fn inputs(cfg: &Config, scene: &wgpu_cue_netrender::Scene) -> AnyResult<()> {
    let backings = scene
        .fonts
        .iter()
        .map(|f| (f.data.as_ref().as_ptr() as usize, f.data.as_ref().len()))
        .collect::<std::collections::HashSet<_>>();
    let input = serde_json::json!({
        "font_family":cfg.font,"sections":120,
        "font_palette_entries":scene.fonts.len(),
        "font_source_bytes":scene.fonts.iter().map(|f|f.data.as_ref().len() as u64).sum::<u64>(),
        "unique_font_backings":backings.len(),
        "unique_font_backing_bytes":backings.iter().map(|(_,size)|*size as u64).sum::<u64>(),
        "font_entry_sizes":scene.fonts.iter().map(|f|f.data.as_ref().len()).take(32).collect::<Vec<_>>(),
        "resource_budget_bytes":RenderLimits::default().resource_bytes
    });
    std::fs::write(
        cfg.output.join("input-resources.json"),
        serde_json::to_vec_pretty(&input)?,
    )?;
    Ok(())
}
