# Owned scene jobs and CPU frames

Date: 2026-11-01. Status: implemented and qualified on native macOS.

## Boundary and ownership

`cue-netrender` exposes `scene_job` under its `scene-jobs` feature and the CPU
adapter under `cpu`. A host can use the scene contract without enabling Vello CPU.
Default features remain empty. Renderer types remain outside `webcue`.

`SceneSnapshot` contains a complete scene in the locked Netrender serialization
format, including font and image source bytes. Capture refuses registry-local
fragments, image keys without pixels, and native compositor surfaces. The host
resolves resource rights before capture. Snapshot bytes are source material,
not an authorization grant or a stable wire format across supplier revisions.

Netrender's capture format preserves Blob IDs for deterministic replay. Decoding
for a new executor instead mints fresh font/image Blob IDs and reconstructs the
reserved empty font sentinel. Renderer caches cannot treat an ID from another
process as their own resource identity. Image keys are local to the complete job.

`RenderLimits` bounds snapshot bytes, total font/image source bytes and frame
bytes. The receiver checks its own limits before handing off a job, even when the
snapshot was captured under larger limits. The CPU session additionally enforces
the provider's operation, image, text and curve-quality limits. These are logical
work/residency limits; parser scratch, allocator overhead and total process memory
remain outside them. Font data and decoded snapshots are trusted inputs.

## Execution and receiving

`CpuSceneSource` implements a raster-only `FrameSource`. Its capability report
advertises CPU RGBA presentation and refuses document, input and semantic controls;
the document owner retains those controls and its own event ledger. Ordinary
frame acquisition only takes an already completed frame. It performs no layout,
rasterization, I/O, thread joins, or GPU work.

```rust,ignore
let snapshot = SceneSnapshot::capture(&scene, limits)?;
let job = source.job(next_scene_revision, snapshot)?;
// The host schedules this CPU work on its chosen worker or executor:
let frame = rasterize(&job, limits)?;
source.complete(frame)?;
// Presenter can now acquire Frame::Cpu and upload it on its own device.
```

`rasterize` is an explicit CPU operation. The library creates no worker pool or
process launcher. A host can schedule it through Armillary, a thread, or a process.
The private proof uses both a worker thread and a supervised helper. The helper
receives the already prepared Genet scene and font bytes; it does not recreate
the document or rediscover fonts.

Each render uses a fresh owned CPU session. Sources and caches cannot accumulate
across jobs or depend on a previous job's image keys. This trades cache retention
for a bounded complete-snapshot contract; performance is not qualified here.
The host separately bounds in-flight jobs and executor concurrency.

Each job carries installation scope, a monotonically increasing scene-publication
revision, and a host-assigned allocation epoch. A new complete snapshot explicitly
supersedes the old job. Completion must match the current expected stamp exactly.
Wrong generations, superseded jobs and duplicate completions cannot replace
current pixels. Resize, hide and retirement invalidate pending jobs and queued
frames. A failed validation leaves the current job available for its valid result.
Rejected job admission does not consume the caller's revision.

Rendering refusal is returned to the host with a typed provider error. The host
can supersede the failed immutable render job with a new publication revision;
this does not replay document commands or authorize automatic backend fallback.
Executor supervision and resource custody remain host responsibilities.

## Pixel and scene conventions

CPU output is owned tightly packed RGBA8, `Alpha::Premultiplied`, `Encoding::Srgb`.
The frame's dimensions must match the endpoint viewport. `complete` validates
extent, stride, payload length, byte budget and pixel convention before installing
the frame. Frame acquisition transfers the pixels once.

This first CPU endpoint requires viewport scale one and an exact scene/viewport
extent match. Other scales receive an explicit unsupported result; no coordinate
conversion is inferred. Viewport changes and renderer recovery require a new
scene-publication revision, even when the underlying HTML is unchanged.

Scene colors must satisfy Netrender's premultiplied scene contract. The CPU
provider validates that contract; the adapter does not silently repair invalid
colors. A document's opaque canvas is scene content and remains independent of
the frame's alpha representation.

## Qualification and additional paths

The [qualification receipt](../receipts/2026-11-01-cpu-render/README.md) records
worker/helper pixel equality for a Genet-prepared text scene, CPU-frame upload and
readback, Classic rendering of the identical scene, stale-frame rejection, owned
images, and resource limits. The proof protocol names the exact Netrender schema
and bounds encoded frames independently of the command proof.

Hybrid can reuse the owned scene boundary while keeping its current CPU/GPU
session and encode/submit ordering with the GPU owner. Its adapter and native
frame-handle transfer remain separate qualification boundaries in the
[Vello placement review](2026-11-01-vello-execution-placement.md). These CPU jobs
establish neither general CPU/Classic parity nor BrowserEngineKit/OS isolation.
