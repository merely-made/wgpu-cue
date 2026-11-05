# Renderer submission, glyph preparation and image resources

Text admission and glyph path lowering dominate the measured Hybrid CPU path.
The cache clear calls themselves are small. Removing those calls alone is not a
supported optimization: current snapshot delivery remints resource identities,
and an identity-preserving diagnostic control makes text slower while making
image reuse effective. Production cache policy, validation and frame custody are
unchanged by this profiling slice. Cargo.lock and all supplier revisions remain
unchanged.

The subsequent [distinct-font admission change](2026-11-05-font-admission.md)
implements and qualifies the font-admission improvement separately. Measurements
in this report remain the original cohort's profiling evidence.

## Production-path attribution

On the measured Linux host, native local `HybridSceneSource::submit` takes a
median 17.03 ms across the complete window workload. Its `HybridSession::render`
span accounts for 16.61 ms. Text clearing is 0.0123 ms, image clearing 0.0005 ms,
snapshot decode 0.0768 ms, target allocation 0.0385 ms, encoder finishing
0.0491 ms and queue submission 0.1113 ms. These are separate distributions;
parent totals overlap children and should not be added to them.

The persistent helper reports 17.59 ms total submission and 17.13 ms session
rendering, with a 0.0151 ms text clear. Those native spans include startup,
steady, loaded and retained-view phases. The local steady phase's complete
active update, including preparation and submission in one measured span, has
a median 25.07 ms. Its preparation median remains 6.89 ms. Fresh-content
intervals remain about 33.33 ms locally and 66.66 ms across processes. These
measurements do not establish a 60 Hz result.

The new spans use the existing opt-in `scene-profile` sink. Without the feature,
measurement is an inlined direct call with no clock or sink. Targets, encoders,
queue submission order, error scopes, refusal cleanup fences, completion polling
and frame-slot release rules are unchanged. An existing exact-pixel Hybrid test
now also checks that all ten submission stages are emitted.

## Controlled resource and cache experiments

`cue-renderer-profile` is a private, serialized native GPU probe using the exact
pinned supplier session. It times delivery, cache clears, target/encoder creation,
session render, encoder finish, queue submit, completion wait and readback
separately. The render span includes admission, image hydration/upload, text
preparation, path lowering and backend GPU encoding. Completion waits are CPU
observations, not GPU timestamps. Readback, equality checking, checksumming and
result persistence are outside the CPU render/submit spans.

The input is the same 120-section document, with eight retained scroll/resize
variants at 800x600 and 960x640. It has approximately 12,000 positioned glyphs
and no images. A geometry control removes glyph runs; an image supplement adds
a 256x256 RGBA image to that geometry; a mixed supplement retains text and adds
the same image. The image has alpha and changes content halfway through the
eight-variant cycle. These supplements are separate from the window fixture.

There are three delivery modes:

- `typed-fresh`: the production typed capture path, with fresh snapshot-local IDs.
- `table-current`: the production sender/receiver packet path, including its
  structural validation, budgets and current ID rebinding.
- `table-interned-control`: the same validated table path followed by diagnostic
  local interning of immutable owners by exact bytes. It has explicit table-size
  and source-byte bounds. It models stable local cache keys without changing the
  production capture API or accepting foreign IDs.

Each mode runs four independent policies: clear both caches, retain text only,
retain images only and retain both. Existing supplier limits/epochs remain in
force. Each case renders 40 frames, excludes its first eight from timing summaries,
and compares every full output byte-for-byte with an independently cleared
reference for that scene variant. Three fresh-process repetitions reverse policy
order in the second repetition. Native Metal, Linux Vulkan and Windows AMD/NVIDIA
DX12 execute the matrix. Fonts differ by OS; hardware ranking is not inferred.

The table gives pooled steady probe medians in milliseconds (96 measured frames
per case across three repetitions). Stable-ID columns use only the diagnostic
interning control. Mac/Windows load varies; compare policies within each host,
not hardware against one another.

| Host / backend | Text: stable IDs, clear → retain text | Geometry only | Images: stable IDs, clear → retain images |
| --- | ---: | ---: | ---: |
| Mac / Metal | 34.02 → 44.33 | 0.74 | 2.79 → 1.27 |
| Linux / Vulkan | 16.60 → 20.22 | 0.41 | 1.48 → 0.45 |
| Windows AMD / DX12 | 25.10 → 32.57 | 0.61 | 2.24 → 0.79 |
| Windows NVIDIA / DX12 | 24.08 → 32.00 | 0.65 | 1.95 → 0.83 |

The Linux text document contains 12,171 positioned glyphs per scene. Current
snapshot delivery prepares 39 distinct admission outlines per frame even when
text retention is requested. The interning control reduces newly prepared
outlines to zero after warmup, but text rendering rises from roughly 16.6 ms to
20.3 ms. The same controlled image path reduces median image-bearing session
rendering from roughly 1.5 ms to 0.45 ms. Image retention avoids uploads on
unchanged-content frames; content switches still upload. In each 32-frame
measured image case, the stable retained control uploads eight times, versus
32 for fresh IDs or clearing. This is bounded source-image reuse, not a claim
about arbitrary image sizes or atlas caching.

## Why retaining text is slower

Source inspection and native CPU stack sampling identify two major costs.
`TextState::plan` checks every glyph run's font against retained fonts and the
fonts already encountered in the current frame. Matching identities trigger
complete byte comparisons. A retained font can therefore be compared through
both collections, repeatedly across many runs. Stable cache keys remove repeated
outline preparation while increasing this repeated font-buffer checking.

In the eight-second Mac current-table sample, 2,512 samples fall inside session
rendering. Exclusive categories within that subtree are approximately 38.3%
font-byte comparisons in text planning, 48.8% glyph drawing/path lowering,
7.3% other text admission/preparation and 5.5% other session work. The separate
stable retained-text capture shifts about 55.1% to font-byte comparisons and
34.3% to glyph drawing/lowering. These are sampled wall-time estimates, not exact
stage timers. Ambient load differs between captures; their absolute wall times
must not be compared.

The pinned Netrender outline backend explicitly uses `AtlasCacher::Disabled`.
Glifo consequently sends prepared outlines through direct path filling. Retaining
an outline/preparation cache therefore does not eliminate positioned-glyph
path processing, strip generation and tile work. The geometry ablation removes
nearly all measured render cost, and the stack sample locates substantial work
inside this lowering path. Viewport/run rejection using validated bounds is an
opportunity to measure, not an implemented change or permission to skip resource
and operation admission.

There is also an identity-lifetime gap in current Cue table delivery.
`ResourceReceiver` looks up its locally minted Blob owners, then calls
`SceneSnapshot::capture_owned`, which remints IDs again. Raw probe rows confirm
32 different font-ID tuples over each 32-frame current-table measurement, versus
one tuple for the interned control. Payload transfer is deduplicated and Arc
backing remains shared, but renderer keys are not stable across table snapshots.

## Ownership and qualification

Repeated font admission belongs to Netrender's shared text-session owner. A
bounded improvement can validate distinct font backings/faces once per frame
while retaining the same-ID/different-bytes refusal, font/glyph limits and
malformed-outline checks. Cached IDs alone must not authorize bypassing those
checks. A stable identity path belongs to Cue's table/snapshot boundary, scoped
to the existing exact renderer incarnation, with public capture continuing to
rebind foreign identities. Image hydration/reuse remains in the supplier session.
Glyph path/visibility work crosses Netrender lowering and the shared Glifo/Vello
implementation. Classic, CPU and WebGL performance are not measured by this
Hybrid matrix, even where ownership mechanisms are shared.

Final correctness gates execute 25 positive tests: 17 on Mac, two Linux Vulkan
native tests and three DX12 native tests on each Windows adapter. Strict targeted
all-target Clippy passes on all three hosts with profiling, and Mac additionally
checks the no-profile feature path. Linux explicitly loads Vulkan validation and
reports zero validation diagnostics. Windows retains its existing warning
baseline; unexpected warnings and errors remain rejected.

The full probe matrix completes 576 cases and 23,040 exact pixel comparisons,
with counter checks confirming identity churn, zero new warm outlines for stable
retained text, and the expected upload reduction for retained stable images.
Eight final native receiving-window runs complete without runtime errors, stay
within the two-import bound and drain imports to zero. All fresh-content 60 Hz
gates remain false. Seven runs meet the OS-feedback coverage threshold; NVIDIA
local does not, so its observed content intervals are partial DXGI samples.
The temporary Windows graphical-session task is removed after completion.

Mac and Windows have unrelated workspace builds during profiling. Load records
are retained; three repetitions and reversed policy order expose variation but
do not turn these into idle-machine statistical benchmarks. Linux supplies the
primary quantitative attribution above. Native window feedback retains the
existing OS-specific coverage limits. Mac/Linux windows use 15-second measured
phases. Windows uses nine-second measured phases: ten additional stage spans at
its prior fast local throughput would exceed the existing 20,000-record cap.
The shorter observation retains the warmup, loaded resize, two stress threads,
held-view custody and drain phases while preserving that diagnostic bound.
The 40-frame GPU probe matrix is identical on all four adapters. No physical-input, photon, sandbox or
full-browser qualification is claimed.

## Reproduction and evidence

Build the private proof binaries with `window-receiver,scene-profile,cpu-raster`
through the mandatory Code build-storage launcher on the Mac, reusing the
`cue-execution-host` lane. Remote runs reuse their existing repository targets.
No new Cargo home or compiler-cache lane is created. Run:

```sh
/absolute/bin/cue-renderer-profile /absolute/fresh/output Arial
/absolute/bin/cue-renderer-profile /absolute/fresh/reversed Arial --reverse
python3 support/execution-proof/tools/summarize_renderer_profile.py /absolute/fresh/output
```

Use native `sans-serif` on the measured Linux/Windows hosts, and
`CUE_DX12_ADAPTER_NAME` to select the same AMD/NVIDIA adapters as the native
handoff fixtures. A bounded stack capture can select one case:

```sh
/absolute/bin/cue-renderer-profile /absolute/fresh/stack-run Arial \
  --only document/table-interned-control/keep-text --frames 512
```

The existing receiving-window fixture collects production spans with its normal
profile directory setting. It does not use diagnostic cache policies or interning.
The [compact receipt](../receipts/2026-11-04-renderer-profile/summary.json) retains
source and raw-file hashes, per-case distributions/repetition medians, native
window results and qualification limits. Raw evidence is retained under
`/Users/markik/Code/output/cue-renderer-profile/`; earlier source2 Linux trials
used a misleading `table-stable` label and are exploratory only. Final source3
uses the accurate `table-current` name and independently checks actual resource
identities. Receipt budgets are 300 MB Mac and 50 MB on each remote host. Source,
earlier controls, frozen binaries and existing qualification worktrees are
preserved.
