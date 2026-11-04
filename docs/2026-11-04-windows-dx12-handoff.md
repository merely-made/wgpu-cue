# Windows DX12 renderer handoff

This slice maps the existing persistent renderer session onto a same-adapter
DX12 resource/fence handoff. The exact Git-pin native gates pass on the AMD Radeon 780M and NVIDIA RTX 4060
Laptop GPU on O-PC (Windows 11 Home Insider Preview, build 26220). The Graft pin is publicly available and a fresh Linux regression fetched it
directly from GitHub. The presence of an importer does not qualify other
Windows devices.

Graft owns shared allocation, native shape validation, COMMON-state transitions,
GPU fence ordering, and actual imported-resource destruction. Cue validates
scope, device epoch, monotonic frame stamps, metadata and bounded imports. The
private process fixture launches and supervises the helper, selects the same
adapter LUID, transfers capabilities, and associates releases with exact leases.
Genet retains document state in the host; only owned scene snapshots reach the
renderer process. The fixture renders through Netrender's Vello Hybrid adapter.

## Native ownership

Each allocation has an NT resource handle and shared DX12 fence. Positive odd
values publish producer copies; the following even value completes consumer use.
The producer copies into its own shared allocation and transitions it to COMMON
before signalling. The private process protocol observes genuine producer
completion before publishing Ready, so a later poisoned shared fence cannot
turn unfinished producer work into a valid frame. The receiving queue waits on that fence before using the
opened allocation. Sealing consumer use records a COMMON transition followed by
the even fence signal. Neither a fence alone nor peer exit authorizes reuse.

wgpu 30's DX12 HAL has no texture-drop callback. The importer attaches a private
IUnknown lifetime witness to the opened native resource, retaining no resource
reference in the witness itself. Its final Release records actual native-object
destruction, including the lifetime retained by texture views. An existing
witness is never overwritten. A release token requires sealing, observed even
GPU completion on an independent consumer fence, and destruction of that resource.
The shared producer fence can report UINT64_MAX after producer-device exit while
the consumer device and imported storage remain live. It is never used as the
consumer completion witness. The host then returns the
exact current lease; the producer independently checks the even fence value.

Microsoft documents the retained interface lifetime for
[SetPrivateDataInterface](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12object-setprivatedatainterface)
and the distinct roles of
[resource barriers](https://learn.microsoft.com/en-us/windows/win32/direct3d12/using-resource-barriers-to-synchronize-resource-states-in-direct3d-12)
and [queue fences](https://learn.microsoft.com/en-us/windows/win32/direct3d12/executing-and-synchronizing-command-lists).
Native queue operations require serialized host queue ownership.

## Private transport

The helper's anonymous stdin/stdout pipes belong to the spawned child. Packet
identity binds schema, scope, renderer incarnation and a system-random nonce.
The helper keeps its exported handles alive until exact release. Received
integers designate handles in that child, not the parent. The parent duplicates
from its owned child process handle into local RAII ownership, then opens the
resource/fence on its own device. No arbitrary parent handle is adopted.

Queues and framing are bounded. The existing renderer session owns admission,
terminal results and retirement. The two-slot pool owns lease generations.
Resize replaces only a free slot; same-size reuse preserves the shared kernel
allocation. Unexpected/stale replies fail; a replacement rejects an old lease.
This is a trusted-host proof, not a public wire ABI or process sandbox.

## Required gates

- Hardware DX12 with the debug layer required in both processes, and matching
  adapter LUIDs. Capture device error queues explicitly; an injected error must
  be detected by the collector before accepting clean native results.
- Local Hybrid versus separate-process byte equality for real Genet snapshots,
  including glyphs, two-slot pressure, resize, and same-size allocation reuse.
- A producer GPU queue deliberately stalled before publication completes:
  consumer readback must remain pending until the producer fence can signal.
- Sealing plus dropping the texture must not release a slot while a view lives.
  Retain a failing control with the lifetime predicate deliberately removed,
  restore the source, and repeat the native gate.
- Renderer termination with accepted pending work produces one terminal result;
  existing imports remain readable; restart rejects stale releases and preserves
  host document state.
- Locked dependency identities, strict targeted Clippy, and bounded regression
  gates on unaffected native platforms before publication.

Cross-adapter sharing, physical window presentation, frame pacing, sandboxing,
all browser shaders, and production deployment remain outside this fixture.

## Qualification record

Graft source: `d903cfacf6ee663ddcc31e14539448dae3794c48`, based on maintained
main `dea7c5f793ccc12e3aab06927e47d53ac4a58295`. Both descriptor and native
helper packages select this exact source. Vello `9f454193`, Netrender `79c9487b`
and Genet `82ef34d9` remain unchanged from the shader qualification cohort.

The exact Git-pin native gates ran three tests on each of two hardware adapters:

| Adapter | Driver | LUID within this boot | Native tests |
| --- | --- | --- | --- |
| AMD Radeon 780M Graphics | 31.0.14084.3001 | 0000000000013ae0 | 3 passed |
| NVIDIA GeForce RTX 4060 Laptop GPU | 32.0.16.1088 | 000000000001262f | 3 passed |

Three proof unit tests include actual D3D12 error and unexpected-warning
injection, nonblocking local admission, and framing refusal. Three CPU stream
and nine renderer-session regressions also pass. Targeted all-target Clippy
passes with `-D warnings` on Windows and macOS. The Mac exact-pin
regression passes five Hybrid, five Metal handoff, and three CPU stream tests.

The collector rejects all error/corruption messages and unexpected warnings.
It retains two warning classes also observed in the direct local Hybrid
control: ID 1361 (ignored sampler comparison function) and ID 820 (unoptimized
clear value). These are allowed only at warning severity; no errors are hidden.
There is no claim of zero diagnostic messages.

Three real fault controls execute nonzero tests and fail with exit 101:
removing the producer queue wait, removing the final-resource lifetime
predicate, and polling the producer fence instead of the independent consumer
fence. Each control restores the original source bytes and verifies its hash.
The initial native fixture independently exposed the producer-exit fence
failure before the independent consumer-fence fix.

Evidence root: `/Users/markik/Code/output/cue-dx12-handoff/` (native logs and
control source snapshots copied from O-PC; receipt budget 100 MB on Windows).
Windows compiler artifacts use `C:/t/cargo-targets/wgpu-cue`; no separate
Windows worktree or Cargo home was created. Source-bearing Mac worktrees are
retained through publication/integration.
The fresh Linux gate on T-PC passes two native Vulkan handoff tests, four
proof unit tests, three CPU stream tests and nine renderer-session tests.
Strict targeted all-target Clippy also passes. The AMD Radeon Graphics
(RADV RENOIR, Mesa 26.2.5) run loads the Vulkan validation layer and records
zero validation diagnostics. Linux and Metal native implementations are unchanged.

The initial exact-pin Windows and Mac checks supplied the Graft source through
per-command local Git transport mirrors before publication. Those are Git source
checks, not Cargo path patches. The subsequent Linux gate fetched the published
Graft revision directly from GitHub with the same locked identities. The lockfile
contains 565 packages; only the two Graft Git source records and the proof package
Windows dependency edge change. All 557 other supplier records are unchanged.
The identical Mac/Windows/Linux lock SHA-256 is
`c0d5eb110b5be6b3c53b119b68ed52b452b5314f8b08577c79900c3809827f17`.

The implementation is Cue commit `4c8e8ecff2286cb3eb0d658cfc63d6ee107e6d3e`;
the following documentation-only commit records the fresh Linux qualification.
