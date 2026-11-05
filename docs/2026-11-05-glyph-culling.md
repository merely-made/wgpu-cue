# Sparse glyph culling and cross-platform rendering

Netrender's sparse text sessions now skip drawing positioned glyphs whose
validated outline bounds cannot reach the target. Cue adopts it through targeted
supplier revisions directly above the font-admission cohort. Admission, budgets,
outline preparation, cache epochs, production cache clearing, snapshot ID
rebinding, GPU submission and native frame-slot custody are unchanged.

## Culling boundary

`TextState::plan` decides visibility after each glyph has passed its ID, outline,
finite-coordinate, segment-budget and derived-transform range checks. A culled
glyph still counts against positioned-glyph and frame-segment budgets, and its
outline is still prepared and retained. An invalid glyph outside the target still
refuses the frame. Face admission and identity-collision refusal are untouched.

The test rectangle is the viewport intersected with the run's device-space clip.
Sparse sessions refuse layer and backdrop filters, so no admitted operation
samples pixels outside the target. Bounds are the control-point box of the
unhinted outline at the run's variation location, mapped through the same affine
the range check uses. The rejection margin is two device pixels plus a 1e-6
relative allowance. A fully culled run still builds an empty Glifo run, so
render-time font errors surface as before. `SparseTextStats::culled_glyphs`
reports the count; the private profiler records it per frame.

Netrender's new CPU test renders a fixture straddling every edge at subpixel
offsets, rotated, under a rounded run clip, at 3.5 px and 41 px and at a
non-default variation location, and requires byte-identical pixels with culling
disabled. It fails when the margin is made negative. Other new tests cover
culling after every check, budgets counting culled glyphs, off-target refusal and
fully culled runs.

## Paired measurements

Before is the frozen font-admission build from the previous round; its hashes
were verified on every host. After is this build. Each host runs three paired
fresh-process repetitions of all 48 cases (40 frames, eight warmup), before and
after interleaved, with policy order and pair order reversed in the second.
Every frame of both builds compares complete RGBA bytes against the per-adapter
old-renderer references exported in the font-admission round.

Median session-render CPU time in milliseconds, pooling non-warmup frames across
the three repetitions:

| Host / backend | Production clearing: before → after | Reduction | Retained-text control: before → after |
| --- | ---: | ---: | ---: |
| Mac / Metal | 16.26 → 11.54 | 29.0% | 16.12 → 11.13 |
| Linux / Vulkan | 13.03 → 7.95 | 39.0% | 12.79 → 7.91 |
| Windows AMD / DX12 | 16.23 → 11.58 | 28.7% | 16.40 → 11.25 |
| Windows NVIDIA / DX12 | 16.76 → 11.24 | 32.9% | 16.14 → 11.20 |

All document and mixed cases fall 23–41%. Geometry-only and image-only controls,
which carry little or no text, move between −13% and +20% around sub-3 ms
medians, consistent with run noise at that scale. The fixture culls 91.5% of positioned glyphs (93.2% on Linux, whose
system sans-serif lays out differently).

The Mac table uses the second of two complete sets. During the first set other
workspace builds drove the load average to 10–56; its no-text geometry control
read 28% slower and the document case only 14% faster, so it is retained in the
receipt but not used. The second set ran at load 6–10.

Correctness holds on all four adapters: 46,080 full-pixel comparisons, all
exact, plus another 11,520 from the second Mac set. For every case, iteration and
variant, before and after match on glyph and op counts, outline preparation,
cache resets, font count/bytes, cached glyphs, image hydration/upload/residency
and the frame checksum.

## Native windows

All 16 window workloads complete with no runtime error, stay within the
two-import bound and drain to zero imported frame slots. The fresh-content 60 Hz
gate remains false in every run.

| Host / placement | Median steady new-content interval before → after (ms) |
| --- | ---: |
| Mac / local | 33.34 → 16.67 |
| Mac / process | 66.69 → 66.69 |
| Linux / local | 33.33 → 16.67 |
| Linux / process | 66.66 → 50.00 |
| Windows AMD / local † | 18.74 → 16.66 |
| Windows AMD / process | 48.70 → 48.56 |
| Windows NVIDIA / local † | 18.65 → 18.04 |
| Windows NVIDIA / process | 54.61 → 54.44 |

† After-AMD and both NVIDIA local windows fall below the DXGI feedback coverage
threshold; their intervals are partial samples. Mac and Linux local windows now
reach a 60 Hz median, but their tails still miss the gate. Cross-process
placements remain bounded by the larger pipeline.

## Gates and scope

Netrender: 106 library tests on main and the CPU/Hybrid text integration suite
(10) on both main and the targeted revision. Cue on the targeted cohort: 48 Mac
tests including the ignored GPU/Metal tests, 25 Linux tests including the Vulkan
fixture with validation loaded and zero diagnostics, and 39 Windows tests
including DX12 handoff on both adapters. Strict targeted Clippy passes on all
three, with no Windows diagnostics.

The lock moves 26 source records to Netrender `eb833a26` and Genet `de14e67c`
(manifest pins only, above `aead381`). Package versions and the Vello, Graft and
wgpu cohort are unchanged. The [compact receipt](../receipts/2026-11-05-glyph-culling/summary.json)
holds per-case distributions, binary hashes, load records and window summaries.
Raw evidence is under `Code/output/cue-glyph-cull/` on each host.
