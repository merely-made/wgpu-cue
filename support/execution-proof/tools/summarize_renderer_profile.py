#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Summarize same-supplier renderer probes; exclude eight warmup frames per case."""
import argparse
import hashlib
import json
import math
from pathlib import Path


def distribution(values):
    values = sorted(values)
    def percentile(q):
        return values[max(0, math.ceil(len(values) * q) - 1)]
    return {"samples": len(values), "mean": sum(values) / len(values),
            "p50": percentile(.5), "p95": percentile(.95),
            "p99": percentile(.99), "max": values[-1]}


def summarize(directory):
    summary = json.loads((directory / "summary.json").read_text())
    rows = json.loads((directory / "rows.json").read_text())
    assert summary["schema"] == "cue-renderer-probe-v1"
    assert len(rows) == sum(c["frames"] for c in summary["cases"])
    assert all(row["pixels_equal"] for row in rows)
    cases = {}
    for case in summary["cases"]:
        name = case["case"]
        all_rows = [r for r in rows if r["case"] == name]
        assert [r["iteration"] for r in all_rows] == list(range(case["frames"]))
        selected = [r for r in all_rows if not r["warmup"]]
        assert len(selected) == case["frames"] - summary["warmup"]
        assert len({r["variant"] for r in all_rows}) == summary["variants"]
        cases[name] = {**case,
            "stages_ms": {key.removesuffix("_ns"): distribution([r[key]/1e6 for r in selected])
                          for key in selected[0] if key.endswith("_ns")},
            "counters": {key: distribution([r[key] for r in selected])
                         for key in ["ops", "glyphs", "prepared_glyphs_delta", "cache_resets_delta",
                                     "font_count", "font_bytes", "cached_glyphs", "hydrated_images_delta",
                                     "uploaded_images_delta", "cached_images", "gpu_texture_bytes"]},
            "variant_checksums": {str(r["variant"]): r["pixel_checksum_fnv64"] for r in all_rows}}
    return {"directory": str(directory), "adapter": summary["adapter"], "font": summary["font"],
            "reverse_policy_order": summary["reverse_policy_order"],
            "raw_sha256": {name: hashlib.sha256((directory/name).read_bytes()).hexdigest()
                           for name in ["rows.json", "summary.json"]}, "cases": cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directories", nargs="+", type=Path)
    args = parser.parse_args()
    print(json.dumps({"schema": "cue-renderer-profile-summary-v1", "runs": [summarize(d) for d in args.directories],
                      "note": "CPU wall time; session.render includes admission, text/image preparation, lowering and GPU encoding. Completion wait/readback are separate CPU observations, not GPU timestamps. Controlled cache ablations do not change production policy or establish native presentation performance."}, indent=2))


if __name__ == "__main__":
    main()
