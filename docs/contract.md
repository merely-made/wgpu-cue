# Contract and ownership

The common `Surface` is deliberately single-owner. It has no `Send` or `Sync`
bounds: a WKWebView producer can remain on the main thread. Construction may
initialize a runtime; ordinary controls, event reads and frame acquisition
must not wait for an engine callback, first paint, external I/O or GPU work.
Layout and command execution still consume CPU time. The first Genet adapter
uses a finite host-held resource map, static HTML, a one-MiB HTML limit and
one command per engine turn. It does not add network transport.

The host owns wake policy, monotonic time, viewport placement, focus routing,
history and acquisition policy, native input clients, OS accessibility adapters,
card/site selection and fallback decisions. No central engine registry or
factory is required by the contract.

## Commands and events

A `Scope` is a host-chosen owner plus installation generation. The host must
use a new generation when replacing a producer, including when choosing another
engine for the same card. `RequestId`s strictly increase within a scope. A
rejected request has no completion and can be retried using the same ID. An
accepted ID cannot be reused. `RequestKey` carries both scope and caller ID;
a late callback from a removed installation cannot settle its replacement.

`Protocol` is a bounded mailbox. Each accepted request reserves storage and a
sequence number for its completion. Observations cannot take those slots.
Both concrete adapters cap pending
requests at half the mailbox capacity, preserving observation headroom so a
completion that follows navigation events can still make progress.
Retirement preserves already queued observations, cancels outstanding requests
in acceptance order, rejects further work, and is idempotent. Event sequence
numbers describe this single ordered queue; receivers must not additionally
drain a supplier's legacy navigation/message pollers.

Replies preserve the operation's meaning. `Done` means a retained operation
was applied. `Dispatched` explicitly means an asynchronous browser accepted a
navigation dispatch; navigation success or failure is a later observation.
WKWebView's supplier provides no navigation IDs, so its report states that
limitation instead of associating an arbitrary navigation callback with a
request. Script evaluation uses the supplier's correlated completion IDs.
Host-chosen timeouts may retire a stalled installation; the contract does not
invent successful outcomes when callbacks never arrive.

Canonical logical key input, keyboard-produced text, independent text and IME
are distinct. A native key envelope additionally carries platform key codes
and produced characters for system webviews. Hosts must choose the applicable
path and avoid sending the same edit twice. Logical pointer/scroll coordinates
and semantic geometry are relative to the viewport. `Viewport` dimensions are
physical pixels; `scale` maps logical pixels to them. Genet's full input result
is retained as an event, including pointer-capture and editable-focus hints.
Unsupported input paths return explicit errors.

## Frame directions

`FrameSource` has a required non-blocking method. Its native variant contains
the canonical move-owned `grafting-frame::OwnedNativeFrame`; no borrowed OS
handle escapes. CPU RGBA is an explicit transport with checked extent, stride
and length. Native overlays are explicitly named, and the GPU presenter refuses
them because they require a native child host.

`TextureSource` receives references to the host's device and queue. The source
may own renderer state, but the trait does not require Genet's `RenderCore` or
any other engine implementation. `Changed`, `Unchanged`, and `Pending` are
distinct. `Unchanged` is valid only while the host has a reusable frame.

`GpuHost` identifies a device lifetime. `FrameStamp` separates the producer
installation, resource allocation epoch and content revision. The presenter
validates scope, device epoch, viewport extent, a sampleable non-MSAA 2D target,
monotonic content, and replacement allocation. Resizing or changing device
invalidates retained presentation; changing scope also resets that installation's
revision history. A new device is a new `GpuHost`, even if a wrapper reuses the
same numeric device identifier.

`NativeImporter` either produces a texture or returns the entire still-owned
frame as `Pending`. It cannot consume a descriptor and leave an unusable retry
token. A newer frame replaces an older pending frame. The Metal adapter queries
shared-event readiness without a CPU wait, preserves custody until ready,
checks the texture's declared shape/format and delegates import to Graft. It
supports retained Metal textures only. `FrameSync::None` requires the producer
to have completed writes under a documented ordering relationship; import does
not manufacture synchronization. Producer images must remain stable while
held by the consumer. Non-Metal synchronization and other image kinds are
explicitly unavailable in this importer.

The WK native capture configuration must declare its alpha and encoding
conventions explicitly; this unqualified path infers neither from an engine
name. Encoding describes the stored image channels; a host must also account for
whether the texture format performs sRGB decoding on sampling. Alpha mode is
explicit. There is no hidden copy or transport fallback. CPU uploads allocate
a replacement texture, while native imports preserve native resource custody.

Genet submits to a fresh host-device target and polls deferred Classic quality
results on later turns. It does not call `RenderCore`'s synchronous checked
raster convenience method, which can wait five seconds. A pending texture is
kept private until the quality observer reports no outstanding counters. The
`wgpu-cue-netrender` adapter owns a private renderer and quality observer on the
host's existing device/queue. Other producers and the shell may draw and poll
their own renderers independently. This uses separate renderer caches without
booting another GPU device. Allocation epochs remain monotonic across private
renderer reconstruction; receiving hosts retain content revisions with their
session. GPU error scopes are popped immediately and their
futures are polled on later turns; unresolved scopes keep the frame private.
Viewport scale, output extent and device epoch must still match a pending
frame. Native two-producer receiving is qualified in Mere; an asynchronous
WebGPU renderer adapter remains a separate integration.

## Semantics and capabilities

A report explicitly names `DocumentSnapshot`, `AccesskitTree`, or `Unavailable`.
The semantic protocol must agree with the reported semantic features. A full
retained document projection and ordered AccessKit tree updates retain their
own representations; a host cannot reinterpret a tree delta as a snapshot.
The tree producer must identify initialization and preserve update order. Its
adapter remains responsible for the supplier tree's validity.

A `PublicationKey` contains scope and a monotonically increasing publication
revision. Actions must match the current publication; retained document actions
also carry the underlying projection revision and node identity. Retirement,
input, viewport changes, content replacement and changed semantic state expire
the old publication before a new one is issued. Node-number reuse cannot revive
an old action. Pixel readiness and semantic lifetime are independent.

The initial Genet adapter exposes retained projections and their advertised
actions, without an OS accessibility bridge. WKWebView exposes no semantic
transport through Scry. Its IME report is unavailable: native key characters
and DOM focus acknowledgment do not prove composition forwarding or candidate
geometry. Typed Scry editable-state events are forwarded if the supplier emits
them, but its current WK producer does not emit those events. The independent
receiver makes no caret claim.

`Support::Partial` and `Unsupported` require non-empty explanations. There is
no capability default, and every member of `Feature::ALL` must appear once.
Frame observation promotes the live report from constructed to observed, but
that status does not claim every supported feature has been exercised. Per
feature execution evidence belongs in a receipt, not in an inferred engine
ranking.


## Host adapter receiving (0.1.1)

`dispatch_document_input` and `document_input_events` supply the canonical
retained input dispatch and ordered result/cursor/navigation/form observations.
Both Inker and the standalone Genet adapter use them. Unsupported native key
fields, non-finite positions and unrepresentable scroll modifiers are refused.
Hosts use the full DocumentInput result for capture/editability/admission and
apply navigation/submission policy once from the dedicated events.

`Protocol::ensure_observations` checks bounded observation capacity before an
adapter mutates its receiver. The single owner emits the checked batch before
accepting other work. It cannot consume slots or sequence numbers reserved for
accepted request completions. `retirement_len` lets a receiving host reserve an
old scope's final stream before a successful installation replacement.

Inker's retained adapter owns the existing canonical session; Pelt keeps
acquisition, history and resource delivery. Pelt's shared-content wrapper exposes
`Source::Native` and `Source::HostDevice` through one control object. Browser
platform bindings preserve their richer extensions and require the engine/host
pair to supply an explicit non-blocking, owned frame conversion. The adapter
refuses fields it cannot represent, including native composition state and
independent key character/repeat fields. Unknown rich browser callbacks retain
their serialized typed payload as a Message; they are not silently discarded.
Legacy cursor callbacks have their own supplier queue and are sampled after
the ordered browser callback stream; no cross-queue callback chronology is
claimed. Native AT remains a platform extension for those bindings.


## Text-input observations

`Event::TextInput(Some(state))` describes a focused writable editor using the
canonical `document-session-api` types, reexported as `TextInputState` and
`LogicalRect`. Producer coordinates are logical viewport coordinates after page
zoom and scroll. Host window placement and device scale are applied once by the
receiver. Selection offsets are UTF-16 code units of the editing value; preedit
selection remains independent UTF-8 byte offsets. Values and native identities
are excluded.

Genet observes completed current layout without triggering layout from the
getter. Its adapters publish after preparation even when semantic publication
is disabled. Edits and viewport changes revoke previous geometry until another
frame is prepared. Hide, blur, replacement and retirement clear the observation.
Unchanged observations are deduplicated. A live editor reserves one mailbox slot
and sequence number for its eventual clearing event. Publication backpressure
preserves the previous successfully published state; ordinary input admission
and completion reservations remain in force. `Protocol::text_input_state()`
returns that same successfully published observation for a single-owner broker
that must inspect it without consuming the event stream twice.
