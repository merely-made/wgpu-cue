// SPDX-License-Identifier: MPL-2.0
use crate::{Error, Result};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    Navigation,
    InlineHtml,
    Reload,
    Stop,
    History,
    Find,
    Zoom,
    Capture,
    ScriptResult,
    CookieRead,
    Mouse,
    Scroll,
    Keyboard,
    NativeKeyboard,
    Text,
    Ime,
    Focus,
    Touch,
    Pen,
    Drag,
    Semantics,
    SemanticActions,
    ResourceInterception,
    Downloads,
    Permissions,
    Authentication,
    Popups,
    Devtools,
}
impl Feature {
    pub const ALL: [Self; 28] = [
        Self::Navigation,
        Self::InlineHtml,
        Self::Reload,
        Self::Stop,
        Self::History,
        Self::Find,
        Self::Zoom,
        Self::Capture,
        Self::ScriptResult,
        Self::CookieRead,
        Self::Mouse,
        Self::Scroll,
        Self::Keyboard,
        Self::NativeKeyboard,
        Self::Text,
        Self::Ime,
        Self::Focus,
        Self::Touch,
        Self::Pen,
        Self::Drag,
        Self::Semantics,
        Self::SemanticActions,
        Self::ResourceInterception,
        Self::Downloads,
        Self::Permissions,
        Self::Authentication,
        Self::Popups,
        Self::Devtools,
    ];
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    Supported,
    Partial(String),
    Unsupported(String),
}
impl Support {
    pub fn permits(&self) -> bool {
        !matches!(self, Self::Unsupported(_))
    }
    fn validate(&self) -> Result<()> {
        if matches!(self, Self::Partial(s)|Self::Unsupported(s) if s.trim().is_empty()) {
            return Err(Error::Invalid(
                "capability limitations need a reason".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationMode {
    NativeFrame,
    HostDevice,
    CpuRgba,
    NativeOverlay,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticProtocol {
    Unavailable,
    DocumentSnapshot,
    AccesskitTree,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    BuildAvailable,
    Constructed,
    Observed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendIdentity {
    pub engine: String,
    pub version: Option<String>,
    pub platform: String,
    pub build_features: Vec<String>,
}
/// The effective installed adapter report, not the supplier's theoretical maximum.
/// No Default: every feature must be named exactly once with explicit limitations.
#[derive(Clone, Debug)]
pub struct CapabilityReport {
    pub identity: BackendIdentity,
    pub presentation: PresentationMode,
    pub evidence: Evidence,
    pub semantics: SemanticProtocol,
    features: BTreeMap<Feature, Support>,
}
impl CapabilityReport {
    pub fn new(
        identity: BackendIdentity,
        presentation: PresentationMode,
        evidence: Evidence,
        semantics: SemanticProtocol,
        features: impl IntoIterator<Item = (Feature, Support)>,
    ) -> Result<Self> {
        if identity.engine.trim().is_empty() || identity.platform.trim().is_empty() {
            return Err(Error::Invalid(
                "backend identity and platform are required".into(),
            ));
        }
        let mut entries = BTreeMap::new();
        for (f, s) in features {
            s.validate()?;
            if entries.insert(f, s).is_some() {
                return Err(Error::Invalid("duplicate capability".into()));
            }
        }
        if entries.len() != Feature::ALL.len() {
            return Err(Error::Invalid("incomplete capability report".into()));
        }
        let semantic_support = entries[&Feature::Semantics].permits();
        if semantic_support != (semantics != SemanticProtocol::Unavailable)
            || (!semantic_support && entries[&Feature::SemanticActions].permits())
        {
            return Err(Error::Invalid(
                "semantic protocol and capabilities disagree".into(),
            ));
        }
        Ok(Self {
            identity,
            presentation,
            evidence,
            semantics,
            features: entries,
        })
    }
    pub fn feature(&self, feature: Feature) -> &Support {
        &self.features[&feature]
    }
    pub fn features(&self) -> impl Iterator<Item = (Feature, &Support)> {
        self.features.iter().map(|(f, s)| (*f, s))
    }
    pub fn require(&self, feature: Feature) -> Result<()> {
        match self.feature(feature) {
            Support::Unsupported(reason) => Err(Error::Unsupported(reason.clone())),
            _ => Ok(()),
        }
    }
}
