# Hybrid host-device qualification

Date: 2026-11-01. Base: `740e8589e51213cedef63103029b3281d562aab1`.
Environment: macOS x86_64, Rust 1.99.1, Metal on AMD Radeon Pro Vega 56.
Native fixtures explicitly refused CPU/software GPU adapters.

## Qualified boundary

`cue-netrender/hybrid` provides a raster-only host-device surface using the shared
owned scene contract. Its CPU preparation, GPU session, encoder, target and
submission remain with one GPU owner. It submits before another provider render
can occur and bounds admission to one pending submission. Texture polling only
polls readiness, requiring a completion callback and resolved error scopes.

Hide, resize and retirement suppress delivery while retaining pending work for
completion observation. Scope, viewport, device, queue and epoch identities remain
explicit. Complete jobs reset source and outline caches between submissions while
reusing provider pipelines/resources. Provider refusal preserves the typed error,
does not consume a revision, and retains a non-publishing cleanup fence to flush
potentially staged uploads. Draw counters exclude cleanup fences.

## Executed gates

Logs are retained under `/Users/markik/Code/output/cue-hybrid-render/`.

| Gate | Result | Log |
| --- | --- | --- |
| Focused command, lifecycle and CPU suite, all features | 42 passed; five native fixtures explicitly ignored in this run | `05-focused-tests.log` |
| Hybrid alone, CPU disabled | Four native tests passed | `04-native-hybrid.log` |
| Combined Metal suite | Nine native tests passed | `06-native-all.log` |
| Workspace check, all targets/features | Passed | `07-workspace-check.log` |
| Focused Clippy, all targets/features, `-D warnings` | Passed | `08-clippy.log` |
| Before pending-frame invalidation was enforced | Expected assertion failure after hide/show | `02-red-invalidation.log` |

The red source and failing executable are retained separately. The same
invalidation assertion passes after pending delivery is disabled. The final test
also checks resize/back invalidation and retains submission custody until polling
observes completion. The intermediate `03-native-hybrid.log` is not red evidence:
its foreign-device fixture incorrectly assumed another `RenderCore::boot` made a
different device. The corrected fixture explicitly requests a second wgpu device
from the adapter, asserts distinct device identity, and tests wrong-queue pairing
separately.

Native checks cover:

- Single-flight backpressure and exactly one admitted draw during repeated frame
  polling. A second draw is submitted after the first is observed complete.
- Premultiplied transparent geometry `[128, 0, 0, 128]`, untouched transparent
  pixels `[0, 0, 0, 0]`, and owned-image pixels `[0, 0, 128, 128]`.
- Same-device/queue binding, an explicitly distinct foreign wgpu device, a mixed
  queue, stale logical device epochs, scale-one admission and source-byte limits.
- Typed refusal of bilinear repeat patterns without changing sampling, cleanup
  fencing without publishing, and retry of the same rejected revision.
- Retained submission observation after retirement.
- Rendering an actual Genet text scene through the host presenter, then rebuilding
  the renderer under a new logical epoch and presentation generation. Rebuilt
  pixels match the original. The existing Genet document still answers find
  commands afterward; it is not recreated for renderer recovery.

The combined gate also reruns CPU-frame receiving and existing Genet retained,
canvas/alpha, IME, text-input and password-geometry fixtures. The existing ICU4X
CJK-segmentation diagnostic remains in those logs; it is outside the ASCII text
fixture's qualification.

## Same-snapshot backend comparison

The Genet scene is the same 160x96, scale-one, blue-canvas/white-Arial text fixture
used by the CPU followup. With both features enabled, one captured snapshot and
its font bytes feed all three renderers. Tests require canvas and foreground text
in every result and record mean absolute channel differences on a 0-255 scale:

| Pair | Mean channel difference |
| --- | ---: |
| Hybrid / CPU | 0.0065266927083333336 |
| Hybrid / Classic | 0.030989583333333334 |

The separate CPU receiving fixture also recorded CPU/Classic mean difference
`0.032145182291666664`. These are measurements for the recorded fixtures and host
font environment. The
tests do not impose a general visual-equivalence tolerance or authorize automatic
backend fallback.

## Dependency and storage identity

Existing package identities, versions, sources and checksums are preserved.
Dependency auditing resolves lockfile names to `(name, version, source)` before
comparison: new duplicate versions require textual disambiguation of several
existing edges without changing their actual targets. Every old resolved edge is
preserved. The only added edges on existing packages are Netrender to `vello_gpu`
and the private proof to the already-locked `pollster` test executor.

The optional GPU/shader closure adds 44 packages. `vello_gpu` and
`vello_gpu_shaders` are 0.3.0 at the existing provider revision
`276b46648c08f3d2062e70559afa999842e1873c`. Netrender remains at
`66b2f72e9e0785819139739cc4503bf1e8766de9`; Genet remains at
`565458ea3ab816940484079330eb165a878a22e0`. Default adapter features remain empty.
Hybrid-only metadata resolves no Vello CPU dependency.

All Cargo commands used the guarded `cue-execution-host` compiler lane. Evidence
is retained separately, including logs, dependency and preservation inventories,
red/final source snapshots and APFS-cloned executables with verified SHA-256 hashes.

```sh
WGPU_BACKEND=metal python3 /Users/markik/Code/tools/build_storage.py build \
  --lane cue-execution-host --reserve-gib 20 \
  --cwd /Users/markik/Code/worktrees/wgpu-cue-hybrid-render-20261101 -- \
  +1.99.1 test --locked --offline -p cue-genet -p cue-execution-proof \
  --all-features -j 4 -- --ignored --test-threads=1 --nocapture
```

The focused suite selects `cue-netrender`, `cue-execution-proof`, and `webcue`
with `--all-features`. Workspace checking adds `--workspace --all-targets`.
Strict Clippy selects those three packages plus `cue-genet` and uses
`--all-targets --all-features -- -D warnings`.

## Limits

Qualification covers local Metal rendering, trusted complete snapshots and scale
one. The device-epoch test is a logical lifetime change on the same physical GPU;
physical device loss and driver reset are unqualified. GPU validation faults and
late provider errors after actual staged uploads were not separately fault-injected;
the cleanup fence is exercised by the typed sampling-refusal fixture.

The trusted host supplies an associated device/queue pair. Identity checks are
not a GPU-object sandbox. Logical budgets are not total driver/process memory
limits, and performance is unmeasured. Native frame-handle transfer, cross-process
GPU presentation, BrowserEngineKit and OS isolation remain separate boundaries.
See the [Hybrid contract](../../docs/2026-11-01-hybrid-render-jobs.md) and
[placement review](../../docs/2026-11-01-vello-execution-placement.md).
