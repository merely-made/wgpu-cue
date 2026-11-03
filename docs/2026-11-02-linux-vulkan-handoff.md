# Linux Vulkan frame handoff

Status: accepted; native handoff qualified on T-PC with the Hybrid shader
validation limitation below.
Date: 2026-11-02

## Context

The host can run the Genet/Hybrid renderer in a separate Linux process without
moving document authority or native allocation mechanics into the engine. This
extends the persistent render-session contract with an actual owned DMA-BUF
transport and a two-slot native pool.

## Decision

Graft owns exportable storage, Vulkan image creation/import, DRM layout,
queue-family transitions, GPU completion, and final-reference release. Cue owns
scope/device binding and monotonic publication stamps. The host owns process
launch, renderer incarnation, framing, sender validation, capability transfer,
and acknowledgement routing. Genet retains the document and produces complete
owned scene snapshots. Netrender owns Hybrid rasterization on the supplied device.

The first supported allocation is linear single-plane RGBA8, with explicit DRM
modifier zero. Both devices require Vulkan DMA-BUF, DRM modifier, external memory
fd, and foreign queue extensions. Graft queries the exact format/usage/handle
combination for importability and exportability; unsupported adapters refuse the
path. A successful CPU or software Vulkan fixture is insufficient qualification.

The producer copies its ordinary Hybrid output to export storage, releases that
image to FOREIGN/GENERAL, and observes completion before publishing a ready
message with an owned descriptor. CPU completion messages provide execution
ordering; the native barriers provide availability, visibility, and ownership.
Ownership transfers are layout-neutral GENERAL-to-GENERAL on both endpoints;
local barriers perform the transfer-destination, general, and sampled-layout
changes separately. The consumer acquires FOREIGN/GENERAL before exposing a normal sampled/readback
wgpu texture. No CPU pixel payload crosses the normal frame transport.

A sealed consumer frame is releasable only after its queue fence and the final
wgpu texture/view destruction are observed. Graft then submits the native
consumer-to-FOREIGN release and waits nonblockingly for that submission's
completion before returning the release stamp. Before the layout-neutral
ownership release, a local transition uses UNDEFINED as its old layout: every
consumer alias has gone, its pixels may be discarded, and the producer always
overwrites the whole image on reuse. This avoids assuming a final wgpu layout
after arbitrary legal consumer reads.

The producer accepts an acknowledgement only for the exact occupied slot epoch,
publication allocation/revision, and renderer identity. Peer death never provides
that witness. A replacement renderer gets a fresh pool and incarnation; the
host's document-owned RenderClock continues increasing across the replacement.
Old imported storage can remain alive independently of either renderer process.

## Alternatives and consequences

FOREIGN ownership is the conservative boundary shared with the existing Linux
importer. Vulkan also permits the narrower EXTERNAL family for compatible
physical-device/driver identities. That can reduce transfer cost, but would add
UUID negotiation and compatibility policy to this first fixture. It remains a
separate optimization after the conservative path is qualified.

Completed-frame messages use host-observed queue completion, keeping native
synchronization handles out of the initial wire contract. External semaphores can
later reduce CPU involvement. This implementation must preserve its actual
ownership barriers whichever synchronization mechanism the host selects.

Single-plane linear RGBA8 keeps export and import descriptors identical and
makes refusal predictable. It costs an extra GPU copy from the ordinary Hybrid
output and may use more bandwidth than a negotiated tiled modifier. Multi-plane,
cross-device, and modifier negotiation require their own capability evidence.

## Host transport

The private proof uses inherited Unix sequenced-packet sockets for bounded
control messages. SCM_RIGHTS carries actual file descriptions, MSG_CMSG_CLOEXEC
adopts them without an exec inheritance window, and SCM_CREDENTIALS validates
kernel sender PID/UID. Full scene jobs use a bounded stdin writer bridge and the
existing pinned private scene codec. Neither this wire format nor this process
launcher is a production sandbox or a supported public IPC protocol.

## Evidence and limits

The native acceptance test compares local and process Hybrid pixels from actual
Genet scenes, holds both slots, holds a texture view past queue completion,
resizes only a released slot, reuses an allocation at the same extent, replaces
the renderer within the same document scope, reads held storage after producer
exit, rejects an old release in the replacement, settles an accepted pending job once
after renderer loss, and checks document state.
Readback exists only to measure the fixture output.

The measured host is T-PC's AMD Radeon Graphics (RADV RENOIR), Mesa 26.2.5,
Rust 1.99.1, with Khronos validation layer 1.4.341 and synchronization validation
enabled. Graft's allocation/import/release test passes with no reported Vulkan
validation errors. Removing the final-reference gate makes its retained-view
assertion fail; restoring the exact source passes again.

The full Hybrid fixture reports `VUID-StandaloneSpirv-None-10684` during shader
module creation: Function storage class variables have explicit Offset layout
decorations. The adjacent `local_hybrid_pixel_control` reproduces this without
process launch, export allocation, or DMA-BUF import. Pixel, ownership, lifetime,
and restart checks pass, but the Hybrid shader pipeline is not validation-clean.
The diagnostic remains visible and is recorded with the fixture results; this
slice does not alter Naga, Vello, Netrender, or Genet pins to address it.

The local macOS regression gate executes all 74 existing tests. Linux gates cover
strict Clippy, kernel descriptor custody/sender checks, the native storage and
process cases, nine common session cases, and three persistent CPU cases.
The stable local lane is `cue-execution-host`; T-PC reuses its existing
`cue-render-session` cache. Receipts and immutable inputs are retained under
`Code/output/cue-vulkan-handoff` on both hosts.

Vello Classic and CPU continue using the existing scene/session vocabulary. This
slice does not claim a separate-process Classic GPU allocator, Linux display
presentation, cross-device DMA-BUF compatibility, optimized DRM modifiers,
external semaphore support, or a Windows DX12 implementation.

References: [Vulkan queue ownership](https://docs.vulkan.org/spec/latest/chapters/synchronization.html#synchronization-queue-transfers),
[image layout transition rules](https://docs.vulkan.org/refpages/latest/refpages/source/VkImageMemoryBarrier.html),
[external-memory allocation](https://docs.vulkan.org/spec/latest/chapters/memory.html).
