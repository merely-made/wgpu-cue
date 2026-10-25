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
| `wgpu-cue` | Host-device rendering, retained presentation, CPU upload, and non-blocking macOS Metal / Linux Vulkan DMA-BUF importers through Graft. |
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
```

The first two execute real Metal work on macOS. The last creates a temporary
private WKWebView window and exits after checking its receiving host. It uses
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
