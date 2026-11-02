# ADR: Persistent renderer sessions behind Cue

**Status:** Accepted and implemented
**Date:** 2026-11-02
**Decider:** Repository owner

## Context

The command session preserves document request obligations across execution loss.
The Metal proof establishes native capability transfer and exported-frame custody.
Neither originally exercised changing scenes through one renderer endpoint. The
host must be able to replace a renderer without replacing its document or inferring
GPU release from process termination. Windows and Linux need the same lifecycle
vocabulary with their own native resource-transfer implementations.

## Decision

`cue-netrender::render_session` owns typed render admission and outcomes.
`RenderTransport` is supplied by the host and may return owned CPU frames,
host-device textures or owned native frames. Its polling methods do not wait for
pipe I/O, raster work, GPU completion or executor shutdown. Encoding bounded input
still has CPU cost. Launch, scheduling, platform transport and resource authority
remain with their existing owners. Document commands are never render jobs.

A surviving host owns `RenderClock` for one installed document scope. The clock
assigns monotonic publication and allocation stamps across renderer incarnations,
including rerendering an unchanged document. These are render publication counters,
not canonical document mutation counters. Each endpoint also has an `ExecutionId`;
a new renderer incarnation does not reset the clock or require a new document scope.
Backend allocation counters and native slot lease IDs remain separate. Host adapters
map their outputs onto the accepted publication stamp.

`RenderSession` reserves a terminal outcome before sending. Count and logical byte
budgets include pending work and completed outcomes awaiting consumption. The byte
charge conservatively includes encoded snapshots, resource source bytes and expected
output pixels. It is not total CPU/GPU RSS, driver allocation accounting or a bound
on frames retained by a consumer after delivery. Backpressure consumes no admission.
Transport Busy retains accepted jobs. Outcomes are Frame, Refused, Cancelled or
ExecutionLost; malformed, duplicate or foreign-incarnation replies close admission
and settle outstanding jobs. There is no automatic replay or snapshot coalescing.
Retirement settles outstanding jobs but does not establish process death or GPU
completion. Stop evidence is observed separately and normal exit remains normal
exit after cleanup. Completed frames retain their originating execution identity.
The host routes only its active renderer's publications to presentation.

The private CPU proof supplies a bounded cooperative worker and a persistent child
with bounded writer/reader bridges. The existing fresh `CpuSession` per complete
snapshot is retained for cache isolation. The process persists across jobs. Drop
kills/reaps children and joins bridge threads; ordinary transport methods never
join. An explicit started-work witness precedes the stalled-child kill test.

The Metal helper now accepts full scene jobs over its bounded input bridge and
rasterizes each job through its persistent Hybrid session. It has two leased
IOSurface slots. A slot's backing changes extent only after that slot is free.
If both slots are held, a resized job waits; releasing one slot permits fresh
storage while the other imported frame remains held. Ready follows GPU-copy
completion. Consumer release still requires a sealed queue fence and final texture
and view destruction. A render terminal outcome does not release that storage.
The private wire schema is v2 and binds the incarnation as well as nonce/scope/PID.

## Options considered

| Option | Complexity | Coupling | Assessment |
| --- | --- | --- | --- |
| Remotely callable `Surface` | High | Document controls, rendering and platform lifetimes | Too broad for this boundary; VM-local values and fine-grained calls need separate treatment. |
| Typed scene jobs and host-supplied endpoint | Medium | Owned Netrender input; concrete output custody | Adopted. Reuses the existing command and frame ownership cut. |
| Backend-specific prepared packets | High | Provider caches, resource IDs and versioned lowering | Requires a separately measured and qualified boundary. |

## Three renderers and platform placement

Classic, GPU/Hybrid and CPU share scene-job/lifecycle vocabulary, not identical
admission or pixel conventions. Classic retains its GPU owner and straight-alpha
output. Hybrid retains encode/submit ordering and premultiplied output. CPU returns
owned premultiplied pixels. Local Classic and Hybrid outputs exercise the same
session contract; CPU exercises local and process endpoints. The two-process GPU
fixture currently uses Hybrid. There is no automatic fallback or general visual
parity claim.

The shared endpoint adds no Mach, Windows or Linux handles. Graft retains native
handle import and synchronization. Windows and Linux hosts must supply their own
capability transfer, producer readiness and consumer-release implementations before
claiming native process-frame qualification. Running CPU process tests on Linux
qualifies portable lifecycle and CPU execution, not a Vulkan frame handoff.

## Consequences and validation

Changing Genet snapshots can run through one process, resize, retain older frames,
and replace the renderer while preserving document state and scope. Hosts must
retain the publication clock across replacement and independently bound consumer
frame custody. They also supply resource grants and bind transport endpoints to
execution identity; identities themselves confer no authority.

Validation includes admission/byte reservations, transport backpressure, duplicate
and stale replies, explicit backend refusal, exactly-once terminal outcomes,
retirement, natural versus forced stop evidence, and same-scope renderer replacement.
The Metal fixture additionally checks changing pixels, resize blocked by two held
slots, resumed rendering after one release, retained storage across process death,
and rejection of an old release by the replacement renderer. Native GPU evidence
remains macOS/AMD Radeon Pro Vega 56; display presentation, device reset,
BrowserEngineKit extensions and Windows/Linux GPU transfer remain unqualified.

Receipts and source snapshots are retained in
`/Users/markik/Code/output/cue-render-session/`. The Linux lane uses an immutable
source snapshot under `/home/markik/Code/output/cue-render-session/` and a stable
cache under `/home/markik/Code/targets/lanes/cue-render-session/` with two build jobs.
No supplier version or lock record changes are part of this slice.
