# wgpu-cue

A shared embedding contract for native Genet content and web engines. The
control plane is independent of the engine and the wgpu major. Presentation
names both directions already present in the stack: a producer supplies an
owned native frame, or renders on the receiving host's device.

`webcue` and `wgpu-cue` form the initial **0.1.0** contract release. Their APIs
may change within the 0.x series. The concrete adapters remain source packages
with `publish = false`; their proof covers the cases recorded below.

```toml
[dependencies]
wgpu-cue = "0.1.0"
# Or, for renderer-free controls only:
webcue = "0.1.0"
```

The published packages resolve their dependencies from crates.io. The development
workspace retains an exact Genet Git source with a registry version fallback,
so its unpublished adapter and the contract share one canonical document type
identity. Cargo strips that Git source when packaging the public contract.

| Package | Boundary |
| --- | --- |
| `webcue` | Controls, canonical retained input/IME, ordered events, correlated requests, semantic publications and actions, complete capability reports, owned native/CPU frame delivery. No wgpu or renderer dependency. |
| `wgpu-cue` | Host-device rendering, retained presentation, CPU upload, and a non-blocking macOS Metal importer through Graft. |
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
CEF, Servo, product migrations, native OS accessibility adapters, and native
IME candidate-window placement remain outside this proof.
