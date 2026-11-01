# Native command-execution qualification

Date: 2026-11-01. Base: `c314b7e3a2b2df4da2d9b0387b0c64c93b6b0e4d`.
Environment: macOS x86_64, Rust 1.99.1; native fixtures selected Metal and
reported AMD Radeon Pro Vega 56.

## Qualified behavior

The host retains request acceptance, terminal reservations and ordered events
while a host-supplied transport owns execution placement and shutdown. The same
static `GenetDocument` produces matching load/find/zoom outcomes on a cooperative
worker and in a supervised subprocess. A helper that enters an injected stall
is terminated and reaped after the surviving host settles its accepted request.

The mailbox rejects capacity insufficient for an accepted completion plus editor
publication/clearing. Completions cannot precede transmission. Loss, retirement,
backpressure, sequence/identity refusal and actual executor-stop evidence remain
separate states. No accepted request is replayed.

## Executed gates

All log paths below are relative to the retained evidence directory
`/Users/markik/Code/output/cue-execution-host/`.

| Gate | Result | Log |
| --- | --- | --- |
| `test -p webcue -p cue-execution-proof` | 33 passed, none failed or ignored | `12-final-tests.log` |
| `check --workspace --all-targets` | Passed | `06-workspace-check.log` |
| Focused `clippy --all-targets -- -D warnings` | Passed | `13-final-clippy.log` |
| Explicit native `cue-genet` retained/text-input tests | Four passed, none ignored in this run | `09c-native-metal-fixed.log` |
| Saved loss-to-retirement baseline | Expected assertion failure: `Retired` instead of `ExecutionLost` | `10-red-protocol.log` |
| Prior execution-session source with current tests | Expected two failures; other 12 passed | `11b-red-execution.log` |

The saved protocol baseline uses the original outcome regression; the final test
also checks preserved observations, completion order and idempotence. The two
execution-session regressions use the same tests on pre-fix and final source.
`11-red-execution.log` is a compile failure from stale metadata after copying
older source timestamps; it is not regression evidence. Refreshing source
timestamps, without clearing the cache, produced the actual failures in `11b`.

Rust formatting, `git diff --check`, TOML parsing and exact lock-package comparison
also passed. Every existing package record and supplier pin is unchanged; only
the private proof package is added. In particular, Genet remains at
`565458ea3ab816940484079330eb165a878a22e0`, Netrender at
`66b2f72e9e0785819139739cc4503bf1e8766de9`, and the Vello provider at
`276b46648c08f3d2062e70559afa999842e1873c`.

## Canvas and alpha diagnosis

The initial native fixture expected CSS half-transparent red over a transparent
page. Both the draft and unchanged base instead produced `[255, 127, 127, 255]`.
The pinned `genet-documents` top-level Livery session explicitly calls
`set_opaque_canvas(true)`. The fixture's expectation predated that policy.

The corrected document fixture checks opaque pink over a white Canvas and an
uncovered white pixel. A separate Classic rasterizer fixture now directly checks
`Alpha::Straight`, sRGB encoding, a half-transparent red pixel `[255, 0, 0, 128]`,
and an untouched transparent pixel `[0, 0, 0, 0]`. Both passed on Metal. No product
canvas policy or renderer behavior was altered to satisfy the tests.

The native log retains an existing ICU4X missing-CJK-segmentation-model diagnostic,
also present on the unchanged base. The retained IME assertions passed; broader
CJK segmentation is not qualified by this receipt.

## Reproduction and retained evidence

Use the workspace guard and stable lane for every Cargo invocation:

```sh
python3 /Users/markik/Code/tools/build_storage.py build \
  --lane cue-execution-host \
  --cwd /Users/markik/Code/worktrees/wgpu-cue-execution-host-20261101 -- \
  +1.99.1 test --locked --offline -p webcue -p cue-execution-proof -j 4
```

The workspace check uses `check --locked --offline --workspace --all-targets -j 4`.
Clippy uses `clippy --locked --offline -p webcue -p cue-genet -p cue-execution-proof
--all-targets -j 4 -- -D warnings`. Native tests use `WGPU_BACKEND=metal` and
`test --locked --offline -p cue-genet --test retained --test text_input -j 4 --
--ignored --test-threads=1 --nocapture`, with a 20 GiB build reservation.

The evidence directory retains the four initial sandbox refusals, subsequent
logs, pre-fix snapshots under `red-baselines/`, source manifests, and APFS-cloned
test/helper binaries with verified SHA-256 manifests under `qualified-binaries/`.
The red checkout is separate from the final implementation checkout.

## Limits

This is a trusted static-document command fixture, not a complete remote Surface,
an OS sandbox, an IPC standard, or browser compliance evidence. It does not
qualify remote input, native frame-handle transfer, BrowserEngineKit, live
migration, durable recovery after host death, CPU/Hybrid rendering, or performance.
The local worker is cooperative; its fixture cleanup joins finite trusted work.
Non-yielding execution requires the process adapter's forced-termination contract.
Executor termination does not prove GPU completion or release exported frames.

The [execution plan](../../docs/2026-11-01-execution-host-plan.md) and
[Vello placement review](../../docs/2026-11-01-vello-execution-placement.md) retain
the ownership decisions and the boundaries for those additional paths.
