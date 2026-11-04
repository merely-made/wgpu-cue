#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Summarize bounded stage JSONL; CPU probes exclude their first 8 iterations."""
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


def summarize(path):
    records = [json.loads(line) for line in path.read_text().splitlines()]
    assert 0 < len(records) <= 20000
    assert [r["sequence"] for r in records] == list(range(len(records)))
    assert len({r["pid"] for r in records}) == 1
    cpu = any(r["stage"] == "probe.iteration" for r in records)
    iteration = -1
    stages = {}
    for record in records:
        if record["stage"] == "probe.iteration":
            iteration = record["bytes"]
            continue
        if cpu and iteration < 8:
            continue
        stages.setdefault(record["stage"], []).append(record["elapsed_ns"] / 1e6)
    if cpu:
        assert iteration == 71
        assert all(len(values) == 64 for values in stages.values())
    return {"file": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "mode": "cpu-probe-warmup-excluded" if cpu else "native-window-all-phases-and-startup",
            "stages_ms": {name: distribution(values) for name, values in sorted(stages.items())}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path)
    args = parser.parse_args()
    print(json.dumps({"schema": "cue-scene-stage-summary-v1", "profiles": [summarize(p) for p in args.paths],
                      "note": "Wall time. Parent totals overlap children; do not add them. JSON parse includes serde resource allocation. Rebinding is the additional fresh-ID byte copy. No rendering-performance claim."}, indent=2))


if __name__ == "__main__":
    main()
