# Receiving-window presentation and pacing

The real receiving-window workload completes on the tested Mac, Windows and
Linux hardware, including local and two-process rendering, input, resize,
retained-view custody and a final zero-import drain. **The fresh-content 60 Hz
target fails in every measured run.** Windows timestamp coverage also fails the
strict coverage threshold. This establishes a performance baseline and preserves
native correctness; it does not qualify sustained 60 fps.

## Ownership and fixture

The optional private `window-receiver` binary owns winit windowing, FIFO
presentation, workload scheduling and measurement. Genet owns the retained
120-section document and dispatches canonical scroll input through
`GenetDocument::input`. Both local and process cases prepare the same owned
scene-snapshot representation. Local rendering uses `HybridSceneSource`; remote
rendering uses the existing persistent renderer session, platform process helper
and Graft importer. No supplier pins, native transfer contracts or public Cue
windowing APIs change. This fixture covers Vello Hybrid only.

The default phases are warmup 3 s, steady scrolling 15 s, scrolling plus two CPU
stress threads and resize 15 s, retained-view custody 3 s, and drain 3 s. Initial
viewport is 800x600; requested resize is 960x640. Windows additionally reports
transient native size changes during launch; all actual sizes are in the receipt.
The host attempts scroll at 60 Hz and coalesces missed opportunities. Input
latency begins at actual host dispatch of the oldest coalesced input represented
by a snapshot. No physical keyboard/mouse latency is measured.

The process helper stays alive across frames. At most two imported slots may
exist. The custody phase retains a texture view for 100 ms after retiring its
frame; releasing a lease while that view remains held is an error. Final drain
requires no pending job, retained view or imported slot. The consumer seals GPU
use after its last sampling submission, then drops its aliases; swapchain
presentation may finish later because it owns the copied result. Cue does not
need to retain the source allocation until display scanout.

Four metric regressions reject retained-texture FPS as fresh-content FPS,
missing feedback and short bursts followed by stalls, and sort callbacks by
presentation time. The 60 Hz gate requires nearly 900 distinct content frames
in each 15 s phase with every observed content interval at most 25 ms. It also
requires more than 95% timestamp coverage and at least 100 OS observations,
actual handled input and a size change. A new scene revision alone cannot turn
a retained content redraw into a fresh input result.

## Executed environments and time bases

| Host | Hardware/backend | Environment | Reported display rate | Timestamp basis |
| --- | --- | --- | --- | --- |
| Mac | AMD Radeon Pro Vega 56 / Metal | macOS 15.8.1 | 60 Hz | `MTLDrawable.addPresentedHandler` and `presentedTime`, aligned with `CACurrentMediaTime` |
| O-PC | AMD Radeon 780M / DX12 | Windows 11 build 26220; driver 31.0.14084.3001 | 165 Hz | DXGI present/refresh counts and QPC reconstruction |
| O-PC | NVIDIA RTX 4060 Laptop / DX12 | Same desktop; driver 32.0.16.1088 | 165 Hz | DXGI present/refresh counts and QPC reconstruction |
| T-PC | AMD RADV RENOIR / Vulkan | Fedora 44, kernel 7.2.13-200.fc44.x86_64, Mesa 26.2.6, Wayland | 60 Hz | `wp_presentation` on the same winit surface commit, protocol clock ID 1 |

Windows retains raw `PresentCount`, `PresentRefreshCount`, `SyncRefreshCount`
and `SyncQPCTime`. A timestamp is reconstructed using the reported display rate;
it is not a per-drawable callback. Polling can skip presents. Coverage is only
92.89–93.75%, so observed interval percentiles remain partial samples and no
complete Windows presentation trace is claimed. Raw counter deltas separately
record estimated late refreshes; skipped polling observations are not all
counted as missed display refreshes. See Microsoft's
[DXGI frame statistics](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics).

The Linux driver does not advertise `VK_GOOGLE_display_timing`. The fixture
therefore uses Wayland presentation feedback through a separately owned event
queue on the existing display and surface; winit keeps ownership of the socket
read loop and window. Feedback records actual timestamps, sequence, refresh,
flags and discarded presentations. The optional Vulkan Google path is compiled
but has not been exercised natively on this host. None of these OS feedback
sources substitutes for photon/scanout instrumentation.

## Measurements

Each row is one optimized run, not a repeated-run statistical comparison. Units
are milliseconds. Content interval means time between observed distinct content
sequences. The target is 16.7 ms; repeated presentation of one texture is excluded.
Windows rows are the partial reconstructed samples described above.

| Run | Steady content interval p50 / p95 / p99 | Loaded content interval p50 / p95 / p99 | Steady input-to-OS p50 / p99 | OS timestamp coverage |
| --- | --- | --- | --- | --- |
| Linux local | 83.3 / 100.0 / 183.3 | 100.0 / 116.7 / 150.0 | 189.7 / 200.3 | 99.48% |
| Linux process | 150.0 / 166.7 / 183.3 | 183.3 / 233.3 / 250.0 | 306.2 / 441.5 | 100.00% |
| Mac local | 116.7 / 116.7 / 133.4 | 116.7 / 183.4 / 183.4 | 256.9 / 270.7 | 99.32% |
| Mac process | 183.4 / 200.1 / 216.7 | 183.4 / 200.1 / 216.7 | 387.0 / 438.1 | 99.69% |
| Windows AMD local | 91.0 / 100.4 / 104.3 | 89.4 / 99.5 / 105.8 | 177.9 / 195.2 | 93.75% |
| Windows AMD process | 145.6 / 162.7 / 166.7 | 139.7 / 171.8 / 206.7 | 287.5 / 334.2 | 93.06% |
| Windows NVIDIA local | 85.3 / 99.2 / 169.8 | 90.1 / 103.6 / 175.4 | 174.5 / 202.6 | 92.89% |
| Windows NVIDIA process | 132.2 / 142.2 / 144.5 | 128.8 / 149.7 / 160.7 | 263.1 / 277.9 | 93.18% |

Raw distributions include maxima, acquisition/prepare/update time, observed
refresh misses, content counts, viewport history and validation results in the
[compact receipt](../receipts/2026-11-04-window-pacing/summary.json). Every final
run records `workload_completed: true`, no runtime error, no uncaptured wgpu
validation errors and zero final imports. All four process runs reach the
expected two-import bound. All eight return exit 1 for the failed pacing gate;
the Windows launch controller records a null child exit-code property and uses
the child's persisted summary for the result. It does not assert a zero exit.

Median combined document prepare plus snapshot capture is about 47–60 ms before
rendering. Local update medians are about 83–115 ms during steady scrolling.
Those measurements already exceed the frame budget without process transfer.
The process path's retained redraw cadence can be much faster than its content
cadence; it must not be described as fresh-content 60 fps. The stage timings
are not an attribution profile: layout, snapshot encoding/validation, renderer
decode and resource reconstruction need separate instrumentation before assigning
causality or claiming an IPC overhead number.

A concrete next investigation is the owned snapshot/resource path: current
capture serializes and validates, and decoding reconstructs process-local
resources. Typed local delivery and stable resource caching are candidates,
not changes implemented here. Measure those stages and keep the local control
and scene/output correctness controls before selecting that optimization.

## Input closure and controls

The portable default is `sans-serif`; it resolves to different resources on
each OS. Linux's final font closure is 1,592,256 bytes, Windows' is 2,035,964.
The Mac uses explicit Arial, 1,524,220 bytes. Each has three palette entries
including the zero-length entry. No fonts are bundled or added to the receipt.

Generic sans-serif on this Mac resolves to 60,493,348 bytes and is correctly
refused by the unchanged 33,554,432-byte font/image resource budget. The original
Linux Arial fixture also exceeded that budget. Those failed controls are retained;
no resource limit was raised to make a run pass. The results compare local and
process execution within each fixture, not pixel-identical fonts across OSes.

The first Wayland resize probe returned its new size synchronously and did not
emit a later `Resized` event. The initial harness ignored that return value, so
its receipt failed workload completion with only the original viewport. The final
harness applies the returned size immediately; both final Linux modes record
800x600 and 960x640. The earlier failing probe remains in the evidence directory.
An initial Windows matrix task reached its controller's five-minute deadline;
its partial runs are preserved. The final controller waits on the root process,
uses a fifteen-minute task deadline and records all four completed runs.

## Verification and evidence

The final source8 gates execute 16 positive tests on Mac (six proof unit,
five Hybrid, five Metal handoff), 13 on Windows (seven proof unit, three native
handoff per adapter), and ten on Linux (eight proof unit, two native handoff):
39 positive test executions across these environments. Targeted all-target
Clippy passes with `-D warnings` on all three. The Windows collector retains its
previously documented warning baseline (IDs 1361 and 820 at warning severity
only) and rejects errors/corruption or unexpected warnings. The Linux native
fixture explicitly loads the validation layer and records zero validation
diagnostics. Neither statement is a claim of a full browser shader audit.

The native fault controls from the prior handoff slices remain separate evidence.
This slice's metric regressions and failing resize/font probes are additional
controls; a failed pacing gate is not added to the passing-test count.

All 565 locked package identities are unchanged. The other 564 package records
are identical; only six private proof dependency edges are added. Genet remains
`82ef34d9`, Netrender `79c9487b`, Vello `9f454193`, Graft `d903cfac`, and wgpu/Naga
30.0.1. No supplier cohort update or crates.io release is required.

Raw evidence root: `/Users/markik/Code/output/cue-window-pacing/`. `source8.json`
and `source8.tar` identify measured code; the compact receipt includes binary
and raw-result SHA-256 values. Final raw runs are `mac-arial-local8`,
`mac-arial-process8`, `windows-collected8/windows-sans-*`, and
`linux-collected8/cue-window-pacing/linux-sans-*`. Native gate logs and receipts
are retained beside them. Development runs and frozen binaries are preserved.
The Mac receipt budget is 650 MB including frozen debug/release apps; Linux and
Windows are each bounded to 100 MB without copied executables. Compiler caches
remain in the existing stable lanes.

See [reproduction instructions](../support/execution-proof/WINDOW_RECEIVER.md).
This qualifies one private same-adapter Hybrid fixture and its bounded native
custody checks. Other GPUs, Classic/CPU paths, full browser workloads, sandboxing,
physical input latency, power behavior, multi-monitor/VRR and production
presentation remain unqualified. There is no hosted-CI or universal 60 fps claim.
