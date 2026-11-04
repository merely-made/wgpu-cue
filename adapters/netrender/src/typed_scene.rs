// SPDX-License-Identifier: MPL-2.0
use crate::{Scene, scene_job::*};
use netrender::peniko::Blob;
use std::{
    io::{self, Write},
    sync::Arc,
};

pub(crate) fn rebind_shared(scene: &mut Scene) {
    for (index, font) in scene.fonts.iter_mut().enumerate() {
        if index == 0 && font.index == 0 && font.data.is_empty() {
            *font = Scene::new(1, 1).fonts[0].clone();
        } else {
            font.data = Blob::new(font.data.clone().into_raw_parts().0);
        }
    }
    for image in scene.image_sources.values_mut() {
        image.data = Blob::new(image.data.clone().into_raw_parts().0);
    }
}
pub(crate) fn hollow(scene: &Scene) -> Scene {
    let mut structure = scene.clone();
    let empty: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(Vec::<u8>::new());
    for font in &mut structure.fonts {
        font.data = Blob::from_raw_parts(empty.clone(), font.data.id());
    }
    for image in structure.image_sources.values_mut() {
        image.data = Blob::from_raw_parts(empty.clone(), image.data.id());
    }
    structure
}
struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("typed structure budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// The pinned Blob serializer emits ([u8,...], id). Each nonempty array adds
/// decimal digits plus separators to the two bytes already charged for [].
fn payload_extra(bytes: &[u8]) -> RenderResult<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    // Establish a checked upper bound before the vectorizable digit sum.
    // Each u8 has at most three digits and one separator: total < 4*n.
    bytes
        .len()
        .checked_mul(4)
        .ok_or(RenderError::Budget("resource JSON size overflow"))?;
    Ok(bytes
        .iter()
        .map(|b| {
            if *b < 10 {
                1usize
            } else if *b < 100 {
                2
            } else {
                3
            }
        })
        .sum::<usize>()
        + bytes.len()
        - 1)
}
pub(crate) fn validate_structure(
    scene: &Scene,
    limits: RenderLimits,
) -> RenderResult<(Vec<u8>, usize)> {
    let structure = hollow(scene);
    let mut writer = Limited {
        bytes: vec![],
        limit: limits.snapshot_bytes,
    };
    serde_json::to_writer(&mut writer, &structure)
        .map_err(|e| RenderError::Snapshot(e.to_string()))?;
    let _: Scene =
        serde_json::from_slice(&writer.bytes).map_err(|e| RenderError::Snapshot(e.to_string()))?;
    let mut encoded_len = writer.bytes.len();
    for bytes in scene
        .fonts
        .iter()
        .map(|f| f.data.as_ref())
        .chain(scene.image_sources.values().map(|i| i.data.as_ref()))
    {
        encoded_len = encoded_len
            .checked_add(payload_extra(bytes)?)
            .ok_or(RenderError::Budget("snapshot bytes overflow"))?;
        if encoded_len > limits.snapshot_bytes {
            return Err(RenderError::Budget("snapshot bytes"));
        }
    }
    Ok((writer.bytes, encoded_len))
}
