# Native receiving-window workload

This private fixture requires an unlocked graphical desktop, a hardware GPU,
the native facilities used by the platform handoff tests, and both helper and
receiver binaries. It uses the pinned Genet/Netrender Vello Hybrid path. It does
not select Classic or Vello CPU. No production windowing API or transport ABI
is added to Cue.

Build both binaries from the workspace root:

```sh
cargo +1.99.1 build --release --locked -p cue-execution-proof --features window-receiver --bins
```

Under `/Users/markik/Code`, use the workspace build-storage launcher and its
stable lane instead of direct Cargo. The lane's `artifacts/release` directory
contains both executables. Other hosts should use their existing stable cache.

Run from the graphical user's environment with fresh absolute output paths:

```sh
/absolute/bin/cue-window-receiver \
  --helper /absolute/bin/cue-execution-proof \
  --output /absolute/receipts/process \
  --font sans-serif --phase-seconds 15 --warmup-seconds 3
```

Add `--local` for the same host-device Hybrid control. On Windows use `.exe`
paths and select an adapter with `CUE_DX12_ADAPTER_NAME`; the helper receives
the host's exact adapter LUID. The receiver uses the GUI subsystem, writes its
own receipts and suppresses the helper console. On Wayland set the graphical
session's `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY` and `DBUS_SESSION_BUS_ADDRESS`
when launching over SSH. `WINIT_UNIX_BACKEND=wayland` selects the measured path.
An SSH shell by itself is not evidence of a working graphical session.

For macOS package already-built binaries as a foreground application:

```sh
python3 support/execution-proof/tools/package_receiver.py \
  --bin-dir /absolute/bin --bundle /absolute/CueProcess.app -- \
  --output /absolute/receipts/mac-process --font Arial \
  --phase-seconds 15 --warmup-seconds 3
```

Open the app in the graphical session. Package another fresh app/output with
`--local` for the control. The tool freezes both binaries using APFS clone
copies, checks SHA-256 and stores arguments and hashes in the bundle. It never
builds code or overwrites a bundle/output. The tested Mac's generic sans-serif
closure exceeds the unchanged 32 MiB font/image budget; Arial is recorded as
its explicit measured fixture. Font resolution differs between operating systems.

The default workload has three seconds of warmup, fifteen seconds of steady
scroll, fifteen seconds of scroll plus resize and two CPU stress threads, three
seconds of retained-view custody, then three seconds of drain. It renders a
120-section text/border document at 800x600, requests 960x640, and dispatches
scroll at a nominal 60 Hz. Slow ticks coalesce missed dispatch opportunities;
the metric starts at actual host dispatch, not at a physical input device.

`summary.json` separates workload completion, OS feedback coverage and the
fresh-content 60 Hz target. `frames.json` preserves raw IDs, scene revisions,
content sequences, host input timestamps, present calls, OS observations and
CPU/acquisition timings. `input-resources.json` records the font closure sizes.
`error.json` records startup/runtime failures or a failed final pacing gate.
Exit 1 with `summary.error == null` can mean the workload completed correctly
but pacing/coverage did not qualify. Never treat that as a performance pass.

The gate requires more than 95 percent OS timestamp coverage with at least
100 observations, handled input, an actual size change, bounded imports, and
nearly 900 distinct content frames per fifteen-second measured phase with no
content interval above 25 ms. Repeating a retained texture cannot satisfy it.
The raw receipt bounds are 8,000 frames and 32 viewport records; long/high-refresh
runs may fail those bounds. Keep default-duration runs and preserve failed runs.

Targeted correctness checks, requiring real native hardware:

```sh
cargo +1.99.1 test --locked -p cue-execution-proof --features window-receiver --lib
# macOS
cargo +1.99.1 test --locked -p cue-execution-proof --features window-receiver --test hybrid --test metal_handoff -- --include-ignored --test-threads=1
# Windows; run separately for each adapter with its name selection
cargo +1.99.1 test --locked -p cue-execution-proof --features window-receiver --test dx12_handoff -- --include-ignored --test-threads=1
# Linux: run vulkan_handoff through tools/validate_vulkan.py, as in the Vulkan qualification.
cargo +1.99.1 clippy --locked -p cue-execution-proof --features window-receiver --all-targets -- -D warnings
```

Read the [qualification record](../../docs/2026-11-04-window-presentation-pacing.md)
for the exact executed environments, timestamp semantics and failed target.

For CPU attribution, additionally build with `scene-profile`. The
`cue-scene-profile ABSOLUTE_FRESH_OUTPUT FONT_FAMILY` binary runs the exact
document through repeated preparation/capture/decode plus a resource-payload
ablation that is never rendered. `CUE_SCENE_PROFILE_DIR` enables per-PID stage
logs in the receiving host and helper. See the [stage profiling record](../../docs/2026-11-04-scene-stage-profile.md)
for the instrumentation boundaries, summarizer and results.
