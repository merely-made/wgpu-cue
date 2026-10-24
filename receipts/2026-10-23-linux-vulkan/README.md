# Shared Vulkan importer — 2026-10-23

The published Graft 0.6.1 receiving gate passes 12 tests: three core unit tests,
eight lifecycle tests and one explicitly requested hardware Vulkan test. The
hardware gate checks a real completed GL DMA-BUF snapshot after producer teardown,
premultiplied alpha, pixel values, stamp/device epoch, and descriptor closure on
synchronization, modifier and epoch refusal. Khronos validation was requested;
the retained log contains no validation error. This is importer evidence, separate
from the Turnstone Servo product qualification. The source hashes identify the
new SDK implementation; formatting after the earlier development probe has no
semantic change. The canonical implementation record is
[the Graft plan](https://github.com/merely-made/wgpu-graft/blob/main/design_docs/2026-10-23_linux_native_cue_plan.md).

The macOS regression gates pass 11 portable tests and both explicitly requested
Metal GPU tests on AMD Radeon Pro Vega 56. The normalized package verifies with
registry Webcue 0.1.1 and Graft 0.6.1. The frozen lock records this receiving cohort.
