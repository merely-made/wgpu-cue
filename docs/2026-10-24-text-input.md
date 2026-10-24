# Shared Genet text-input geometry, 2026-10-24

The retained Genet producer publishes the neutral `TextInput` observation after
preparing current layout, including when semantic publication is disabled.
`document-session-api` 0.1.1 owns the renderer-free state and rectangle;
Webcue 0.1.2 reexports them under its existing names. Wgpu Cue 0.1.3 uses that
control plane. The concrete Genet adapter pins Genet
`4532fe03755bdb61eee7847ebeb8ad47bda37d96`.

The rectangle includes document scroll and page zoom in logical viewport units.
The receiving host adds its admitted placement and applies device scale once.
Selection offsets use UTF-16; the independent preedit selection remains UTF-8.
Field text and document/native identities do not enter the observation.

Publishing a live editor reserves its eventual clearing event and sequence
number. Unchanged observations deduplicate; failed publication preserves the
protocol's last successful observation. A producer can revoke that observation
using its reserved slot when old geometry becomes unusable. Edit, hide,
viewport invalidation and retirement clear the Genet adapter's published state.

Executed on macOS with Rust 1.99.0:

- Genet's focused geometry suite: three passing static/scripted tests, covering
  selection and emoji offsets, preedit and commit, scrolling, zoom, resize,
  writable-state invalidation, empty/multiline/password fields, blur and detach.
  A focused-field removal reproduced a stale editor access; the repaired frame
  path revokes that editor before reading its value or publishing geometry.
- `cargo test --workspace --offline --locked -j2`: fourteen passing ordinary
  tests; native GPU tests remain explicitly ignored by that default command.
- The new `shared_genet_text_input_geometry_without_semantic_publication`
  fixture passes explicitly on AMD Radeon Pro Vega 56 / Metal, rejecting a CPU
  adapter. It checks logical geometry at device scale 2, input hints, UTF-16
  commit offsets, clearing and retirement, with semantics disabled. The tested
  executable SHA-256 is
  `2f25d86b2a22661f8f56fd5d455abff5ed7bf191f69b2cebea0d7cd072bb64a4`.
- The existing `real_genet_receiver_lifecycle_ime_and_pixels` regression passes
  on Metal, including actual receiving pixels, alpha, stale-action refusal,
  canonical composition and accepted-request completion under a load flood.
  Its ICU4X complex-script model diagnostic remains part of the evidence; this
  does not qualify physical CJK input methods.
- The Webcue package dry run verifies against the published registry version
  of `document-session-api`, without requiring a development Git patch.

`cargo clippy --workspace --all-targets --offline --locked -j2 -- -D warnings`
also passes. The renderer-free Webcue core checks for
`wasm32-unknown-unknown` on Rust 1.99.1. `document-session-api` 0.1.1,
`webcue` 0.1.2 and `wgpu-cue` 0.1.3 are published; each package completed
registry dependency verification before upload.

The new observation and the existing physical Typing Booster receipt establish
different boundaries. These programmatic checks do not requalify an OS candidate
popup, human accessibility operation, or physical keyboard input. Pelt and
Turnstone receiving are recorded by their own source-bound checks.
