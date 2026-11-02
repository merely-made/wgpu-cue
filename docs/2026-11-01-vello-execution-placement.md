# Vello execution placement behind Cue

**Date:** 2026-11-01
**Status:** Classic, CPU and GPU/Hybrid have bounded native qualification; native frame transfer remains separate

## Renderer choice and placement

Renderer choice, execution placement and presentation transport are independent
host decisions. The three implementations are Vello Classic (the original compute
renderer), Vello GPU (CPU preprocessing with GPU rasterization/compositing), and
Vello CPU. Netrender retains `HybridSession` and the `vello-hybrid` feature; its
dependency alias now selects `vello_gpu`. WebGL2 is another backend route of that
sparse GPU implementation, not another renderer family requiring another process.

| Renderer | Current Netrender ownership | Useful placement | Cue receiving path |
| --- | --- | --- | --- |
| Classic | Registry-bearing `Renderer`, device/queue, retained resources and deferred quality observations | Keep renderer and GPU resources with the device owner. Content can prepare a source scene elsewhere. | Existing `TextureSource` on the host device; a remote GPU owner needs owned native-frame transfer. |
| GPU / Hybrid | `HybridSession` owns CPU sources/caches, GPU textures and renderer resources on one supplied device/queue | `HybridSceneSource` keeps the session intact with the GPU owner and explicitly submits before polling readiness. CPU preparation is a possible future worker boundary, not an existing process-transfer contract. | Opt-in host-device adapter; a remote GPU owner still needs native-frame transfer. |
| CPU | `CpuSession` owns source/derived resources and produces a fresh CPU pixmap without a GPU | A local worker or helper process can own the raster session. Provider-internal parallelism still needs a shared CPU budget. | Opt-in `CpuSceneSource` returns owned premultiplied `CpuFrame`; the host may upload those pixels. |

`cue-netrender::SceneRasterizer` implements Classic. The opt-in `cpu` feature adds
the CPU path and its locked supplier closure. `scene-jobs` exposes the shared owned
scene boundary without enabling CPU. The default feature set remains empty;
choosing a presentation mode does not construct or qualify another adapter.
The opt-in `hybrid` feature similarly reuses `scene-jobs` without enabling CPU.

## Common input belongs above backend-specific preparation

Genet's CPU preparation produces Netrender's authoritative `Scene`. A renderer
service would receive an owned scene snapshot and the resource sources/rights
needed to interpret it. This allows content and raster placement to differ while
retaining one document owner. Browser agent placement should not depend on the
renderer choice.

The default process boundary belongs before backend-specific lowering. Classic's
encoding and the sparse renderers' prepared data have different dependencies and
cache identities. CPU and GPU share Sparse Strips infrastructure; that does not
make prepared packets a stable wire representation. A later split after sparse
CPU preparation needs a versioned owned packet, resource remapping, bounds and
ordering, and should be justified by measurements.

Scene serialization alone is insufficient. Fonts, image sources, retained fragments
and external textures require resolved ownership. `SparseFragmentRegistry` says
its IDs are process-local and not a cross-process resource protocol; Classic IDs
are registry-local too. A receiver needs a resource namespace and epoch, or a fully
resolved bounded source snapshot. Caller-shaped text and its font palette must
stay coherent. Resolve resource authority before raster admission; a content ID
is not a resource grant.

The command execution interface carries `Command` and `Event`. It is a document-command
endpoint, not a generic renderer service. Renderer jobs need their own typed
scene/resource interface near Netrender, sharing lifecycle rules where useful.
Do not smuggle scenes through `Command::Script`, `Event::Message` or opaque strings,
and do not add Vello types to renderer-free `webcue`.

The [CPU followup](2026-11-01-cpu-render-jobs.md) now implements `SceneSnapshot`
and `RenderJob` near Netrender, plus a CPU raster operation and a nonblocking frame
inbox. Full snapshots carry font/image bytes; received Blob IDs are reminted before
renderer caches can use them. Resource grants remain with the embedding host.

## Scheduling and completion constraints

**Classic:** Cue already submits work and polls renderer-private quality counters
and error scopes on later turns. Acceptance, submission and a usable frame are
distinct events. Deferred results cannot be consumed by another renderer or a
replacement generation.

**GPU / Hybrid:** The pinned `HybridSession::render` encodes into a supplied
same-device target. Its contract requires submitting that encoder before the next
render call because provider queue writes update shared buffers. Keep this
render/submit ordering under one owner. Returning from `render` establishes neither
queue submission nor GPU completion. The API does not expose an independently
transferable CPU-preparation job.

**CPU:** `CpuSession::render` returns premultiplied RGBA pixels. A Cue adapter must
declare `Alpha::Premultiplied`, validate size/stride/byte budgets and establish the
encoding convention; it cannot inherit Classic's straight-alpha declaration.
CPU raster completion and later host upload/presentation remain separate. CPU
output avoids device sharing but has a frame-bandwidth and queue-memory cost.

Document canvas policy is separate from pixel alpha representation. The pinned
Genet top-level session paints an opaque Canvas under transparent author
backgrounds. A renderer must preserve that scene content; declaring straight or
premultiplied alpha does not authorize removing the document's canvas. Native
qualification checks top-level canvas behavior and raw rasterizer transparency
with separate fixtures.

Armillary can schedule admitted CPU jobs and own local actors. The embedding host
still owns processes/extensions, termination and budgets. Account for renderer
threads as well as actors and helpers; a full-core pool per surface would multiply
the intended concurrency. Bound in-flight scene bytes, font/image residency,
derived caches, output pixels and GPU allocations as well as message counts.
These workload limits are not an OS sandbox or total driver-memory guarantee.

Executor termination does not establish completion of submitted GPU work or end
exported-frame custody. Fences, import custody and device epochs remain with the
presentation/resource owner. Late frames still need installation, content revision,
viewport, allocation and device-lifetime validation.

Complete immutable render snapshots may be coalesced before execution under an
explicit policy. Retained deltas cannot be dropped without a replacement baseline.
Neither policy permits dropping accepted document commands or terminal events.
A failed renderer can be rebuilt from an authorized source snapshot under a new
render request; recovery must not replay script, navigation or document effects.

## Admission and fallback

Use the selected Netrender adapter's actual admission contract. Free sparse lowerers
and owned CPU/Hybrid sessions accept different resource subsets. The owned sessions
have bounded image/pattern/outline-text support but are not equivalent to Classic
for every operation. For example, the pinned Hybrid path refuses bilinear repeat
patterns that the CPU path admits. External GPU textures and resource-bearing
fragments need explicit resolution.

Host-selected fallback needs admission and visual/semantic parity for the actual
scene. Refuse unsupported operations; do not silently omit filters, change sampling
or transfer application authority. Renderer choice can change while document state,
selection and grants stay with their existing owner.

## Qualification after the command slice

1. The same Genet-prepared text snapshot has now rendered through Classic, CPU
   and Hybrid on the recorded host. Native fixtures assert canvas/text presence
   and record pixel differences. This bounded scene and typed-refusal evidence
   does not establish general parity or authorize automatic routing/fallback.
2. The CPU `FrameSource` slice is implemented with explicit premultiplication,
   encoding and byte budgets. A Genet text scene produced identical local/helper
   pixels, host upload/readback preserved them, and stale jobs were refused.
   Qualification currently covers scale-one scenes and the recorded fixture;
   the optional dependencies retain the coherent supplier revision.
3. The [Hybrid adapter](2026-11-01-hybrid-render-jobs.md) now owns encode/submit
   ordering and nonblocking readiness. Native tests cover device/queue/epoch
   identity, premultiplied geometry and images, typed sampling refusal, stale
   targets and renderer reconstruction. Physical device loss and driver reset
   remain separate qualification boundaries.
4. Qualify native frame transport for a separate GPU owner independently. A local
   texture or serialized scene does not prove OS handle transfer, synchronization,
   BrowserEngineKit integration or security isolation.

The CPU and Hybrid followups extend the accepted plan with opt-in features. Supplier revisions
remain unchanged. Backend selection and fallback continue to require host policy.

## Source basis

- Cue base: `c314b7e3a2b2df4da2d9b0387b0c64c93b6b0e4d`.
- Cue's locked Netrender: `66b2f72e9e0785819139739cc4503bf1e8766de9`.
- Inspected Netrender main: `11e208b84a981993df8077bb6025f8b3adbe2291`. The inspected
  scene, backend, session and fragment files have no diff from the locked revision.
  This review does not adopt the newer main.
- Coherent Vello provider: `276b46648c08f3d2062e70559afa999842e1873c`.
- [Provider renderer names](https://github.com/mark-ik/vello/blob/276b46648c08f3d2062e70559afa999842e1873c/README.md)
- [Backend admission](https://github.com/merely-made/netrender/blob/66b2f72e9e0785819139739cc4503bf1e8766de9/netrender/src/vello_backends.rs)
- [Owned CPU and Hybrid sessions](https://github.com/merely-made/netrender/blob/66b2f72e9e0785819139739cc4503bf1e8766de9/netrender/src/vello_backends/sessions.rs)
- [Source registry and ID scope](https://github.com/merely-made/netrender/blob/66b2f72e9e0785819139739cc4503bf1e8766de9/netrender/src/vello_backends/fragments.rs)
- [Current Cue rasterizer](../adapters/netrender/src/lib.rs)

The initial [command qualification](../receipts/2026-11-01-execution-host/README.md)
covered 33 tests and four Metal fixtures. The
[CPU qualification](../receipts/2026-11-01-cpu-render/README.md) adds owned-scene
worker/helper rasterization and host receiving. GPU/Hybrid admission and native
frame transport remain separate boundaries; the recorded CPU/Classic scene
comparison does not establish general backend parity.
The [Hybrid qualification](../receipts/2026-11-01-hybrid-render/README.md) adds the
same-snapshot three-backend comparison and local GPU-owner lifecycle evidence.
