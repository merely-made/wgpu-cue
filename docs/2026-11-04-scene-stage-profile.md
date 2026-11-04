# Scene preparation and snapshot cost attribution

The earlier 47–60 ms preparation stage is predominantly full-scene JSON
encoding and parsing of embedded font resources. Retained document preparation
is sub-millisecond at the median in this fixture. Fresh resource-ID rebinding
is also sub-millisecond. This slice adds opt-in timing and diagnostic probes;
it makes no rendering, codec, resource-caching or supplier changes.

## Measurement boundary

The shared fixture is exactly the prior receiving-window document: 120 styled
sections, text, borders, 602 scene operations and three font palette entries.
Mac uses Arial (1,524,220 source bytes); Linux and Windows use their native
sans-serif closures (1,592,256 and 2,035,964 bytes). No fonts are stored in these
receipts. Limits and dependency identities remain unchanged.

Each CPU probe performs 72 scroll/prepare/capture/decode iterations, changes
800x600 to 960x640 at iteration 40, and discards the first eight iterations
when summarizing. Three runs per host yield 192 measured iterations per host.
The probe asserts operation/font counts, exact decoded font bytes, and fresh
nonempty font Blob IDs. Document preparation includes the retained Genet
`try_frame` path and scene construction, not a separately isolated layout
algorithm. Resize can still cause a large preparation outlier; medians do not
assert a bound on layout latency.

`scene-profile` is an explicit adapter feature. A host installs one process-wide
callback before work; all participating threads can report bounded records.
Without the feature, the timing wrapper is a direct inlined call with no clock
or sink. The private proof owns persistence, accepts an explicit profile path,
creates one JSONL file per PID, and refuses overwrite. It fails on log errors
or more than 20,000 records per process. It never logs scene/font bytes.

The stage hooks surround the existing calls without changing their order or
policy. Timings are wall time, not sampled CPU cycles. Parent totals overlap
children and must not be added. Child observer writes occur inside enclosing
totals. A small residual includes byte freezing into Arc storage, temporary
validation-scene destruction, instrumentation and other bookkeeping; those
costs are not separately attributed.

## CPU probe results

The table contains pooled medians in milliseconds. Every row has 192 samples
per stage; the compact receipt retains per-run and pooled mean/p50/p95/p99/max.
Windows batch measurements differ from its earlier interactive workload;
absolute cross-run timing must not be treated as a controlled machine comparison.

| Host | Document prepare | Capture total | JSON encode | Validation parse | Later decode parse | Fresh-ID rebinding |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Mac | 0.502 | 56.798 | 26.868 | 27.430 | 27.377 | 0.285 |
| Linux | 0.197 | 48.170 | 22.373 | 25.020 | 24.827 | 0.169 |
| Windows | 0.440 | 80.555 | 38.289 | 40.449 | 40.037 | 0.511 |


Capture first checks resource closure, serializes the entire Scene, calls
`from_bytes`, parses into another Scene, checks closure again, and freezes the
bytes. Closure checks take only microseconds in this document. JSON encoding
plus the validation parse account for most capture time. The later `decode`
performs another parse and closure check, then creates fresh job-local Blob IDs
by copying font/image bytes. The JSON parser itself already allocates resource
buffers and constructs deserialized Blobs; `decode.rebind_resources` measures
the additional namespace-rebinding copy, not every resource-related allocation.

Source inspection confirms that the pinned Netrender Blob serializer emits
`(bytes, id)`, with byte slices represented as numeric arrays in JSON. The Mac
snapshot is about 5.07 MB despite only 1.52 MB of font source bytes. Repeating
that closure for every scrolling frame explains why the snapshot stage remains
expensive while retained document preparation is cheap.

## Resource-payload ablation

The diagnostic probe clones the same scene operations but replaces only font
and image payload bytes with empty buffers. It times direct JSON encode/parse
of that clone; the clone is never rendered or passed off as a valid text scene.
It is a cost-isolation experiment, not an optimization or output-equivalence test.

| Host | Full JSON encode → no resource bytes | Full parse → no resource bytes |
| --- | ---: | ---: |
| Mac | 26.868 → 1.621 | 27.430 → 2.574 |
| Linux | 22.373 → 1.597 | 25.020 → 2.726 |
| Windows | 38.289 → 2.188 | 40.449 → 3.460 |


On Mac the resulting structural payload is about 0.60 MB versus 5.07 MB in the
full snapshot. Large reductions in both encoding and parsing identify embedded
resource payload processing as the dominant portion of these JSON stages.
This experiment does not prove an equivalent resource-reference protocol or
predict its final rendering performance.

## Native confirmation and remaining boundaries

The profiled local and process receiving windows complete on Mac/Metal and
Linux/Vulkan with OS feedback, actual resize, bounded imports and zero final
imports. All retain a failed 60 Hz pacing target. Their traces include startup,
warmup, steady, loaded and drain work; the native stage summaries deliberately
include all phases rather than presenting them as steady-only statistics.

Mac local median capture is 59.65 ms: encoding 26.96 ms and validation parsing
30.06 ms. Mac process-host capture is 65.02 ms under this run's scheduling/load;
the helper separately records approximately 29.36 ms receive-side parsing and
29.55 ms renderer decoding. Linux local capture is 50.19 ms, and process-host
capture 50.18 ms; the helper's receive parse is 23.52 ms and decode parse 24.06 ms.
Counts and raw per-thread records are retained. These are separate stages,
including concurrent work on the reader/render threads, not an additive
end-to-end IPC latency model.

Windows AMD Radeon 780M interactive local/process windows also complete with zero
final imports and failed pacing gates. Local median document preparation is 0.19 ms, capture 46.90 ms, encoding
24.01 ms and validation parsing 21.87 ms. Its later decode parse is 21.83 ms
and rebinding 0.28 ms. Process-host capture is 53.56 ms; the helper records
27.97 ms receive parsing and 28.70 ms decode parsing. These interactive results
confirm the same split near the original 47–60 ms range. Per-PID results are
retained in the compact receipt. The batch CPU timings above are higher than the earlier
window baseline; no CPU-frequency or scheduler tracing was collected to explain
that difference. The resource-payload attribution remains consistent. Windows
OS timestamp coverage retains the limitations documented in the pacing slice.

The resource reconstruction timing here covers serialized source buffers and
fresh Blob identities. GPU allocation/upload, glyph outline preparation and
backend caches remain downstream work. Source inspection also shows that
`HybridSceneSource::submit` explicitly clears image/text caches before rendering;
this slice does not time those backend internals or remove those clears.

The next bounded change should preserve the validated ownership contract while
avoiding this repeated representation work: a typed local scene path can retain
owned/shared resources without the JSON round trip, and a persistent process
resource table can transfer immutable font/image bytes once per authorized
renderer incarnation. Both need equivalence, invalidation, byte-budget and
restart controls. Simply omitting validation or reusing arbitrary incoming Blob
IDs would weaken existing guarantees and is not proposed. A binary codec alone
would still resend unchanged resources; measure the ownership cut first.

## Verification and reproduction

A new regression observes real snapshot operations and checks identical capture
bytes, fresh resource IDs, resource-budget refusal, actual stage emission and
single installation of the sink. It passes on all three hosts. Mac additionally
passes six proof unit, five Hybrid, five Metal handoff and nine renderer-session
tests: 28 positive test executions including the three platform runs of the new
regression. The CPU probes and receiving-window measurements are recorded
separately from that test count. Strict targeted Clippy passes with profiling
enabled on Linux/Windows and disabled on Mac, checking the normal no-clock path.

Build the binaries with `window-receiver,scene-profile` using the existing stable
build lane (the Code workspace launcher is mandatory on this Mac). Run:

```sh
/absolute/bin/cue-scene-profile /absolute/fresh/cpu-run Arial
python3 support/execution-proof/tools/summarize_scene_profile.py /absolute/fresh/cpu-run/stages-*.jsonl
```

Use `sans-serif` on the measured Linux/Windows hosts. For a native window, create
an empty profile directory, set `CUE_SCENE_PROFILE_DIR` to its absolute path and
run the existing window workload. Helpers inherit the directory and emit their
own PID file. For a macOS app bundle, put the absolute directory in
`Contents/Resources/profile-directory.txt`; both frozen executables read it.
The profile path is private diagnostic configuration, not a document capability.

The [compact receipt](../receipts/2026-11-04-scene-profile/summary.json) retains
stage distributions, raw/binary hashes, source identity and workload status.
Raw evidence is under `/Users/markik/Code/output/cue-scene-profile/`, including
three CPU runs per host, source1 manifest/archive, frozen Mac binaries/apps,
per-PID timing records, native summaries and gate logs. Budgets are 300 MB Mac
and 50 MB on each remote host. The exact Cargo.lock bytes and supplier cohort
from the window-pacing slice remain unchanged. No hosted CI, all-backend
rendering or 60 Hz performance qualification is claimed.
