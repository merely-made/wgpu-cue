# Two-process Metal frame handoff

The host owns process placement, the control transport, and slot reuse policy.
Genet produces an owned Netrender scene snapshot; Vello Hybrid rasterizes it in
the helper. Graft owns native allocation/import and consumer GPU release
tracking. Webcue owns a bounded, renderer-free frame-slot ledger. The existing
unit `GraftMetalImporter` remains available for retained in-process textures;
`GraftIoSurfaceImporter` adds tracked IOSurface custody.

## Storage and synchronization

The helper owns two RGBA8 IOSurfaces. Each lease has a scope, slot index, pool-wide
monotonic allocation epoch, and increasing content revision. It offers an actual
IOSurface Mach send right before the controlled copy is allowed. An offer alone
cannot become a Cue frame. After the GPU copy completes, the helper observes its
queue callback and sends Ready for that exact lease. Only then does the host
reconstruct the IOSurface and submit it to the importer.

The private bridge uses bounded Mach messages with a transferred port descriptor,
not a serialized process-local port number. It checks the kernel audit sender
PID, session nonce, scope and lease. Scene snapshots travel once through the
existing bounded stdin framing. Normal frame transfer carries no CPU pixels.
Readback exists only in qualification. This fixture is trusted local code, not
an application-signature authentication or OS sandbox implementation.

`FrameSync::None` in the received frame means the host has already observed the
producer's GPU completion message. The host must preserve that ordering when
replacing the transport, for example with XPC. No Metal pointer is shared between
processes, and neither process borrows the other's wgpu device.

The consumer calls `seal_use(stamp)` after its final GPU submission. The importer
waits for both the queue completion callback and final HAL texture destruction.
The Presenter, texture clones, views and queued GPU uses must relinquish custody.
`poll_releases` yields each completed stamp once. The host returns its matching
lease acknowledgement; producer reuse requires both producer completion and
consumer release. Early release, duplicate acknowledgements, stale scope or
slot epochs, and a full pool cannot authorize reuse. Retiring a pool forbids new
leases without fabricating either GPU completion.

The producer makes one GPU copy from Hybrid's ordinary output texture into
exportable storage. The consumer directly wraps the same IOSurface allocation.
This is not a zero-copy renderer-output claim. Storage format, extent, plane
layout, bytes per element and device epoch are checked before import.

## Failure and restart

Helper exit before Ready never publishes a frame. Loss drops pending rights and
abandons that session's pool; a new helper uses a new scope generation and fresh
allocations. Exit after import does not invalidate the consumer's retained native
storage. Its local GPU uses still need their own completion and release checks.
The proof transport bounds control packets to 4096 bytes, two outstanding offers,
eight queued responses and 30-second helper/deadline waits. Sends time out after
100 ms. Drop kills and reaps the helper. Shutdown requires all leases returned.

## Qualification

Native fixture: macOS x86_64, AMD Radeon Pro Vega 56, wgpu 30.0.1, the repository's
locked Genet/Netrender/Vello cohort, a 160x96 scale-one Genet document with a blue
background and white Arial text. Both processes log their distinct PIDs and
native adapter. The same scene rendered locally and through the helper compares
all 61,440 RGBA bytes exactly. The tests also qualify withheld readiness,
two-slot backpressure, retained Presenter/clone custody, exactly-once release,
monotonic slot reuse, stale metadata/device generations, orderly shutdown,
producer exit before Ready and readable imported storage after producer exit.

Run through the workspace build-storage launcher:

```sh
python3 /Users/markik/Code/tools/build_storage.py build \
  --lane cue-execution-host --reserve-gib 20 --cwd <checkout> -- \
  +1.99.1 test --locked --offline -p cue-execution-proof \
  --features metal-handoff --test metal_handoff -- \
  --ignored --nocapture --test-threads=1
```

Evidence lives in `/Users/markik/Code/output/cue-metal-handoff/`. Native evidence
does not qualify physical presentation, driver reset, iOS, BrowserEngineKit
extension entitlements, a production supervisor or sandbox, cross-GPU migration,
or other Vello backends. Classic and CPU retain their existing execution paths;
the native storage contract is independent of which renderer prepares pixels.
Graft's new API is pinned from Git for both grafting packages; it is not yet in
registry 0.6.1. Git consumers of Cue must carry the same pair of patches in
their workspace until a release contains the new API:

```toml
[patch.crates-io]
grafting = { git = "https://github.com/merely-made/wgpu-graft.git", rev = "a93693d2aaf0342075d3e9d7caedf51cf67edbc0" }
grafting-frame = { git = "https://github.com/merely-made/wgpu-graft.git", rev = "a93693d2aaf0342075d3e9d7caedf51cf67edbc0" }
```

Apple documents the capability lifetime in
[IOSurfaceCreateMachPort](https://developer.apple.com/documentation/iosurface/iosurfacecreatemachport(_:))
and [IOSurfaceLookupFromMachPort](https://developer.apple.com/documentation/iosurface/iosurfacelookupfrommachport(_:)).
The exported right must be deallocated; lookup retains storage without destroying
the port. Graft provides RAII custody for that reference.
