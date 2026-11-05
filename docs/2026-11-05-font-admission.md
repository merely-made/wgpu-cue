# Distinct font admission and cross-platform rendering

Netrender now admits each distinct immutable font backing allocation and
collection face once per frame. It retains identity-collision refusal and all
per-run/per-glyph checks. Cue adopts the fix through targeted supplier revisions
above its existing cohort. Production text/image cache clearing, snapshot ID
rebinding, admission budgets, GPU submission and native frame-slot custody are
unchanged.

## Admission boundary

`TextState::plan` builds two private frame-local records. An identity index checks
each distinct `(Blob ID, backing allocation)` against a previously accepted
canonical backing for that ID. Different allocations with the same ID still
compare actual bytes and reject differences. Repeated uses of the same live
immutable Arc allocation need no repeated full-buffer comparison. Scene and
retained-state owners keep those allocations alive throughout the plan; allocation
keys never leave the process or persist beyond the frame.

A separate `(backing allocation, collection index)` record caches parsed FontRef,
units-per-em and glyph count after validating head/maxp, unsupported color/bitmap/
SVG tables, variation-axis count and supported outline format. Retained renderer
IDs do not bypass face admission in a later frame. Each run still resolves its
own variation location and validates size/transforms. Each positioned glyph still
checks its ID, outline/derived bounds and aggregate segment work. Outline cache
keys, preparation, cache epochs and commit semantics are unchanged.

Font count/byte accounting continues to use the existing ID-plus-face FontKey.
Sharing one physical backing under different IDs does not collapse that budget.
An equal-byte alias in another allocation remains a distinct backing for face
admission, and a different collection index remains a distinct face. All temporary
admission records disappear on refusal; accepted session state changes only at
its existing commit point.

## Failing control and correctness

The work-count regression actually fails before memoization: 64 runs perform 64
face admissions instead of one. Its source, executable SHA-256 and exit-101 log
are retained. After the fix, eight focused unit tests cover once-per-frame work,
shared backings under different IDs, equal-byte aliases in different backings,
retained/same-frame identity collisions, collection faces, and later glyph/
transform refusal. A counted AsRef alias observes one byte comparison and one
face parse per frame despite repeated runs and retained fonts. The existing five
CPU VB3 text tests preserve positioning, variations/fallback, malformed-text
refusal, outline budgets and cache lifecycle.

Those 13 tests also pass after the same source patch is integrated into current
Netrender main, whose unrelated Vello cohort is preserved. Native performance
below is qualified on the targeted Cue cohort, not every current-main renderer.
The Netrender main fix is `9214a084ddff40d4b2c6c50ea0dea14ff15040fd`; Cue pins its
same-source targeted counterpart `21b73901bbcb46556bb6f54511f5d2499d4269f0`, directly
above prior pin `79c9487`. Genet `aead381ee7ce75705e281c41774f4e71b7efa0d7` changes only
four manifest pins above `82ef34d`, keeping Scene types unified without selecting
unrelated current Genet/main work. Its canonical dirty documentation is preserved.

## Paired measurements

The before/after probe uses the same 120-section document and eight scroll/resize
variants at 800x600 and 960x640. Geometry-only, image-bearing and mixed controls
remain separate. Each of 48 delivery/cache cases runs 40 frames, discarding eight
warmup frames, in three fresh-process repetitions. Cache policy order reverses in
the second repetition. The production cache policy remains unchanged; retained
text and stable-ID interning are diagnostic controls.

The baseline exports 32 compressed full-pixel references per hardware adapter
from independent cleared-cache renders. Every fixed frame compares complete
RGBA bytes with those old-renderer references, not merely with another fixed
render or a checksum. Reference decompression is bounded by the expected frame
size. Compression, readback, pixel checks and persistence are outside render/
submission spans. flate2 1.1.10 was already selected in the lock; the private
profiler gains one optional dependency edge to it.

Paired median session-render CPU times in milliseconds follow. These exclude
delivery, GPU completion waiting, readback and pixel comparison. The first pair
uses current table delivery and the unchanged production cache-clearing policy.
The second uses the existing diagnostic stable-ID/retained-text control.

| Host / backend | Production clearing: before → after | Reduction | Retained-text control: before → after |
| --- | ---: | ---: | ---: |
| Mac / Metal | 24.77 → 15.18 | 38.7% | 33.91 → 14.94 |
| Linux / Vulkan | 16.41 → 13.49 | 17.8% | 20.08 → 13.27 |
| Windows AMD / DX12 | 23.11 → 14.63 | 36.7% | 30.42 → 13.28 |
| Windows NVIDIA / DX12 | 25.33 → 16.38 | 35.3% | 32.40 → 14.82 |

All four adapters' baseline and fixed runs also retain identical source-image upload,
new-outline, cache-reset, font/residency and glyph-work counters for each matching
case/iteration. The fixed implementation removes repeated face admission without
reducing accepted scene content or its original outline work. Timings are CPU
wall time; session render includes admission, hydration, text/path processing and
GPU encoding. Completion waiting and readback are separate observations, not GPU
timestamps. Fonts differ by OS. Concurrent workspace load is retained in receipts;
absolute times are not a statistical hardware ranking.

The new eight-second Mac stack capture has 2,111 samples inside session rendering.
About 80.2% fall in glyph drawing/path lowering, 11.4% in other text admission/
preparation, 8.1% in other session work and 0.3% in byte comparison under text
planning. That last category can include small metadata/outline-key comparisons;
it is not exclusively full font payload comparison. The earlier sampled full-font
comparison hotspot is no longer material. This is sampled attribution, not an
exact timer decomposition or a comparison of idle-machine captures.

All 16 before/after native window workloads complete without runtime errors,
remain within the two-import bound and finish with zero imported frame slots.
Fresh-content 60 Hz gates remain false in every run. CPU preparation/submission
improves, but median new-content cadence remains limited by the larger pipeline.

| Host / placement | Median new-content interval before → after (ms) |
| --- | ---: |
| Mac / local | 33.34 → 33.34 |
| Mac / process | 66.69 → 66.69 |
| Linux / local | 33.33 → 33.33 |
| Linux / process | 66.66 → 66.66 |
| Windows AMD / local | 23.80 → 18.60 |
| Windows AMD / process | 54.62 → 48.54 |
| Windows NVIDIA / local † | 25.73 → 18.19 |
| Windows NVIDIA / process | 54.55 → 48.54 |

† Both NVIDIA local windows fall below the required DXGI feedback coverage;
their intervals are partial samples. The other 14 windows meet the coverage
threshold. Full loaded-phase/tail distributions and content counts are in the
receipt. These medians do not constitute a 60 Hz qualification.

Glyph/path processing remains the next large CPU component. This change does not
add glyph visibility rejection, an atlas, retained text policy or stable production
resource IDs. Those mechanisms retain their own validation, resource and lifetime
requirements.

## Supplier and evidence scope

All 565 package versions remain unchanged. Twenty-six source records move only
to the targeted Genet/Netrender revisions; one existing flate2 edge is added to
the private proof package. Other package records and the Vello `9f454193`, Graft
`d903cfac`, wgpu/naga 30.0.1 cohort remain unchanged. The separate Netrender main
integration retains its own supplier cohort and receives CPU text qualification.
No current-main Genet source, unrelated lock refresh or provider change enters
the measured Cue graph.

Final correctness gates execute 123 positive tests on the targeted consumer
cohort: 13 owner CPU tests, 43 Mac, 28 Linux and 39 Windows executions. Another
13 owner CPU tests qualify the same patch on current Netrender main, for 136
passing final test executions. Strict targeted all-target Clippy passes on Mac,
Linux and Windows. Linux explicitly loads Vulkan validation and reports zero
validation diagnostics; Windows retains its known warning baseline and rejects
unexpected warnings/errors.

The matrix executes 46,080 full-pixel comparisons: 23,040 baseline policy checks
and 23,040 fixed-versus-old-renderer comparisons. Resource, outline-preparation,
glyph-count and cache-epoch counters match before/after for corresponding cases.
The extra 512-frame Mac stack probe also compares against the old full-pixel
references. Both temporary Windows graphical-session tasks are removed. A
pre-GUI wait coordinator needed a retry after the shared package-cache delay;
no window had launched during that expired wait.

Native window observations use the same nine-second measured phases before and
after on every host, retaining warmup, two loaded CPU threads, resize, held-view
custody and drain. This duration preserves the existing 20,000-record per-process
profile cap after the prior submission spans were added. OS timestamp coverage
and phase/tail distributions are retained per run. No physical-input/photon,
full-browser, sandbox, all-backend or production 60 Hz qualification is inferred.

Build the proof with `window-receiver,scene-profile,cpu-raster`, using the mandatory
Code workspace build-storage launcher on the Mac and the existing stable lane.
The baseline and fixed executables are frozen separately. A matching GPU probe
pair is reproducible with:

```sh
/absolute/baseline-probe /absolute/fresh/before Arial --export-pixels
/absolute/fixed-probe /absolute/fresh/after Arial \
  --compare-pixels /absolute/fresh/before/pixels
```

Use native sans-serif on the measured Linux/Windows hosts and the same selected
DX12 adapter for both halves. Add `--reverse` for the second repetition. Long
stack captures can select a case with `--only document/table-current/clear-both`
and `--frames 512`. Both private helper/receiver executables must match their cohort.

The [compact receipt](../receipts/2026-11-05-font-admission/summary.json) contains
raw/source/binary hashes, reference-pixel hashes, paired distributions, invariant
checks and native results. Raw evidence is retained at
`/Users/markik/Code/output/cue-font-admission/`. Receipt budgets are 500 MB Mac and
150 MB Linux and 180 MB Windows, preserving the failing control, baseline/fixed executables,
compressed references and all earlier evidence. Existing compiler lanes are
reused; no Cargo home or new compiler-cache lane is introduced.
