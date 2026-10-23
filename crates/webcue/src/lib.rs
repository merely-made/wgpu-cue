// SPDX-License-Identifier: MPL-2.0
//! One single-owner surface control plane, independent of renderer and wgpu major.
//! Canonical retained input/document/semantic types are reused, not copied.
//! `document-session-api` is renderer-free; its ownership remains with Genet.
#![forbid(unsafe_code)]
mod capabilities;
mod document_input;
pub use document_input::{dispatch_document_input, document_input_events};
mod frame;
mod protocol;
pub use accesskit;
pub use capabilities::*;
/// Full canonical document vocabulary, including types nested in input results,
/// semantic actions, form submissions, find state and document reports.
pub use document_session_api as document;
pub use document_session_api::{
    DocumentA11yActionRequest, DocumentA11yProjection, DocumentCapabilities, DocumentFindState,
    DocumentZoomState, SessionButtonState, SessionFocusDirection, SessionFormSubmission,
    SessionIme, SessionInput, SessionInputResult, SessionKey, SessionModifiers,
    SessionPointerButton,
};
pub use frame::*;
pub use grafting_frame;
pub use protocol::*;

/// Host-chosen owner identity, unique among concurrently installed surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub owner: u128,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestKey {
    pub scope: Scope,
    pub id: RequestId,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported(String),
    Invalid(String),
    Backpressure,
    Retired,
    Stale,
    RequestOrder,
    NotPending,
    Exhausted,
    Backend(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq)]
/// Physical output extent and logical-to-physical pixel scale. Input and
/// semantic geometry use logical pixels relative to this viewport.
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}
impl Viewport {
    pub fn validate(self) -> Result<Self> {
        if self.width == 0 || self.height == 0 || !self.scale.is_finite() || self.scale <= 0.0 {
            return Err(Error::Invalid(
                "viewport dimensions and scale must be positive".into(),
            ));
        }
        Ok(self)
    }
}
/// Optional native key fields supplement the canonical logical key/text/IME API.
#[derive(Clone, Debug)]
pub struct NativeKey {
    pub key_code: u32,
    pub scan_code: u32,
    pub pressed: bool,
    pub characters: String,
    pub characters_ignoring_modifiers: String,
    pub repeat: bool,
    pub modifiers: SessionModifiers,
}
#[derive(Clone, Debug)]
pub enum Input {
    Document(SessionInput),
    NativeKey(NativeKey),
    Scroll {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        modifiers: SessionModifiers,
    },
}
#[derive(Clone, Debug)]
pub enum Command {
    Navigate { address: String },
    LoadHtml { address: String, html: String },
    Reload,
    Stop,
    Back,
    Forward,
    Script(String),
    ReadCookies { address: String },
    Find { text: String, backwards: bool },
    Zoom(f32),
    Capture,
    SetSemantics(bool),
}
impl Command {
    pub fn feature(&self) -> Feature {
        match self {
            Self::Navigate { .. } => Feature::Navigation,
            Self::LoadHtml { .. } => Feature::InlineHtml,
            Self::Reload => Feature::Reload,
            Self::Stop => Feature::Stop,
            Self::Back | Self::Forward => Feature::History,
            Self::Script(_) => Feature::ScriptResult,
            Self::ReadCookies { .. } => Feature::CookieRead,
            Self::Find { .. } => Feature::Find,
            Self::Zoom(_) => Feature::Zoom,
            Self::Capture => Feature::Capture,
            Self::SetSemantics(_) => Feature::Semantics,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Reply {
    /// Dispatch accepted by an asynchronous browser; observe navigation separately.
    Dispatched,
    Done,
    Text(String),
    Find(DocumentFindState),
    Zoom(DocumentZoomState),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationKey {
    pub scope: Scope,
    pub revision: u64,
}
/// A complete retained projection or an ordered supplier tree update. Adapters must
/// declare which form they expose; reading a snapshot is not an atomic action lease.
#[derive(Clone, Debug)]
pub enum SemanticPayload {
    Snapshot(DocumentA11yProjection),
    Tree {
        initialization: bool,
        update: accesskit::TreeUpdate,
    },
}
#[derive(Clone, Debug)]
pub enum SemanticAction {
    Document {
        publication: PublicationKey,
        action: DocumentA11yActionRequest,
    },
    Tree {
        publication: PublicationKey,
        action: accesskit::ActionRequest,
    },
}
impl SemanticAction {
    pub fn publication(&self) -> PublicationKey {
        match self {
            Self::Document { publication, .. } | Self::Tree { publication, .. } => *publication,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Event {
    /// Exact retained result for admission/capture/editability. Navigation and
    /// submission effects also get dedicated events; hosts apply policy once.
    DocumentInput(SessionInputResult),
    TextInput(Option<TextInputState>),
    NavigationStarted {
        address: String,
    },
    AddressChanged(String),
    TitleChanged(String),
    NavigationFinished {
        address: String,
        success: bool,
    },
    NavigationRequested {
        address: String,
    },
    FormSubmitted(SessionFormSubmission),
    CursorChanged(String),
    Message(String),
    InputHandled {
        id: u64,
        consumed: bool,
        default_prevented: bool,
    },
    Semantics {
        publication: PublicationKey,
        payload: SemanticPayload,
    },
    SemanticsRetired(PublicationKey),
    CapabilitiesChanged(CapabilityReport),
    Completed {
        request: RequestKey,
        result: Result<Reply>,
    },
    Diagnostic(String),
}
#[derive(Clone, Debug)]
pub struct Envelope {
    pub scope: Scope,
    pub sequence: u64,
    pub event: Event,
}

/// All accepted operations settle in the same queue as observations. No method
/// may wait for first paint, GPU completion, I/O, or a UI callback. `advance` is a
/// bounded engine turn; the host supplies monotonic time and owns wake policy.
/// Producers may be main-thread-only: no Send/Sync bound is imposed.
pub trait Surface {
    fn scope(&self) -> Scope;
    fn capabilities(&self) -> &CapabilityReport;
    fn set_viewport(&mut self, viewport: Viewport) -> Result<()>;
    fn set_visible(&mut self, visible: bool) -> Result<()>;
    fn input(&mut self, input: Input) -> Result<()>;
    fn request(&mut self, id: RequestId, command: Command) -> Result<RequestKey>;
    fn semantic_action(&mut self, action: SemanticAction) -> Result<()>;
    fn advance(&mut self, now: std::time::Duration) -> Result<()>;
    fn poll_event(&mut self) -> Option<Envelope>;
    /// Idempotent; accepted work is cancelled in order. Drain the final queue.
    fn retire(&mut self) -> Result<()>;
}

/// Focused editable geometry in logical pixels. Selection indices are UTF-16
/// code units, matching system webviews; IME preedit selection stays in the
/// canonical SessionIme coordinate system.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputState {
    pub element_kind: String,
    pub input_type: String,
    pub input_mode: String,
    pub autocomplete: String,
    pub multiline: bool,
    pub password: bool,
    pub selection_utf16: (u32, u32),
    pub caret: LogicalRect,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
