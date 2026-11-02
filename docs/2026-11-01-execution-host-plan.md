# Execution hosts behind Webcue

**Status:** Command-execution slice implemented and qualified on native macOS
**Date:** 2026-11-01

## Context and decision

Webcue already owns installation identity, request admission, ordered observations,
completion reservations and retirement. Keep that contract at the host edge.
Genet owns document semantics; wgpu-cue and Graft own presentation and native frame
custody. Hosts supply execution and transport. No engine registry, process launcher,
sandbox policy or GPU reference belongs in the renderer-free execution contract.

An installation scope and an execution incarnation are separate identities.
Neither identity grants authority. An execution adapter must bind its endpoint to
the expected identity; a privileged resource broker must independently enforce
origin, partition and resource permissions.

The [Vello placement review](2026-11-01-vello-execution-placement.md) extends this
decision across Classic, GPU/Hybrid and CPU. Renderer choice, execution placement
and output transport are independent. Keep the default scene/resource boundary
above backend-specific lowering, with GPU state local to its device owner. CPU
raster workers can instead return owned pixels. The current command endpoint does
not become a renderer protocol by accepting opaque scene payloads.

## This implementation

1. Add a host-owned command execution session to `webcue`. Accepting a request
   reserves its terminal event before transport. Transport backpressure retains
   locally accepted work. Transport loss closes admission and settles outstanding
   work with an explicit unknown-execution outcome. Never replay accepted work.
2. Separate retirement of the host-facing installation from evidence that its
   executor stopped. Stopping is nonblocking and host supplied. A broken channel
   proves loss of contact, not process death. Preserve final observations and
   editor clearing under mailbox pressure.
3. Extract the existing static Genet document commands from GPU presentation.
   Keep the same implementation in the existing local Genet adapter and a native
   child-process proof. Keep canonical document types and existing supplier pins.
4. Exercise admission, stale identity, duplicate completion, bounded queues,
   execution loss, retirement and independent stop evidence. Compare actual Genet
   load/find/zoom results locally and through a supervised subprocess; kill a
   subprocess with accepted work outstanding and retain the result.
5. Run focused tests and Clippy through the workspace storage guard. Retain logs
   in `Code/output/cue-execution-host`, document qualification limits, and integrate
   the scoped change on main after rechecking the remote and canonical checkout.

## Contract boundaries

The initial execution interface forwards correlated commands and ordered events;
it is not a complete remote `Surface` or a wire-format commitment. Full remote
input, publication actions, clocks, visibility and frame acquisition need explicit
admission semantics. `TextureSource` remains local to its borrowed device. A remote
renderer needs a platform resource-transfer adapter; a remote content producer
could instead feed a local renderer.

Execution stop is not GPU completion or exported-frame release. Renderer adapters
retain their resource/fence obligations separately. Hybrid's encode/submit ordering
and CPU's premultiplied output are explicit constraints in the Vello review. The
command slice keeps the existing Classic rasterizer. The
[CPU followup](2026-11-01-cpu-render-jobs.md) adds opt-in owned scene jobs and CPU
frames using the same supplier revisions.

The native proof is trusted static HTML with held resources, not an OS sandbox or
a compliant browser. Its bounded framing and subprocess ownership are fixture
mechanics, not a proposed universal IPC library. There is no live migration,
automatic restart, persisted request ledger or exactly-once external side-effect
claim. The surviving host can settle its own requests after executor failure.

## Alternatives and consequences

Making every engine trait remotely callable would expose VM-local values and
fine-grained synchronous operations. Keeping the existing document-command unit
together avoids that coupling. Folding process launch into `Surface` would tie
embedding to one topology and exclude host-controlled extension lifecycles.

The host ledger adds a queue and requires transport adapters to preserve ordered
delivery and distinguish backpressure from disconnection. In return, child failure
cannot erase the host's accepted-request obligations. Execution guarantees are
explicit declarations from the trusted host, not proof of sandbox effectiveness.

## Connections beyond this slice

Browser placement needs agent/agent-cluster and browsing-context identity;
permissions need an independent authority broker; service workers need lifetimes
without surfaces. Apple extensions add host-only launch, scheduling grants and
interruption handling. These connect to this contract through execution ownership,
not by adding browser policy or platform handles to the common command vocabulary.

## Implementation and validation receipt

The command-execution slice is implemented from upstream
`c314b7e3a2b2df4da2d9b0387b0c64c93b6b0e4d`. The same `GenetDocument` runs in the
existing local surface, a bounded cooperative worker, and the supervised native
helper. It prepares CPU layout before finding text. The stalled-helper fixture
observes an explicit hold witness before requesting termination and reaping it.

[Qualification receipt](../receipts/2026-11-01-execution-host/README.md):

- 33 focused command, lifecycle and framing tests passed, including local/process
  command parity, nonblocking local admission/stop, helper termination and reaping.
- Four native Metal tests passed on an AMD Radeon Pro Vega 56: retained lifecycle,
  IME, bounded navigation pressure, text-input/password geometry, document canvas
  policy, and separate Classic straight-alpha/transparent-clear readback.
- Workspace all-targets check and focused all-targets Clippy with `-D warnings`
  passed. Rust formatting, whitespace and TOML checks passed.
- The saved loss-to-retirement baseline failed its outcome assertion. The prior
  execution session failed the two current mailbox/transmission regressions; the
  final implementation passed all 14 execution-session tests and the expanded
  execution-loss test.
- Existing lockfile package records and supplier pins remain unchanged. The only
  added package record is the private `cue-execution-proof` fixture.

The initial sandbox prevented four guarded launches from running process
inventory. Unrestricted execution resolved that environmental blocker. All Cargo
commands still used the `cue-execution-host` build-storage lane. Logs, pre-fix
sources, final source snapshots and hash-verified frozen binaries are retained in
`/Users/markik/Code/output/cue-execution-host/`.

Native qualification exposed a stale test assumption inherited from the base:
Genet's pinned top-level session deliberately paints an opaque Canvas. The same
fixture failed identically before this extraction. The test now checks that
policy, while a separate raw rasterizer fixture checks actual transparent pixels.
No document canvas behavior or supplier pin changed. An existing ICU4X diagnostic
about missing CJK segmentation data remains in the native log; passing the IME
assertion does not qualify CJK segmentation generally.

The CPU adapter, worker/helper scene proof and Hybrid GPU-owner adapter are
qualified separately in their followup receipts. Remote input, platform-native handle transfer,
BrowserEngineKit integration and OS sandboxing remain outside this command slice.
The Vello review records their ownership constraints and qualification boundaries;
these native Classic checks do not qualify those additional paths.
