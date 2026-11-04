# wgpu-cue

A shared embedding contract for native Genet content and web engines. The
control plane is independent of the engine and the wgpu major. Presentation
names both directions already present in the stack: a producer supplies an
owned native frame, or renders on the receiving host's device.

`wgpu-cue` is at **0.1.3**, with renderer-free `webcue` at **0.1.2**
(the initial contract shipped as 0.1.0). Their APIs
may change within the 0.x series. The concrete adapters remain source packages
with `publish = false`; their proof covers the cases recorded below.

```toml
[dependencies]
wgpu-cue = "0.1.3"
# Or, for renderer-free controls only:
webcue = "0.1.2"
```

The public packages resolve their dependencies from crates.io. The development
workspace patches the canonical document API to the exact Genet source used by
its unpublished adapter. A receiving workspace using Git Genet must apply its
own root `document-session-api` registry patch to that same Git source; Cargo
does not inherit dependency patch tables.

| Package | Boundary |
| --- | --- |
| `webcue` | Controls, canonical retained input/IME, ordered events, correlated requests, semantic publications and actions, complete capability reports, owned native/CPU frame delivery. No wgpu or renderer dependency. |
| `wgpu-cue` | Host-device rendering, retained presentation, CPU upload, and non-blocking macOS Metal, Linux Vulkan DMA-BUF and Windows DX12 importers through Graft. |
| `cue-netrender` | Private retained renderer and deferred quality/error-scope polling on the host device; independent producers never consume one another's quality results. |
| `cue-genet` | Concrete retained Livery sessions, held resources, document semantics, logical input/IME, and deferred GPU quality checks. |
| `cue-scry` | Concrete macOS WKWebView adapter, asynchronous snapshots, native Metal handoff, navigation observations, script results, native keys, and typed editable-state forwarding where supplied. |

`Surface` owns the common control protocol. `FrameSource: Surface` supplies a
move-owned frame. `TextureSource: Surface` borrows the host device and queue.
The receiving host chooses `Source::Native` or `Source::HostDevice`; it does
not need an Inker, Pelt, or Turnstone dependency. `Presenter` retains the last
usable texture while a producer or a native fence is pending. There is no
blocking default frame acquisition.

Canonical document input, IME and retained semantics are re-exported from
Genet's renderer-free `document-session-api`. Text-input state and caret
rectangles share that API too: logical viewport coordinates include page zoom
and scroll; the host adds placement and applies device scale once. Selection
uses UTF-16 offsets, while preedit retains its independent UTF-8 offsets.
Publishing a live editor reserves an ordered clearing event so a full mailbox
cannot strand geometry at hide or retirement. Native resource custody remains
`grafting-frame`; imports remain Graft's job. `webcue` adds lifecycle and host
protocol, rather than copying these types. The exact dependency revisions and
limits of the executed proof are in [the receipt](docs/2026-10-22-proof.md).

Requests use strictly increasing host IDs within a host-chosen owner and
installation generation. Acceptance reserves a terminal event slot. Rejection
accepts no work. An accepted operation produces exactly one completion, even
if retirement occurs while observation events have filled the queue. Hosts
must drain the final queue after `retire()` and retire their presenter when
removing an installation. See [the protocol](docs/contract.md).

`webcue::execution` provides host-owned accounting for commands sent to a
host-supplied executor. It retains accepted work through transport backpressure,
reports execution loss without replay, and distinguishes installation retirement
from observed executor termination. The initial interface covers commands and
ordered observations; full remote `Surface` controls and native frame transfer
remain separate. The [execution-host plan](docs/2026-11-01-execution-host-plan.md)
records the boundary and validation status. `support/execution-proof` contains a
private worker-thread/process Genet document-command fixture; it is not a sandbox or a
supported IPC protocol.

`cue-netrender` offers an opt-in `scene-jobs` feature for complete owned scene
snapshots and a `cpu` feature for Vello CPU jobs and a nonblocking `CpuFrame`
inbox. Hosts supply scheduling; document controls keep their existing owner.
The [CPU render-job contract](docs/2026-11-01-cpu-render-jobs.md) describes byte
budgets, process-local resource identities, premultiplied pixels and stale-job
refusal. The private proof's `cpu-raster` feature compares worker/helper pixels
from an actual Genet text scene and exercises host receiving on Metal.

The opt-in `hybrid` feature adds `HybridSceneSource` on a host device. Submission
owns its encoder and queue ordering; frame polling only observes readiness.
The [Hybrid contract](docs/2026-11-01-hybrid-render-jobs.md) covers single-flight
backpressure, GPU error scopes, invalidation, cleanup fences and reconstruction.
The proof's `hybrid-raster` feature enables native qualification independently of CPU.

The [two-process Metal handoff](docs/2026-11-01-metal-frame-handoff.md) adds actual
IOSurface capability transfer, producer GPU readiness, and tracked consumer GPU
release. `webcue::frame_slots` bounds reuse; `GraftIoSurfaceImporter` waits for
both final texture custody and a sealed queue fence. The proof's `metal-handoff`
feature runs the same Genet Hybrid scene in a helper with two exportable slots.

[Persistent render sessions](docs/2026-11-02-persistent-render-sessions.md) add
host-owned job admission, reserved terminal outcomes, byte budgets and renderer
incarnations. A document-owned publication clock survives renderer replacement.
The CPU proof supports persistent local/process endpoints; the Metal proof renders
changing scenes, resizes held-slot storage safely and replaces its renderer while
an older imported frame remains retained. Classic and Hybrid local outputs share
the same typed session vocabulary. Platform-native transfer remains with Graft.

The [Linux Vulkan handoff](docs/2026-11-02-linux-vulkan-handoff.md) adds tracked
linear RGBA8 DMA-BUF storage, explicit ownership release, and a private Linux
process fixture under `vulkan-handoff`. Its qualification is recorded separately
from the existing CPU and Metal paths.

The [Windows DX12 handoff](docs/2026-11-04-windows-dx12-handoff.md) adds shared
resource/fence custody, an independent consumer completion witness and a private
persistent process fixture under `dx12-handoff`. `GraftDx12SurfaceImporter`
requires final texture/view destruction before a lease can be returned. The
fixture selects the same adapter LUID in both processes; qualification records
AMD and NVIDIA runs separately from public pin availability and presentation.

Capabilities describe the installed adapter, including limitations; they do
not inherit a supplier's theoretical maximum. Every feature requires an
explicit supported, partial, or unavailable answer. The report also names the
presentation direction, semantic protocol, backend identity, platform and
build features. Initial construction and observed frame production are
separate evidence states. Hosts can use the same report for engine selection
and an inspector; routing policy remains with the host.

## Executable checks

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --check -p webcue -p wgpu-cue -p cue-genet -p cue-scry
```

The native checks are explicit because a missing GPU or desktop is not a pass:

```sh
cargo test -p wgpu-cue --test presentation --locked -- --ignored --nocapture
cargo test -p cue-genet --test retained --locked -- --ignored --nocapture
cargo run -p cue-scry --example wk_receiver --locked
# Linux: hardware Vulkan, EGL DMA-BUF import and GBM are required.
cargo test -p wgpu-cue --test vulkan --locked -- --ignored --nocapture
# Windows: hardware DX12 and the D3D12 debug layer are required.
cargo test -p cue-execution-proof --features dx12-handoff,cpu-raster --test dx12_handoff --locked -- --include-ignored --test-threads=1
```

The first two execute real Metal work on macOS. The Scry example creates a
temporary private WKWebView window and exits after checking its receiving host. It uses
asynchronous CPU snapshots and needs no screen-capture permission. Scry's
native capture path is implemented but has not been qualified by that example.
The initial 0.1.0 proof excluded product receiving. Inker/Pelt/Graphshell
receiving is recorded in Mere's `design_docs/inker_docs/implementation_strategy/2026-10-22_shared_surface_receiving_plan.md`. Servo/Metal product receiving and retained Vulkan/Wayland receiving are
recorded in `turnstone/design_docs/2026-08-25_browser_surface_implementation_plan.md`.
The Linux native producer/import pass is tracked in
`wgpu-graft/design_docs/2026-10-23_linux_native_cue_plan.md`; its shared importer
accepts move-owned, producer-complete RGBA/BGRA DMA-BUFs with explicit layouts.
Unsupported semaphore synchronization is refused, not treated as a sync file.
Concrete CEF/Scry shared-native receiving, OS accessibility adapters and IME
candidate-window placement retain separate qualification gates.
