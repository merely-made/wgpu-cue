# Hybrid scene jobs on the host device

Date: 2026-11-01. Status: implemented and qualified on native Metal.

## Placement and ownership

The opt-in `cue-netrender/hybrid` feature exposes `HybridSceneSource`, a raster-only
`TextureSource` on the embedding host's existing device and queue. It consumes the
same complete `SceneSnapshot` used by CPU jobs. Hybrid can be enabled without CPU;
the default feature set remains empty. The private proof's `hybrid-raster` and
`cpu-raster` features select their respective adapters independently.

The pinned provider still combines CPU preparation and GPU raster/compositing in
one `HybridSession`. This adapter keeps that session, its encoder, target textures
and submission with one GPU owner. A content producer can prepare the owned scene
elsewhere. No device reference or prepared provider packet becomes a process wire
value. Font/image grants remain host decisions before snapshot capture.

The trusted constructor supplies a device and its associated queue. Later calls
must match both identities, the device epoch and surface viewport. The adapter
creates the encoder and target internally; callers supply neither GPU object.
Identity checks do not certify arbitrary device/queue pairings as a GPU sandbox.

## Explicit submission and nonblocking readiness

Construction initializes provider GPU state under error scopes. The host polls
`poll_idle` until initialization scopes have resolved. `submit` then validates a
new scene-publication revision, the snapshot/resource/frame budgets, the device
limit and the scene/viewport extent. This first path requires scale one.

`submit` performs CPU preparation, encoding and immediate queue submission. It
submits its encoder before returning, satisfying the pinned provider's requirement
to submit before another render mutates shared buffers. CPU preparation is bounded
by logical provider limits; no latency or performance bound is claimed.

At most one submission remains pending. Another `submit` returns backpressure
until the host explicitly polls completion. `TextureSource::render` only polls;
it never decodes another scene, encodes another draw, submits or waits for GPU
completion. Ordinary controls and event polling perform no GPU work.

Readiness requires a queue-completion callback and resolved GPU error scopes.
Scopes cover constructor work, target creation, provider encoding, encoder finish
and queue submission. A GPU validation failure becomes a sticky renderer fault and
cannot publish a texture. The queue callback covers earlier shared-queue work as
well, so readiness may conservatively wait for unrelated prior host submissions.

The returned target is same-device Rgba8Unorm with premultiplied sRGB pixels.
Presentation uses the existing host-device route; no import or CPU readback is
inserted into normal rendering.

## Invalidation, refusal and reconstruction

Resize, hide and retirement invalidate delivery immediately. Pending work remains
owned and pollable until its completion callback and scopes resolve. Hide/show or
resize/back cannot revive its old target. Retirement closes controls and rendering;
`poll_idle` remains available to observe the retained submission.

Provider refusal preserves the requested operation and returns its typed error.
There is no sampling downgrade or backend fallback. A late provider error can
follow staged image uploads, so the adapter conservatively flushes those writes
with a non-publishing cleanup submission and retains its fence. A refused revision
remains available after cleanup. `submitted_count` counts admitted draws, excluding
these cleanup fences. Preflight refusal, such as a foreign context or source-byte
budget error, admits no draw.

Complete snapshots replace source palettes. The adapter clears owned image and
outline caches between jobs after prior work has drained, while reusing provider
pipelines and renderer resources. Limits cover logical sources, derived image
payloads and individual frame bytes, not total GPU/driver memory. One in-flight
target and a completed target may coexist; the presenter owns delivered textures.

A changed device epoch is stale for the existing source. The host reconstructs
the renderer with a new presentation installation and authorized scene snapshot;
document state and commands remain with the existing document owner. Native proof
covers a logical epoch change on the same physical device, explicit foreign-device
refusal and renderer reconstruction. Physical device loss and driver reset remain
separate qualification boundaries.

Executor shutdown does not cancel queued GPU work or end exported-frame custody.
Native handle transfer and platform-specific GPU process supervision remain with
the host and Graft adapters.

## Qualification

The [receipt](../receipts/2026-11-01-hybrid-render/README.md) records a real Genet
text scene, premultiplied transparent geometry and owned images, single-flight
backpressure, no redraw during polling, stale-job refusal, typed bilinear-repeat
refusal, resource limits and renderer reconstruction while the document remains
available. With both adapters enabled, the identical snapshot is also rendered
through CPU and Classic for a bounded canvas/text comparison.

The [placement review](2026-11-01-vello-execution-placement.md) retains the three
renderer choices and the independent native-frame transport boundary. These local
fixtures do not qualify general backend parity, cross-process GPU handles,
BrowserEngineKit integration or OS isolation.
