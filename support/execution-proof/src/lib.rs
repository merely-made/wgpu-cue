// SPDX-License-Identifier: MPL-2.0
//! Private native proof, not a supported IPC format or sandbox implementation.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};
use webcue::{execution::*, *};
use wgpu_cue_genet::{GenetDocument, HeldResources};
mod local;
pub use local::Local;
pub mod process;
#[cfg(feature = "cpu-raster")]
pub mod raster;

pub const MAX_FRAME: usize = 2 * 1024 * 1024;
const VERSION: u32 = 1;
pub fn viewport() -> Viewport {
    Viewport {
        width: 640,
        height: 480,
        scale: 1.0,
    }
}
pub fn document() -> Result<GenetDocument> {
    GenetDocument::new(
        viewport(),
        "https://fixture.invalid/".into(),
        "<!doctype html><title>Initial</title><p>initial</p>".into(),
        HeldResources::default(),
    )
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Identity {
    owner: u128,
    generation: u64,
    incarnation: u64,
}
impl From<ExecutionId> for Identity {
    fn from(id: ExecutionId) -> Self {
        Self {
            owner: id.scope.owner,
            generation: id.scope.generation,
            incarnation: id.incarnation,
        }
    }
}
impl From<Identity> for ExecutionId {
    fn from(id: Identity) -> Self {
        Self {
            scope: Scope {
                owner: id.owner,
                generation: id.generation,
            },
            incarnation: id.incarnation,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub enum Work {
    Load { address: String, html: String },
    Find { text: String, backwards: bool },
    Zoom(f32),
    Reload,
}
impl Work {
    fn from_command(command: &Command) -> Result<Self> {
        Ok(match command {
            Command::LoadHtml { address, html } => Self::Load {
                address: address.clone(),
                html: html.clone(),
            },
            Command::Find { text, backwards } => Self::Find {
                text: text.clone(),
                backwards: *backwards,
            },
            Command::Zoom(factor) => Self::Zoom(*factor),
            Command::Reload => Self::Reload,
            _ => {
                return Err(Error::Unsupported(
                    "private proof forwards load/find/zoom/reload only".into(),
                ));
            }
        })
    }
    fn command(self) -> Command {
        match self {
            Self::Load { address, html } => Command::LoadHtml { address, html },
            Self::Find { text, backwards } => Command::Find { text, backwards },
            Self::Zoom(factor) => Command::Zoom(factor),
            Self::Reload => Command::Reload,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct Request {
    version: u32,
    identity: Identity,
    request: u64,
    work: Work,
}
impl Request {
    pub fn from_request(request: &ExecutionRequest) -> Result<Self> {
        if request.execution.scope != request.request.scope {
            return Err(Error::Stale);
        }
        Ok(Self {
            version: VERSION,
            identity: request.execution.into(),
            request: request.request.id.0,
            work: Work::from_command(&request.command)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
enum Observation {
    Holding,
    NavigationStarted(String),
    Address(String),
    Title(String),
    NavigationFinished { address: String, success: bool },
    Done,
    Find(DocumentFindState),
    Zoom(DocumentZoomState),
    Failed(String),
}
#[derive(Serialize, Deserialize)]
pub struct Update {
    version: u32,
    identity: Identity,
    request: u64,
    sequence: u64,
    event: Observation,
}
impl Update {
    pub fn into_update(self) -> Result<ExecutionUpdate> {
        if self.version != VERSION {
            return Err(Error::Invalid("proof version mismatch".into()));
        }
        let execution: ExecutionId = self.identity.into();
        let key = RequestKey {
            scope: execution.scope,
            id: RequestId(self.request),
        };
        let complete = |result| Event::Completed {
            request: key,
            result,
        };
        let event = match self.event {
            Observation::Holding => Event::Message("proof hold entered".into()),
            Observation::NavigationStarted(address) => Event::NavigationStarted { address },
            Observation::Address(address) => Event::AddressChanged(address),
            Observation::Title(title) => Event::TitleChanged(title),
            Observation::NavigationFinished { address, success } => {
                Event::NavigationFinished { address, success }
            }
            Observation::Done => complete(Ok(Reply::Done)),
            Observation::Find(value) => complete(Ok(Reply::Find(value))),
            Observation::Zoom(value) => complete(Ok(Reply::Zoom(value))),
            Observation::Failed(error) => complete(Err(Error::Backend(error))),
        };
        Ok(ExecutionUpdate {
            execution,
            sequence: self.sequence,
            event,
        })
    }
}

pub struct Executor {
    document: GenetDocument,
    identity: Option<ExecutionId>,
    last_request: Option<u64>,
    sequence: u64,
}
impl Executor {
    pub fn new() -> Result<Self> {
        let mut document = document()?;
        document.prepare(viewport())?;
        Ok(Self {
            document,
            identity: None,
            last_request: None,
            sequence: 0,
        })
    }
    fn admit(&mut self, request: &Request) -> Result<()> {
        let identity: ExecutionId = request.identity.into();
        if request.version != VERSION
            || self.identity.is_some_and(|id| id != identity)
            || self
                .last_request
                .is_some_and(|last| request.request <= last)
        {
            return Err(Error::Invalid(
                "invalid proof request identity/version/order".into(),
            ));
        }
        self.identity = Some(identity);
        self.last_request = Some(request.request);
        Ok(())
    }
    /// A bounded witness that the real helper entered the injected stall.
    pub fn hold(&mut self, request: Request) -> Result<Update> {
        self.admit(&request)?;
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Exhausted)?;
        Ok(Update {
            version: VERSION,
            identity: request.identity,
            request: request.request,
            sequence: self.sequence,
            event: Observation::Holding,
        })
    }
    pub fn execute(&mut self, request: Request) -> Result<Vec<Update>> {
        self.admit(&request)?;
        let mut events = Vec::new();
        let result = self
            .document
            .execute(request.work.command(), viewport())
            .and_then(|result| {
                self.document.prepare(viewport())?;
                Ok(result)
            });
        match result {
            Ok(result) => {
                for event in result.observations {
                    events.push(match event {
                        Event::NavigationStarted { address } => {
                            Observation::NavigationStarted(address)
                        }
                        Event::AddressChanged(address) => Observation::Address(address),
                        Event::TitleChanged(title) => Observation::Title(title),
                        Event::NavigationFinished { address, success } => {
                            Observation::NavigationFinished { address, success }
                        }
                        _ => {
                            return Err(Error::Unsupported(
                                "observation is outside proof format".into(),
                            ));
                        }
                    });
                }
                events.push(match result.reply {
                    Reply::Done => Observation::Done,
                    Reply::Find(value) => Observation::Find(value),
                    Reply::Zoom(value) => Observation::Zoom(value),
                    _ => return Err(Error::Unsupported("reply is outside proof format".into())),
                });
            }
            Err(error) => events.push(Observation::Failed(error.to_string())),
        }
        events
            .into_iter()
            .map(|event| {
                self.sequence = self.sequence.checked_add(1).ok_or(Error::Exhausted)?;
                Ok(Update {
                    version: VERSION,
                    identity: request.identity,
                    request: request.request,
                    sequence: self.sequence,
                    event,
                })
            })
            .collect()
    }
}

struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized proof frame",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    encode_with_limit(value, MAX_FRAME)
}
pub fn encode_with_limit<T: Serialize>(value: &T, limit: usize) -> io::Result<Vec<u8>> {
    let mut out = Limited {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value).map_err(io::Error::other)?;
    Ok(out.bytes)
}
pub fn write_frame(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    write_frame_with_limit(out, bytes, MAX_FRAME)
}
pub fn write_frame_with_limit(out: &mut impl Write, bytes: &[u8], limit: usize) -> io::Result<()> {
    if bytes.len() > limit || bytes.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized proof frame",
        ));
    }
    out.write_all(&(bytes.len() as u32).to_be_bytes())?;
    out.write_all(bytes)?;
    out.flush()
}
pub fn read_frame<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<Option<T>> {
    read_frame_with_limit(input, MAX_FRAME)
}
pub fn read_frame_with_limit<T: DeserializeOwned>(
    input: &mut impl Read,
    limit: usize,
) -> io::Result<Option<T>> {
    let mut length = [0; 4];
    if input.read(&mut length[..1])? == 0 {
        return Ok(None);
    }
    input.read_exact(&mut length[1..])?;
    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized proof frame",
        ));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_refuses_oversize_truncation_and_bad_json() {
        let bytes = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(read_frame::<Request>(&mut bytes.as_slice()).is_err());
        assert!(read_frame::<Request>(&mut [0, 0, 0, 5, b'{'].as_slice()).is_err());
        assert!(read_frame::<Request>(&mut [0, 0, 0, 1, b'!'].as_slice()).is_err());
        assert!(encode(&"x".repeat(MAX_FRAME)).is_err());
    }
}

#[cfg(all(feature = "metal-handoff", target_os = "macos"))]
pub mod metal_handoff;

#[cfg(feature = "cpu-raster")]
pub mod render_stream;
#[cfg(any(
    feature = "cpu-raster",
    all(feature = "dx12-handoff", target_os = "windows"),
    all(feature = "vulkan-handoff", target_os = "linux"),
    all(feature = "metal-handoff", target_os = "macos")
))]
mod render_wire;

#[cfg(all(feature = "vulkan-handoff", target_os = "linux"))]
pub mod vulkan_handoff;

#[cfg(all(feature = "dx12-handoff", target_os = "windows"))]
pub mod dx12_handoff;
