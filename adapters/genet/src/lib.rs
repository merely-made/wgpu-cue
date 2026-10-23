// SPDX-License-Identifier: MPL-2.0
//! Native retained Genet content behind Webcue; no Pelt/Inker orchestration.
use document_session_api::{
    DocumentFindQuery, DocumentSession, SessionEngine, SessionSpawnRequest,
};
use genet_documents::{LiverySessionEngine, ResourceFetcher};
use genet_host_api::{ResourceFetchResponse, ResourceRequest};
use genet_render_host::RenderCore;
use netrender::Scene;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use webcue::*;
use wgpu_cue::{RenderContext, Rendered, TextureSource};

/// A finite host-held resource inventory. This adapter never performs network or file I/O.
#[derive(Clone, Default)]
pub struct HeldResources(pub std::collections::BTreeMap<String, Vec<u8>>);
impl ResourceFetcher for HeldResources {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        self.0.get(url).cloned()
    }
    fn fetch_request(&self, _: &ResourceRequest) -> Option<ResourceFetchResponse> {
        None
    }
}
pub struct GenetSurface {
    protocol: Protocol,
    report: CapabilityReport,
    viewport: Viewport,
    engine: LiverySessionEngine<HeldResources>,
    session: Box<dyn DocumentSession<Scene>>,
    core: Arc<RenderCore>,
    address: String,
    html: String,
    dirty: bool,
    visible: bool,
    revision: u64,
    scene: Option<Scene>,
    render_dirty: bool,
    rasterizer: wgpu_cue_netrender::SceneRasterizer,
    commands: VecDeque<(RequestKey, Command)>,
    outgoing: VecDeque<Event>,
    completion: Option<(RequestKey, Result<Reply>)>,
    semantics: bool,
    publications: PublicationTracker,
}
fn backend(e: impl std::fmt::Display) -> Error {
    Error::Backend(e.to_string())
}
impl GenetSurface {
    /// Own private renderer/quality state on the core's existing device. The
    /// host and other producers may draw and poll their own renderers freely.
    pub fn new(
        scope: Scope,
        viewport: Viewport,
        address: String,
        html: String,
        resources: HeldResources,
        core: Arc<RenderCore>,
    ) -> Result<Self> {
        let viewport = viewport.validate()?;
        let rasterizer =
            wgpu_cue_netrender::SceneRasterizer::new(core.renderer().wgpu_device.core.clone())?;
        if html.len() > 1024 * 1024 {
            return Err(Error::Invalid(
                "held HTML exceeds the 1 MiB initial adapter limit".into(),
            ));
        }
        let engine = LiverySessionEngine::new(resources);
        let request = SessionSpawnRequest::new(&address)
            .with_body(&html)
            .with_content_type("text/html")
            .with_viewport(viewport.width, viewport.height);
        let session = engine.spawn(&request).map_err(backend)?;
        let identity = BackendIdentity {
            engine: "genet.livery".into(),
            version: Some("729414db135".into()),
            platform: std::env::consts::OS.into(),
            build_features: vec!["livery".into(), "held-resources".into()],
        };
        let features=Feature::ALL.map(|f| (f,match f {
            Feature::InlineHtml|Feature::Reload|Feature::Mouse|Feature::Scroll|Feature::Keyboard|Feature::Text|Feature::Focus|Feature::Zoom=>Support::Supported,
            Feature::Find=>Support::Partial("new forward queries; reverse/step controls are not exposed".into()),
            Feature::Navigation=>Support::Partial("navigation requests are handed to the host; replacement content is supplied through LoadHtml".into()),
            Feature::Ime=>Support::Partial("retained preedit/commit; native candidate-window positioning remains the host's responsibility".into()),
            Feature::Semantics|Feature::SemanticActions=>Support::Partial("retained document projection and its advertised actions; no native OS bridge in this adapter".into()),
            Feature::History|Feature::Stop=>Support::Unsupported("host owns acquisition and browsing history".into()),
            Feature::NativeKeyboard=>Support::Unsupported("send canonical logical keys and independent text/IME".into()),
            Feature::ScriptResult|Feature::CookieRead=>Support::Unsupported("static Livery lane has no script engine or cookie store".into()),
            Feature::Capture=>Support::Unsupported("correlated GPU readback is not implemented".into()),
            Feature::Touch|Feature::Pen|Feature::Drag=>Support::Unsupported("rich pointer and drag forwarding is not implemented".into()),
            Feature::ResourceInterception=>Support::Unsupported("resources are held at construction; no dynamic interception protocol".into()),
            Feature::Downloads|Feature::Permissions|Feature::Authentication|Feature::Popups|Feature::Devtools=>Support::Unsupported("browser extension is not exposed by this retained adapter".into()),
        }));
        let report = CapabilityReport::new(
            identity,
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::DocumentSnapshot,
            features,
        )?;
        Ok(Self {
            protocol: Protocol::new(scope, 256)?,
            report,
            viewport,
            engine,
            session,
            core,
            address,
            html,
            dirty: true,
            visible: true,
            revision: 0,
            scene: None,
            render_dirty: true,
            rasterizer,
            commands: VecDeque::new(),
            outgoing: VecDeque::new(),
            completion: None,
            semantics: false,
            publications: PublicationTracker::new(scope),
        })
    }
    fn flush(&mut self) -> Result<()> {
        while let Some(event) = self.outgoing.front() {
            match self.protocol.emit(event.clone()) {
                Ok(()) => {
                    self.outgoing.pop_front();
                }
                Err(Error::Backpressure) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn admit_controls(&mut self) -> Result<()> {
        self.protocol.ensure_live()?;
        self.flush()?;
        if self.outgoing.is_empty() {
            Ok(())
        } else {
            Err(Error::Backpressure)
        }
    }
    fn invalidate_publication(&mut self) {
        self.rasterizer.suspend();
        if let Some(key) = self.publications.retire() {
            self.outgoing.push_back(Event::SemanticsRetired(key));
        }
    }
    fn prepare(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.scene = Some(
            self.session
                .try_frame(
                    (self.viewport.width as f32 / self.viewport.scale).ceil() as u32,
                    (self.viewport.height as f32 / self.viewport.scale).ceil() as u32,
                )
                .map_err(backend)?,
        );
        self.revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        if self.semantics
            && let Some(projection) = self.session.accessibility_projection()
        {
            let publication = self.publications.publish(self.revision)?;
            self.outgoing.push_back(Event::Semantics {
                publication,
                payload: SemanticPayload::Snapshot(projection),
            });
        }
        self.rasterizer.suspend();
        self.dirty = false;
        self.render_dirty = true;
        Ok(())
    }
    fn execute(&mut self, command: Command) -> Result<Reply> {
        match command {
            Command::Navigate { address } => {
                self.outgoing
                    .push_back(Event::NavigationRequested { address });
                Ok(Reply::Done)
            }
            Command::LoadHtml { address, html } => self.replace(address, html),
            Command::Reload => self.replace(self.address.clone(), self.html.clone()),
            Command::SetSemantics(enabled) => {
                self.invalidate_publication();
                self.semantics = enabled;
                self.dirty = true;
                self.prepare()?;
                Ok(Reply::Done)
            }
            Command::Find { text, backwards } => {
                if backwards {
                    return Err(Error::Unsupported(
                        "reverse find stepping is not exposed in this initial adapter".into(),
                    ));
                }
                let state = self
                    .session
                    .document_find(&DocumentFindQuery::new(text))
                    .map_err(backend)?;
                self.invalidate_publication();
                self.dirty = true;
                Ok(Reply::Find(state))
            }
            Command::Zoom(factor) => {
                let state = self.session.set_page_zoom(factor).map_err(backend)?;
                self.invalidate_publication();
                self.dirty = true;
                Ok(Reply::Zoom(state))
            }
            _ => Err(Error::Unsupported("operation is not exposed".into())),
        }
    }
    fn replace(&mut self, address: String, html: String) -> Result<Reply> {
        if html.len() > 1024 * 1024 {
            return Err(Error::Invalid("held HTML exceeds 1 MiB".into()));
        }
        let request = SessionSpawnRequest::new(&address)
            .with_body(&html)
            .with_content_type("text/html")
            .with_viewport(self.viewport.width, self.viewport.height);
        let session = self.engine.spawn(&request).map_err(backend)?;
        self.invalidate_publication();
        self.session = session;
        self.address = address.clone();
        self.html = html;
        self.dirty = true;
        self.outgoing.push_back(Event::NavigationStarted {
            address: address.clone(),
        });
        self.outgoing
            .push_back(Event::AddressChanged(address.clone()));
        if let Some(title) = self.session.inspect().and_then(|r| r.title) {
            self.outgoing.push_back(Event::TitleChanged(title));
        }
        self.outgoing.push_back(Event::NavigationFinished {
            address,
            success: true,
        });
        Ok(Reply::Done)
    }
}
impl Surface for GenetSurface {
    fn scope(&self) -> Scope {
        self.protocol.scope()
    }
    fn capabilities(&self) -> &CapabilityReport {
        &self.report
    }
    fn set_viewport(&mut self, v: Viewport) -> Result<()> {
        self.admit_controls()?;
        self.viewport = v.validate()?;
        self.invalidate_publication();
        self.dirty = true;
        Ok(())
    }
    fn set_visible(&mut self, visible: bool) -> Result<()> {
        self.protocol.ensure_live()?;
        self.visible = visible;
        self.session.set_hidden(!visible);
        Ok(())
    }
    fn input(&mut self, input: Input) -> Result<()> {
        self.protocol.ensure_live()?;
        if !self.outgoing.is_empty() {
            self.flush()?;
            if !self.outgoing.is_empty() {
                return Err(Error::Backpressure);
            }
        }
        let result = dispatch_document_input(self.session.as_mut(), input)?;
        self.outgoing.extend(document_input_events(result));
        self.invalidate_publication();
        self.dirty = true;
        self.flush()
    }
    fn request(&mut self, id: RequestId, command: Command) -> Result<RequestKey> {
        self.report.require(command.feature())?;
        if matches!(
            &command,
            Command::Find {
                backwards: true,
                ..
            }
        ) {
            return Err(Error::Unsupported(
                "reverse find stepping is not exposed".into(),
            ));
        }
        if let Command::Zoom(f) = &command
            && (!f.is_finite() || *f <= 0.)
        {
            return Err(Error::Invalid("zoom must be finite and positive".into()));
        }
        // Keep observation headroom. If all slots were reserved for terminal
        // events, observations that must precede completion could deadlock.
        if self.protocol.pending() >= 128 {
            return Err(Error::Backpressure);
        }
        let key = self.protocol.accept(id)?;
        self.commands.push_back((key, command));
        Ok(key)
    }
    fn semantic_action(&mut self, action: SemanticAction) -> Result<()> {
        self.admit_controls()?;
        self.publications.validate(action.publication())?;
        let SemanticAction::Document { action, .. } = action else {
            return Err(Error::Unsupported(
                "this producer exposes retained document semantics".into(),
            ));
        };
        if !self.session.dispatch_accessibility_action(&action) {
            return Err(Error::Stale);
        }
        self.invalidate_publication();
        self.dirty = true;
        Ok(())
    }
    fn advance(&mut self, _now: Duration) -> Result<()> {
        self.protocol.ensure_live()?;
        self.flush()?;
        if !self.outgoing.is_empty() {
            return Ok(());
        }
        if self.completion.is_none()
            && let Some((key, command)) = self.commands.pop_front()
        {
            let result = self.execute(command);
            self.completion = Some((key, result));
        }
        if self.completion.is_some() {
            // Keep observations ahead of completion even if a receiver stops draining.
            self.flush()?;
            if !self.outgoing.is_empty() {
                return Ok(());
            }
            let (key, result) = self.completion.take().unwrap();
            self.protocol.settle(key, result)?;
        }
        self.prepare()?;
        self.flush()
    }
    fn poll_event(&mut self) -> Option<Envelope> {
        self.protocol.poll()
    }
    fn retire(&mut self) -> Result<()> {
        self.commands.clear();
        self.completion = None;
        self.outgoing.clear();
        self.publications.retire();
        self.scene = None;
        self.rasterizer.suspend();
        self.protocol.retire()
    }
}
impl TextureSource for GenetSurface {
    fn render(&mut self, cx: &RenderContext<'_>, needs_frame: bool) -> Result<Rendered> {
        self.protocol.ensure_live()?;
        if cx.device != self.core.device() {
            return Err(Error::Invalid(
                "Genet core belongs to a different host device".into(),
            ));
        }
        if !self.visible {
            return Ok(Rendered::Pending);
        }
        if cx.viewport != self.viewport {
            return Err(Error::Invalid("host and producer viewports differ".into()));
        }
        if let Some(loss) = self.core.device_failure() {
            self.rasterizer.suspend();
            return Err(backend(format!("device lost: {loss:?}")));
        }
        if self.dirty {
            self.rasterizer.suspend();
        }
        if let Some(rendered) = self.rasterizer.poll(cx)? {
            if matches!(rendered, Rendered::Changed(_))
                && self.report.evidence != Evidence::Observed
            {
                self.report.evidence = Evidence::Observed;
                self.outgoing
                    .push_back(Event::CapabilitiesChanged(self.report.clone()));
            }
            return Ok(rendered);
        }
        if !self.dirty && !self.render_dirty && !needs_frame {
            return Ok(Rendered::Unchanged);
        }
        self.prepare()?;
        let scene = self
            .scene
            .as_ref()
            .ok_or_else(|| Error::Invalid("no prepared scene".into()))?;
        self.revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        let rendered = self.rasterizer.render(
            cx,
            scene,
            FrameStamp {
                scope: self.protocol.scope(),
                allocation_epoch: 0,
                content_revision: self.revision,
            },
        )?;
        self.render_dirty = false;
        Ok(rendered)
    }
}
