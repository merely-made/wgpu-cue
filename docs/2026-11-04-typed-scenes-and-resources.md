# Typed scene delivery and renderer resource tables

`SceneSnapshot::capture_owned` adds local delivery of an owned, immutable scene.
The existing complete byte-snapshot path remains available as a correctness and
performance control. Native persistent helpers additionally use a bounded
resource table scoped to one exact document scope and renderer incarnation.
Neither change alters Graft allocation, readiness, final-view custody or GPU
release witnesses. Supplier revisions remain unchanged.

## Typed local validation and custody

Capture first applies the existing frame-size, closed-resource and source-byte
checks. Native compositor surfaces, registry-local fragments and unresolved image
keys remain refused. It mints process-local Blob IDs while retaining the original
immutable Arc backing buffers; it never adopts supplier-chosen IDs into a cache.

The resource-free scene structure still passes through the exact pinned serde
format and parser. This preserves malformed-field/non-finite-value rejection,
including the format's explicit no-clip sentinel representation. Font/image
payloads are represented by empty buffers for that validation, and remain owned
by the original typed scene. This is typed delivery with structural validation;
it does not eliminate every JSON operation or all scene metadata cloning.

Admission continues to charge the exact size of the complete resource-bearing
JSON snapshot. The pinned Blob serializer emits numeric byte arrays; their
decimal digits and separators are counted without writing/parsing those arrays.
A checked `4*n` bound makes that digit sum overflow-safe. The same snapshot,
resource and expected-output budgets are enforced by render sessions and stricter
receiving limits. Aliases still count toward the existing per-scene resource
budget; table deduplication does not relax job admission.

Typed decode checks receiving limits and clones scene metadata while sharing its
immutable resource owners. IDs are minted on typed capture, so decoded clones of
one immutable snapshot share those identities. Byte decode continues to mint new
IDs on every decode. Calling `bytes()` explicitly materializes the complete
snapshot lazily; normal typed local delivery and admission never do so. The lazy
encoding is checked against the exact accounted size.

## Per-renderer resource protocol

The shared adapter owns `ResourceSender`, `ResourceReceiver` and `ResourcePacket`.
The private transports own framing, ordered queues, process startup and native
frame transfer. No platform IPC or process launch policy enters the resource table.

Each table is bound to `(scope.owner, scope.generation, incarnation)`. Packets
carry a strictly consecutive sequence, resource-free scene structure, font/image
references, and additions for previously unseen immutable payloads. Resource
equality compares actual bytes; foreign Blob IDs and hash-only equality do not
authorize reuse. The receiving table mints its own process-local identities.

A sender produces a candidate next state. Each native transport installs that
candidate only after its ordered send queue accepts the exact encoded packet.
Busy or encoding failure leaves the sender unchanged. Receive validates namespace,
sequence, structural shape, complete bindings, additions and all budgets before
committing additions or sequence. Replay, old incarnations, unknown references,
duplicate/redefined/unreferenced uploads and duplicate image bindings fail.

Tables retain at most 256 distinct payloads and at most the configured resource
byte budget (32 MiB by default). They do not evict or reuse tokens. Exhaustion
refuses the work; the host can replace the renderer incarnation explicitly.
Reference counts are bounded by the serialized structure, independently of the
unique-resource limit. A scene with 300 image aliases to one four-byte resource
therefore retains one table entry, while still charging all aliases to its
per-scene resource budget.

Snapshots retain Arc owners independently of the table. Dropping a table or
replacing its incarnation does not invalidate retained snapshots. Native frame
slots keep their existing independent lease/fence/view lifetime rules. A new
incarnation starts with an empty table, requires its own additions and rejects
the old incarnation's packets. No persistent journal, automatic replay, global
resource authority or hostile-parser sandbox is added.

Persistent Metal startup now sends dimensions without a redundant font-bearing
seed snapshot. Its legacy `Acquire` fixture retains an explicit seed snapshot
for the existing fault and lifetime controls. All three native persistent
transports use resource packets for typed snapshots and the complete byte codec
for byte controls. The one-shot CPU raster and persistent CPU transport retain
their byte protocol; their regression gates are rerun. Host/helper binaries for
this private protocol must be built together.

## Controls and qualification

The no-copy regression checks exact full-wire accounting, shared backing pointer,
fresh process-local identity, lazy encoding and resource lifetime after snapshot
drop. It also checks malformed-field/closure rejection and stricter receiving
budgets. Selecting the real legacy byte capture in that same test causes one
asserted failure, retained with a binary hash and log.

Table tests cover once-only transfer, retry without sender commit, content
deduplication across different backings, lifetime after table drop, failed-packet
atomicity, replay, replacement, unknown/duplicate references and cumulative byte
pressure. An initial receiver reference-count check incorrectly rejected 300
aliases of one payload. The new regression genuinely fails before that check is
fixed, then passes. The failing source archive and log are retained.

Native pixel comparisons use the complete byte path as the local control and
typed/table delivery in the helper, including changed content, resize, held
imports, producer termination and replacement. An additional Metal test compares
typed local and byte outputs exactly for real Genet glyphs, images and alpha.
Existing native producer readiness and final-view release checks remain active.

All 16 final receiving-window workloads complete without runtime errors and
finish with zero imported frame slots. Every process run stays within the two-slot
bound. **The fresh-content 60 Hz gate remains false in every run.** These are
single paired runs of the same fixture and build features, not a statistical
hardware ranking. Values below are steady-phase medians. Preparation includes
retained document work plus snapshot capture; content intervals use OS feedback.

| Hardware / placement | Prepare byte → typed (ms) | Fresh-content interval byte → typed (ms) |
| --- | ---: | ---: |
| Mac / Metal, local | 56.29 → 7.00 | 116.70 → 33.34 |
| Mac / Metal, process | 58.39 → 7.01 | 183.39 → 66.69 |
| Linux / Vulkan, local | 49.40 → 6.79 | 99.99 → 33.33 |
| Linux / Vulkan, process | 48.93 → 6.78 | 149.99 → 66.66 |
| Windows AMD / DX12, local | 46.31 → 5.88 | 86.24 → 21.63 |
| Windows AMD / DX12, process | 51.14 → 5.81 | 156.97 → 54.62 |
| Windows NVIDIA / DX12, local | 49.85 → 6.35 | 91.45 → 25.10 |
| Windows NVIDIA / DX12, process | 50.50 → 6.56 | 151.39 → 54.64 |

Windows timestamps are reconstructed from DXGI counters. Three typed runs exceed
the required coverage; NVIDIA local typed and all four byte controls do not.
Their observed content intervals are partial samples. The compact receipt keeps
coverage fractions, flags, counts, loaded-phase/tail distributions and raw hashes.
Mac and Linux runs exceed the coverage threshold. Repeating retained textures is
not counted as fresh content, and no photon/input-device latency is measured.

The typed process tables contain two font payloads: Mac 1,524,220 bytes across
514 committed packets, Linux 1,592,256 across 513, Windows AMD 2,035,964 across
565, and Windows NVIDIA 2,035,964 across 574. Byte controls leave their tables
empty. These statistics describe successful ordered delivery in the fixture;
there is no automatic replay or durable exactly-once guarantee on process loss.

Final correctness gates execute 43 positive tests on Mac, 28 on Linux and 39 on
Windows (110 test executions total), including CPU transport regressions. Strict
all-target targeted Clippy passes on all three. Linux explicitly loads the Vulkan
validation layer and records zero native validation diagnostics. Windows retains
only its documented warning baseline, IDs 1361 and 820 at warning severity;
errors/corruption and unexpected warnings remain rejected.

The faster source4 Mac process run also exposed a release-order race during
retained-view custody. The host polled a valid Released acknowledgement into its
response queue but kept the old slot in its offer ledger until polling finished.
A following Offer for the same freed slot was therefore rejected as occupied.
The original failed run and frozen binaries remain a genuine pre-fix control.
Source5 applies the exact validated acknowledgement while consuming messages,
before checking the next Offer. A pending exact-lease marker rejects unsolicited,
stale or duplicate acknowledgements. Offer bounds, final-view custody and GPU
completion requirements remain intact. The native persistent fixture now delays
polling by 100 ms after release so the acknowledgement and replacement offer batch
together; it passes along with the full post-fix Mac matrix.


The window fixture keeps the prior 120-section document, fonts, viewport sizes,
15-second measured phases, two stress threads, retained-view phase and drain.
`--byte-snapshots` selects the byte control independently of `--local`.
Each table statistic is `[entries, retained_source_bytes, committed_packets]`.
The tuple records the sender's incarnation-owned table, retained until its
transport object is dropped; it is separate from final imported frame-slot count.

The table removes repeated font-payload transfer and resource-bearing JSON
parsing. Structural encoding/validation and renderer preparation remain. In
particular, image/text cache clears in `HybridSceneSource::submit` are unchanged;
no GPU/glyph-cache speedup is claimed from buffer/table reuse alone.

## Reproduction and evidence

Build both helper and receiver with `window-receiver,scene-profile,cpu-raster`.
Use the mandatory Code workspace build-storage launcher on this Mac and the
existing stable lane. Run the prior receiving-window command with `--local`
for typed local, without it for typed process, and add `--byte-snapshots` to
either for its byte control. Use fresh absolute output paths and a graphical
session. Mac uses Arial; measured Windows/Linux fixtures use native sans-serif.
`CUE_SCENE_PROFILE_DIR` or the Mac bundle's `profile-directory.txt` enables
per-process stage logs, as in the prior profiling slice.

Raw evidence is retained at `/Users/markik/Code/output/cue-typed-resources/`.
`source4.json` identifies Linux/Windows measurements; `source5.json` includes
the final Mac-only release-order repair. Final source5 builds on Linux/Windows
produce executables byte-identical to their measured source4 binaries, and their
strict Clippy gates are refreshed. Mac measurements use source5 frozen apps. Earlier source
attempts and controls are preserved separately. The [compact receipt](../receipts/2026-11-04-typed-resources/summary.json) contains raw
and executable hashes, workload status, table statistics and pacing distributions.
Receipt caps are 600 MB on Mac, including APFS-cloned apps, and 60 MB on each
remote. No new Cargo home or compiler-cache lane was created.

All 565 locked package identities remain unchanged. Only `cue-netrender` gains
a direct edge to the already-selected serde package; the other 564 package
records are identical. The changes qualify this private same-adapter Hybrid
fixture, with CPU transport regressions separate from native presentation.
Classic, full browser workloads, arbitrary GPUs, process sandboxing and physical
input/photon latency retain their separate qualification requirements.
