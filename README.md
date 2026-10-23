# wgpu-cue

A shared embedding contract for native Genet content and web engines. The
control plane is independent of the engine and the wgpu major. Presentation
names both directions already present in the stack: a producer supplies an
owned native frame, or renders on the receiving host's device.

`webcue` and `wgpu-cue` are at **0.1.1** (the initial contract shipped as 0.1.0). Their APIs
may change within the 0.x series. The concrete adapters remain source packages
with `publish = false`; their proof covers the cases recorded below.

```toml
[dependencies]
wgpu-cue = "0.1.1"
# Or, for renderer-free controls only:
webcue = "0.1.1"
```

The public packages resolve their dependencies from crates.io. The development
workspace patches the canonical document API to the exact Genet source used by
its unpublished adapter. A receiving workspace using Git Genet must apply its
own root `document-session-api` registry patch to that same Git source; Cargo
does not inherit dependency patch tables.

| Package | Boundary |
| --- | --- |
| `webcue` | Controls, canonical retained input/IME, ordered events, correlated requests, semantic publications and actions, complete capability reports, owned native/CPU frame delivery. No wgpu or renderer dependency. |
| `wgpu-cue` | Host-device rendering, retained presentation, CPU upload, and a non-blocking macOS Metal importer through Graft. |
| `wgpu-cue-netrender` | Private retained renderer and deferred quality/error-scope polling on the host device; independent producers never consume one another's quality results. |
| `wgpu-cue-genet` | Concrete retained Livery sessions, held resources, document semantics, logical input/IME, and deferred GPU quality checks. |
| `wgpu-cue-scry` | Concrete macOS WKWebView adapter, asynchronous snapshots, native Metal handoff, navigation observations, script results, native keys, and typed editable-state forwarding where supplied. |

`Surface` owns the common control protocol. `FrameSource: Surface` supplies a
move-owned frame. `TextureSource: Surface` borrows the host device and queue.
The receiving host chooses `Source::Native` or `Source::HostDevice`; it does
not need an Inker, Pelt, or Turnstone dependency. `Presenter` retains the last
usable texture while a producer or a native fence is pending. There is no
blocking default frame acquisition.

Canonical document input, IME and retained semantics are re-exported from
Genet's renderer-free `document-session-api`. Native resource custody remains
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
cargo fmt --check -p webcue -p wgpu-cue -p wgpu-cue-genet -p wgpu-cue-scry
```

The native checks are explicit because a missing GPU or desktop is not a pass:

```sh
cargo test -p wgpu-cue --test presentation --locked -- --ignored --nocapture
cargo test -p wgpu-cue-genet --test retained --locked -- --ignored --nocapture
cargo run -p wgpu-cue-scry --example wk_receiver --locked
```

The first two execute real Metal work on macOS. The last creates a temporary
private WKWebView window and exits after checking its receiving host. It uses
asynchronous CPU snapshots and needs no screen-capture permission. Scry's
native capture path is implemented but has not been qualified by that example.
The initial 0.1.0 proof excluded product receiving. Inker/Pelt/Graphshell
receiving is recorded in Mere's `design_docs/inker_docs/implementation_strategy/2026-10-22_shared_surface_receiving_plan.md`. Native CEF/Servo
receiving, OS accessibility adapters and IME candidate-window placement remain
separate qualification gates.
