# Hybrid Vulkan shader validation

The rendering fixture must pass both its pixel/lifetime assertions and Vulkan
validation. A Rust test exit of zero alone does not establish this: the original
local Hybrid control passed while the validation layer reported seven instances
of `VUID-StandaloneSpirv-None-10684`.

## Diagnosis and bounded fix

The locked Vello shader package at `276b46648c08f3d2062e70559afa999842e1873c`
contains mutable vertex-output structs in `render`, `blend`, `copy`, and `filter`.
Naga 30.0.1 emits `Offset` decorations for those struct types. Wgpu 30.0.1 chooses
SPIR-V 1.6 on Vulkan 1.3+ devices. Function-local variables cannot use those
explicit layouts in SPIR-V 1.5 and later. A three-line WGSL compute shader with a
mutable local struct reproduces the problem independently of Genet, Vello, IPC,
and native frame imports.

The targeted Vello change computes scalar/vector fields and returns the vertex
output with a value constructor. It preserves member order, output bindings,
calculations, uniform layouts, fragment inputs, and the SPIR-V version. The
`clear` shader already follows this pattern. This removes the invalid local
storage from these five shipped shaders; it is not a general Naga backend fix
for arbitrary WGSL containing mutable aggregate variables.

The standalone regression compiles all five linked shaders as SPIR-V 1.6, both
as complete modules and separately for each entry point (17 modules total), and
runs `spirv-val --relax-block-layout --target-env vulkan1.3`. It requires a real
validator and retains the compiled inputs. The original shader sources fail that
same regression; the modified sources pass.

## Native validation gate

After building the Linux `vulkan_handoff` test target with the `vulkan-handoff`
feature, run the actual test executable through:

```sh
python3 support/execution-proof/tools/validate_vulkan.py \
  --log <retained-log> --receipt <retained-json> --expected-tests 2 -- \
  <vulkan_handoff-test-executable> --ignored --nocapture --test-threads=1
```

The runner forces the layer to log to stdout, enables shader validation, disables
shader-validation caching, enables synchronization validation, and records
loader evidence that the validation layer was inserted, and rejects nonzero
command exits, absent or unexpected test counts, ignored tests, validation
errors, VUID diagnostics, and synchronization hazards. Helper stdout remains
inherited, so diagnostics from the separate renderer are included. Run the gate
against debug fixtures; release builds may disable wgpu diagnostic routing.

The gate's Python regressions include the original failure mode: successful
pixel tests accompanied by an invalid-shader diagnostic must fail the gate.
The existing local-control and persistent-handoff tests continue to own pixel,
slot-reuse, retained-view, resize, peer-exit, and restart assertions.

## Evidence status

Evidence is retained in
`/Users/markik/Code/output/cue-hybrid-vulkan-validation/`. On T-PC's AMD RADV
RENOIR adapter, the original local pixel control still passes its pixel assertions
but fails the strict gate with seven shader-validation errors. With the targeted
shader-package overlay, both the local control and the persistent two-process
handoff pass, with zero validation diagnostics. All ten existing native Hybrid
and Metal handoff tests also pass on the Mac's AMD Radeon Pro Vega 56.

An initial native gate run without forced layer logging incorrectly appeared
clean even for the original shaders. Those initial receipts are excluded from
qualification; `native-red-forced` and `native-green-forced` are the controls.
Loading the layer alone is insufficient evidence that diagnostics are captured.

The tested supplier fix is commit
`9f454193808dda0db4beb5a46aa295206bfa706d`, a direct child of the existing
Vello pin, published on `shader-vulkan-1.6-276b466`. These hardware results use a
shader-package source overlay. Portable consumer-pin adoption is a separate
integration step: Cargo refuses a Git patch pointing to another revision of
the same repository. Do not describe the current locked Cue cohort as fixed
until that integration is recorded. The earlier handoff receipt remains
unchanged as historical evidence of the failure.

The explicit-layout rules are documented in the
[Khronos SPIR-V guide](https://github.com/KhronosGroup/SPIRV-Guide/blob/main/chapters/explicit_layout.md)
and [Vulkan shader validation rules](https://docs.vulkan.org/spec/latest/appendices/spirvenv.html#VUID-StandaloneSpirv-None-10684).
