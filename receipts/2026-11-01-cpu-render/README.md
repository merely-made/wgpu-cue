# Owned scene jobs and CPU rendering qualification

Date: 2026-11-01. Base: `4d17e0c258e8fea840f52e249562d6e577de2c2a`.
Environment: macOS x86_64, Rust 1.99.1. Native receiving used Metal on
AMD Radeon Pro Vega 56 and explicitly refused a CPU/software GPU adapter.

## Implemented boundary

`cue-netrender/scene-jobs` exposes complete owned scene snapshots and render jobs
without enabling Vello CPU. `cue-netrender/cpu` adds an explicit CPU raster
operation and a nonblocking one-frame receiving endpoint. Default features remain
empty. The host supplies scheduling, process supervision and resource authority;
the document retains control, input and semantic ownership.

Snapshots include font and image bytes and refuse unresolved image keys,
registry-local fragments and native compositor surfaces. Decode creates fresh
Blob identities and reconstructs the reserved empty font sentinel. The receiver
enforces its own snapshot, source-byte and output-byte budgets before job handoff.
Expected scope/revision/allocation stamps prevent late or duplicate frames from
replacing current pixels. Resize, hide and retirement invalidate pending jobs.

CPU frames are owned premultiplied sRGB RGBA8. This endpoint requires scale one
and exact scene/viewport extents. Typed provider admission failures remain visible;
there is no implicit backend fallback or scene-color repair.

## Executed evidence

Logs are retained under `/Users/markik/Code/output/cue-cpu-render/`.

| Gate | Result | Log |
| --- | --- | --- |
| Focused CPU, command and lifecycle suite | 42 passed; one native fixture explicitly ignored in this run | `08-final-tests.log` |
| Shared scene jobs with CPU disabled | Passed | `09-scene-jobs-only.log` |
| Workspace check, all targets and features | Passed | `10-workspace-check.log` |
| Focused Clippy, all targets, `-D warnings` | Passed | `11-clippy.log` |
| Native Metal suite with CPU enabled | Five passed | `07-native-metal.log` |
| Final CPU receiving/Classic comparison after namespace normalization | One passed | `12-final-native.log` |
| Inbox before expected-stamp validation | Expected two assertion failures; other three tests passed | `05-red-inbox.log` |

The two pre-fix failures demonstrate that superseded and invalidated jobs could
replace pixels before the stamp check. Their assertions pass in the final suite.
The final suite adds resource-budget, identity, image ownership, pixel-convention
and retry-after-rejected-admission coverage. Earlier lock-resolution and invalid
test-input logs are retained but are not counted as the expected regression.

The actual Genet fixture is a 160x96 scale-one document with a blue canvas and
16px Arial text. Its prepared scene contains glyph runs and owned font sources.
The worker thread and helper process rasterized the same snapshot to byte-equal
pixels, preserving a scope owner wider than 64 bits. The helper receives scene
data; it does not rerun Genet document preparation or rediscover fonts.

Native receiving uploaded those CPU pixels on the host device and read them back
byte-for-byte, preserving premultiplied alpha metadata. Classic rendered the same
decoded scene with its canvas and foreground text intact. Mean absolute channel
difference between CPU and Classic was `0.032145182291666664` on a 0-255 scale.
That metric describes this fixture and host font environment, not general backend
parity. The test asserts canvas/text presence for both renderers and records the
difference; it does not impose a general visual-equivalence tolerance.

The existing retained lifecycle, document-canvas, raw Classic transparency,
text-input and password-geometry fixtures also passed with the CPU/serde feature
closure enabled. Their native log retains the previously observed ICU4X missing
CJK-segmentation-model diagnostic; the ASCII CPU fixture does not qualify CJK
segmentation.

## Supplier and feature identity

All existing package identities, versions, sources and checksums are preserved.
The lockfile adds 15 packages for the optional CPU/serde dependency closure and
feature-induced dependency edges. No existing dependency edge is removed.
`final-dependency-delta.json` records the precise additions.

- Genet: `565458ea3ab816940484079330eb165a878a22e0`.
- Netrender: `66b2f72e9e0785819139739cc4503bf1e8766de9`.
- Classic and sparse Vello provider: `276b46648c08f3d2062e70559afa999842e1873c`.
- Sparse CPU/common packages: 0.3.0; Glifo: 0.4.0, all at that provider revision.

## Reproduction and retained artifacts

All Cargo invocations used the existing `cue-execution-host` compiler lane via
the workspace storage guard. Evidence is separate from compiler intermediates.

```sh
python3 /Users/markik/Code/tools/build_storage.py build \
  --lane cue-execution-host --reserve-gib 20 \
  --cwd /Users/markik/Code/worktrees/wgpu-cue-cpu-render-20261101 -- \
  +1.99.1 test --locked --offline -p cue-netrender -p cue-execution-proof \
  -p webcue --features cue-execution-proof/cpu-raster -j 4
```

The independent contract check uses
`check --locked --offline -p cue-netrender --no-default-features --features scene-jobs`.
The workspace check uses `check --locked --offline --workspace --all-targets
--all-features`. Clippy selects `cue-netrender`, `cue-genet`, `cue-execution-proof`
and `webcue`, enables `cue-execution-proof/cpu-raster`, and uses
`--all-targets -- -D warnings`.

Native gates set `WGPU_BACKEND=metal`. The five-fixture run selects `cue-genet`
and `cue-execution-proof`, enables `cue-execution-proof/cpu-raster`, and passes
`-- --ignored --test-threads=1 --nocapture`. The final focused native run selects
the proof's `raster` target and
`rendered_cpu_frame_uploads_unchanged_to_host_device -- --ignored --exact --nocapture`.

The evidence directory retains the red source and failing binary, final sources,
validation logs, dependency/preservation inventories, and APFS-cloned test/helper
binaries with verified SHA-256 manifests.

The host still bounds in-flight jobs and executor concurrency. Fresh sessions
trade cache retention for independent complete jobs. These measurements establish
no performance claim, total-memory sandbox, OS isolation, general CPU/Classic
parity, Hybrid adapter, native frame-handle transfer or BrowserEngineKit support.
The [CPU contract](../../docs/2026-11-01-cpu-render-jobs.md) records those boundaries.
