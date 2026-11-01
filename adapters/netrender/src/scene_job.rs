// SPDX-License-Identifier: MPL-2.0
//! Complete owned scenes at the boundary before renderer-specific preparation.
use netrender::{Scene, SceneOp};
use std::{
    io::{self, Write},
    sync::Arc,
};
use webcue::*;

#[derive(Clone, Copy, Debug)]
pub struct RenderLimits {
    pub snapshot_bytes: usize,
    pub resource_bytes: usize,
    pub frame_bytes: usize,
}
impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            snapshot_bytes: 64 * 1024 * 1024,
            resource_bytes: 32 * 1024 * 1024,
            frame_bytes: 16 * 1024 * 1024,
        }
    }
}
#[derive(Debug)]
pub enum RenderError {
    Control(Error),
    Snapshot(String),
    UnresolvedResource(&'static str),
    Budget(&'static str),
    #[cfg(feature = "cpu")]
    Raster(netrender::vello_backends::SparseSessionError),
}
impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CPU render: {self:?}")
    }
}
impl std::error::Error for RenderError {}
impl From<Error> for RenderError {
    fn from(error: Error) -> Self {
        Self::Control(error)
    }
}
pub type RenderResult<T> = std::result::Result<T, RenderError>;

pub(crate) fn frame_bytes(width: u32, height: u32, limits: RenderLimits) -> RenderResult<usize> {
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(RenderError::Budget("frame size overflow"))?;
    if width == 0 || height == 0 || bytes > limits.frame_bytes {
        return Err(RenderError::Budget("frame pixels"));
    }
    Ok(bytes)
}

fn closed_resources(scene: &Scene, limits: RenderLimits) -> RenderResult<usize> {
    frame_bytes(scene.viewport_width, scene.viewport_height, limits)?;
    if !scene.compositor_surfaces.is_empty() {
        return Err(RenderError::UnresolvedResource("native compositor surface"));
    }
    let mut bytes = 0usize;
    for length in scene
        .fonts
        .iter()
        .map(|font| font.data.as_ref().len())
        .chain(
            scene
                .image_sources
                .values()
                .map(|image| image.data.as_ref().len()),
        )
    {
        bytes = bytes
            .checked_add(length)
            .ok_or(RenderError::Budget("resource bytes overflow"))?;
        if bytes > limits.resource_bytes {
            return Err(RenderError::Budget("font/image source bytes"));
        }
    }
    for op in &scene.ops {
        let key = match op {
            SceneOp::Image(image) => Some(image.key),
            SceneOp::Pattern(pattern) => Some(pattern.tile),
            SceneOp::Fragment(_) => {
                return Err(RenderError::UnresolvedResource("registry-local fragment"));
            }
            _ => None,
        };
        if key.is_some_and(|key| !scene.image_sources.contains_key(&key)) {
            return Err(RenderError::UnresolvedResource(
                "image key without owned pixels",
            ));
        }
    }
    Ok(bytes)
}

struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("snapshot byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A complete owned snapshot in the pinned Netrender serde format. Resource IDs
/// are job-local; bytes confer no origin/storage/resource authorization. The host
/// must resolve grants before capture. Decoding is for trusted, bounded inputs,
/// not a hostile-data parser sandbox or a stable cross-version wire standard.
#[derive(Clone, Debug)]
pub struct SceneSnapshot {
    bytes: Arc<[u8]>,
    width: u32,
    height: u32,
    resource_bytes: usize,
}
impl SceneSnapshot {
    pub fn capture(scene: &Scene, limits: RenderLimits) -> RenderResult<Self> {
        closed_resources(scene, limits)?;
        let mut writer = Limited {
            bytes: Vec::new(),
            limit: limits.snapshot_bytes,
        };
        serde_json::to_writer(&mut writer, scene)
            .map_err(|e| RenderError::Snapshot(e.to_string()))?;
        Self::from_bytes(writer.bytes, limits)
    }
    pub fn from_bytes(bytes: Vec<u8>, limits: RenderLimits) -> RenderResult<Self> {
        if bytes.len() > limits.snapshot_bytes {
            return Err(RenderError::Budget("snapshot bytes"));
        }
        let scene: Scene =
            serde_json::from_slice(&bytes).map_err(|e| RenderError::Snapshot(e.to_string()))?;
        let resource_bytes = closed_resources(&scene, limits)?;
        Ok(Self {
            bytes: bytes.into(),
            width: scene.viewport_width,
            height: scene.viewport_height,
            resource_bytes,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn size(&self) -> FrameSize {
        FrameSize::new(self.width, self.height)
    }
    pub fn resource_bytes(&self) -> usize {
        self.resource_bytes
    }
    pub fn decode(&self, limits: RenderLimits) -> RenderResult<Scene> {
        if self.bytes.len() > limits.snapshot_bytes {
            return Err(RenderError::Budget("snapshot bytes"));
        }
        let mut scene: Scene = serde_json::from_slice(&self.bytes)
            .map_err(|e| RenderError::Snapshot(e.to_string()))?;
        closed_resources(&scene, limits)?;
        // Netrender capture preserves Blob IDs for deterministic replay. A new
        // execution namespace must mint fresh IDs before using renderer caches.
        // Font slot zero is the provider's reserved empty sentinel.
        for (index, font) in scene.fonts.iter_mut().enumerate() {
            if index == 0 && font.index == 0 && font.data.as_ref().is_empty() {
                *font = Scene::new(1, 1).fonts[0].clone();
            } else {
                font.data = netrender::peniko::Blob::new(Arc::new(font.data.as_ref().to_vec()));
            }
        }
        for image in scene.image_sources.values_mut() {
            image.data = netrender::peniko::Blob::new(Arc::new(image.data.as_ref().to_vec()));
        }
        Ok(scene)
    }
}

/// Immutable job identity, distinct from document commands and their effects.
/// New full snapshots supersede older jobs at the receiving endpoint.
#[derive(Clone, Debug)]
pub struct RenderJob {
    pub stamp: FrameStamp,
    pub snapshot: SceneSnapshot,
}
