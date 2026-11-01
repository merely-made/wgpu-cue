// SPDX-License-Identifier: MPL-2.0
//! Static Genet document commands, independent of GPU ownership and transport.
use document_session_api::{
    DocumentFindQuery, DocumentSession, SessionEngine, SessionSpawnRequest,
};
use genet_documents::{LiverySessionEngine, ResourceFetcher};
use genet_host_api::{ResourceFetchResponse, ResourceRequest};
use netrender::Scene;
use webcue::{Command, Error, Event, Reply, Result, Viewport};

/// A finite host-held resource inventory; no network or file access is performed.
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

/// The same document owner can execute locally or inside a host-supplied helper.
/// Construction and commands perform CPU work; they do not initialize a GPU.
pub struct GenetDocument {
    engine: LiverySessionEngine<HeldResources>,
    pub(crate) session: Box<dyn DocumentSession<Scene>>,
    address: String,
    html: String,
}

pub struct DocumentCommandResult {
    /// Observations must be delivered before the corresponding terminal reply.
    pub observations: Vec<Event>,
    pub reply: Reply,
}

fn backend(error: impl std::fmt::Display) -> Error {
    Error::Backend(error.to_string())
}

impl GenetDocument {
    /// Prepare CPU layout and a scene. Retained find geometry depends on this;
    /// producing a scene does not submit GPU work or establish presentation.
    pub fn prepare(&mut self, viewport: Viewport) -> Result<Scene> {
        let viewport = viewport.validate()?;
        self.session
            .try_frame(
                (viewport.width as f32 / viewport.scale).ceil() as u32,
                (viewport.height as f32 / viewport.scale).ceil() as u32,
            )
            .map_err(backend)
    }

    pub fn new(
        viewport: Viewport,
        address: String,
        html: String,
        resources: HeldResources,
    ) -> Result<Self> {
        let viewport = viewport.validate()?;
        Self::validate_html(&html)?;
        let engine = LiverySessionEngine::new(resources);
        let session = engine
            .spawn(&Self::spawn_request(viewport, &address, &html))
            .map_err(backend)?;
        Ok(Self {
            engine,
            session,
            address,
            html,
        })
    }

    fn validate_html(html: &str) -> Result<()> {
        if html.len() > 1024 * 1024 {
            return Err(Error::Invalid("held HTML exceeds 1 MiB".into()));
        }
        Ok(())
    }

    fn spawn_request(viewport: Viewport, address: &str, html: &str) -> SessionSpawnRequest {
        SessionSpawnRequest::new(address)
            .with_body(html)
            .with_content_type("text/html")
            .with_viewport(viewport.width, viewport.height)
    }

    pub fn execute(
        &mut self,
        command: Command,
        viewport: Viewport,
    ) -> Result<DocumentCommandResult> {
        let viewport = viewport.validate()?;
        let mut observations = Vec::new();
        let reply = match command {
            Command::Navigate { address } => {
                observations.push(Event::NavigationRequested { address });
                Reply::Done
            }
            Command::LoadHtml { address, html } => return self.replace(address, html, viewport),
            Command::Reload => {
                return self.replace(self.address.clone(), self.html.clone(), viewport);
            }
            Command::Find { text, backwards } => {
                if backwards {
                    return Err(Error::Unsupported(
                        "reverse find stepping is not exposed".into(),
                    ));
                }
                Reply::Find(
                    self.session
                        .document_find(&DocumentFindQuery::new(text))
                        .map_err(backend)?,
                )
            }
            Command::Zoom(factor) => {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(Error::Invalid("zoom must be finite and positive".into()));
                }
                Reply::Zoom(self.session.set_page_zoom(factor).map_err(backend)?)
            }
            _ => {
                return Err(Error::Unsupported(
                    "operation is not exposed by the document-command component".into(),
                ));
            }
        };
        Ok(DocumentCommandResult {
            observations,
            reply,
        })
    }

    fn replace(
        &mut self,
        address: String,
        html: String,
        viewport: Viewport,
    ) -> Result<DocumentCommandResult> {
        Self::validate_html(&html)?;
        let session = self
            .engine
            .spawn(&Self::spawn_request(viewport, &address, &html))
            .map_err(backend)?;
        self.session = session;
        self.address = address.clone();
        self.html = html;
        let mut observations = vec![
            Event::NavigationStarted {
                address: address.clone(),
            },
            Event::AddressChanged(address.clone()),
        ];
        if let Some(title) = self.session.inspect().and_then(|report| report.title) {
            observations.push(Event::TitleChanged(title));
        }
        observations.push(Event::NavigationFinished {
            address,
            success: true,
        });
        Ok(DocumentCommandResult {
            observations,
            reply: Reply::Done,
        })
    }
}
