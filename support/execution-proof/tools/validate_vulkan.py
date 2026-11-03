#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Run a Vulkan fixture with validation diagnostics included in its result.

The command should execute one Rust test target with --nocapture. Build with the
workspace's guarded Cargo launcher first, then pass the resulting test binary.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import re
import subprocess


def assess(output, exit_code, expected_tests):
    summaries = re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", output
    )
    counts = [sum(int(row[i]) for row in summaries) for i in range(3)]
    diagnostics = collections.Counter(re.findall(r"\bVUID-[\w-]+", output))
    layer_loaded = 'Insert instance layer "VK_LAYER_KHRONOS_validation"' in output
    errors = bool(re.search(r"Validation Error:|SYNC-HAZARD", output))
    passed = (
        exit_code == 0
        and expected_tests > 0
        and counts == [expected_tests, 0, 0]
        and layer_loaded
        and not diagnostics
        and not errors
    )
    return {
        "passed": passed,
        "command_exit": exit_code,
        "expected_tests": expected_tests,
        "tests": dict(zip(("passed", "failed", "ignored"), counts)),
        "validation_layer_loaded": layer_loaded,
        "validation_diagnostics": dict(diagnostics),
        "validation_errors_or_sync_hazards": errors,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--expected-tests", type=int, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or args.expected_tests < 1:
        parser.error("a command and a positive expected test count are required")
    env = os.environ.copy()
    env.update(
        VK_INSTANCE_LAYERS="VK_LAYER_KHRONOS_validation",
        VK_LAYER_ENABLES="VK_VALIDATION_FEATURE_ENABLE_SYNCHRONIZATION_VALIDATION_EXT",
        VK_LOADER_DEBUG="layer",
        # wgpu may install its own callback without a logger. Force a layer
        # sink and revalidate shaders instead of trusting a warm disk cache.
        VK_LAYER_DEBUG_ACTION="VK_DBG_LAYER_ACTION_LOG_MSG",
        VK_LAYER_LOG_FILENAME="stdout",
        VK_LAYER_REPORT_FLAGS="error,warn",
        VK_LAYER_CHECK_SHADERS="1",
        VK_LAYER_CHECK_SHADERS_CACHING="0",
        VK_LAYER_ENABLE_MESSAGE_LIMIT="0",
    )
    args.log.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    with args.log.open("w") as log:
        result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT)
    report = assess(args.log.read_text(errors="replace"), result.returncode, args.expected_tests)
    report.update(command=command, log=str(args.log.resolve()))
    args.receipt.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
