// SPDX-License-Identifier: MPL-2.0
//! Bounded immutable source resources for one ordered renderer incarnation.
//! Tables never evict or reuse tokens. Pressure refuses admission; dropping the
//! incarnation drops table custody while snapshots retain their own Arc owners.
use crate::{Scene, scene_job::*};
use netrender::peniko::Blob;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use webcue::execution::ExecutionId;
const MAX_ENTRIES: usize = 256;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Namespace {
    pub owner: u128,
    pub generation: u64,
    pub incarnation: u64,
}
impl From<ExecutionId> for Namespace {
    fn from(e: ExecutionId) -> Self {
        Self {
            owner: e.scope.owner,
            generation: e.scope.generation,
            incarnation: e.incarnation,
        }
    }
}
impl Namespace {
    pub fn execution(self) -> ExecutionId {
        ExecutionId {
            scope: webcue::Scope {
                owner: self.owner,
                generation: self.generation,
            },
            incarnation: self.incarnation,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upload {
    pub token: u64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourcePacket {
    pub namespace: Namespace,
    pub sequence: u64,
    pub structure: String,
    pub fonts: Vec<Option<u64>>,
    pub images: Vec<(u64, u64)>,
    pub uploads: Vec<Upload>,
}
#[derive(Clone)]
pub struct ResourceSender {
    namespace: Namespace,
    sequence: u64,
    entries: Vec<Blob<u8>>,
    bytes: usize,
    limits: RenderLimits,
}
impl ResourceSender {
    pub fn new(execution: ExecutionId, limits: RenderLimits) -> RenderResult<Self> {
        if execution.incarnation == 0 {
            return Err(webcue::Error::Stale.into());
        }
        Ok(Self {
            namespace: execution.into(),
            sequence: 0,
            entries: vec![],
            bytes: 0,
            limits,
        })
    }
    /// Return a candidate state. The transport must install it only after its
    /// ordered send queue accepts this exact packet. Busy drops the candidate.
    pub fn prepare(&self, snapshot: &SceneSnapshot) -> RenderResult<(ResourcePacket, Self)> {
        crate::scene_profile::measure(
            "resources.prepare_packet",
            snapshot.resource_bytes(),
            || {
                let scene = snapshot.decode(self.limits)?;
                let structure = snapshot.structure().ok_or(RenderError::Snapshot(
                    "resource delivery requires a typed snapshot".into(),
                ))?;
                let mut candidate = self.clone();
                candidate.sequence = candidate
                    .sequence
                    .checked_add(1)
                    .ok_or(webcue::Error::Exhausted)?;
                let mut packet = ResourcePacket {
                    namespace: self.namespace,
                    sequence: candidate.sequence,
                    structure: String::from_utf8(structure.to_vec())
                        .map_err(|e| RenderError::Snapshot(e.to_string()))?,
                    fonts: vec![],
                    images: vec![],
                    uploads: vec![],
                };
                for (i, font) in scene.fonts.iter().enumerate() {
                    packet
                        .fonts
                        .push(if i == 0 && font.index == 0 && font.data.is_empty() {
                            None
                        } else {
                            Some(candidate.token(&font.data, &mut packet.uploads)?)
                        });
                }
                let mut images = scene.image_sources.iter().collect::<Vec<_>>();
                images.sort_by_key(|(k, _)| **k);
                for (key, image) in images {
                    packet
                        .images
                        .push((*key, candidate.token(&image.data, &mut packet.uploads)?));
                }
                Ok((packet, candidate))
            },
        )
    }
    fn token(&mut self, blob: &Blob<u8>, uploads: &mut Vec<Upload>) -> RenderResult<u64> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.as_ref() == blob.as_ref())
        {
            return Ok(index as u64 + 1);
        }
        if self.entries.len() == MAX_ENTRIES
            || blob.len() > self.limits.resource_bytes.saturating_sub(self.bytes)
        {
            return Err(RenderError::Budget("renderer resource table"));
        }
        self.bytes += blob.len();
        self.entries.push(blob.clone());
        let token = self.entries.len() as u64;
        uploads.push(Upload {
            token,
            bytes: blob.as_ref().to_vec(),
        });
        Ok(token)
    }
    pub fn packets(&self) -> u64 {
        self.sequence
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn entries(&self) -> usize {
        self.entries.len()
    }
}
pub struct ResourceReceiver {
    namespace: Namespace,
    sequence: u64,
    entries: BTreeMap<u64, Blob<u8>>,
    bytes: usize,
    limits: RenderLimits,
}
impl ResourceReceiver {
    pub fn new(execution: ExecutionId, limits: RenderLimits) -> RenderResult<Self> {
        if execution.incarnation == 0 {
            return Err(webcue::Error::Stale.into());
        }
        Ok(Self {
            namespace: execution.into(),
            sequence: 0,
            entries: BTreeMap::new(),
            bytes: 0,
            limits,
        })
    }
    /// Transactional: a rejected packet changes neither sequence nor resources.
    /// Foreign serialized Blob IDs are ignored; all data gets local identities.
    pub fn receive(&mut self, packet: ResourcePacket) -> RenderResult<SceneSnapshot> {
        crate::scene_profile::measure("resources.receive_packet", packet.structure.len(), || {
            self.receive_inner(packet)
        })
    }
    fn receive_inner(&mut self, packet: ResourcePacket) -> RenderResult<SceneSnapshot> {
        if packet.namespace != self.namespace
            || self.sequence.checked_add(1) != Some(packet.sequence)
        {
            return Err(webcue::Error::Stale.into());
        }
        if packet.structure.len() > self.limits.snapshot_bytes
            || packet.fonts.len() > packet.structure.len()
            || packet.images.len() > packet.structure.len()
            || packet.uploads.len() > MAX_ENTRIES
        {
            return Err(RenderError::Budget("resource packet"));
        }
        let mut scene: Scene = serde_json::from_str(&packet.structure)
            .map_err(|e| RenderError::Snapshot(e.to_string()))?;
        if scene.fonts.len() != packet.fonts.len()
            || scene.image_sources.len() != packet.images.len()
            || scene.fonts.iter().any(|f| !f.data.is_empty())
            || scene.image_sources.values().any(|i| !i.data.is_empty())
        {
            return Err(RenderError::Snapshot(
                "resource structure shape/payload mismatch".into(),
            ));
        }
        let used = packet
            .fonts
            .iter()
            .flatten()
            .copied()
            .chain(packet.images.iter().map(|(_, token)| *token))
            .collect::<BTreeSet<_>>();
        let mut additions = BTreeMap::new();
        let mut bytes = self.bytes;
        for upload in packet.uploads {
            if upload.token == 0
                || self.entries.contains_key(&upload.token)
                || additions.contains_key(&upload.token)
                || !used.contains(&upload.token)
            {
                return Err(RenderError::Snapshot(
                    "duplicate/redefined/unreferenced resource token".into(),
                ));
            }
            bytes = bytes
                .checked_add(upload.bytes.len())
                .ok_or(RenderError::Budget("resource table overflow"))?;
            if bytes > self.limits.resource_bytes
                || self.entries.len() + additions.len() >= MAX_ENTRIES
            {
                return Err(RenderError::Budget("renderer resource table"));
            }
            additions.insert(upload.token, Blob::new(Arc::new(upload.bytes)));
        }
        let lookup = |token: &u64| {
            self.entries
                .get(token)
                .or_else(|| additions.get(token))
                .cloned()
                .ok_or(RenderError::UnresolvedResource(
                    "unknown renderer resource token",
                ))
        };
        for (i, (font, token)) in scene.fonts.iter_mut().zip(packet.fonts).enumerate() {
            font.data = match token {
                Some(token) => lookup(&token)?,
                None if i == 0 && font.index == 0 => Scene::new(1, 1).fonts[0].data.clone(),
                None => {
                    return Err(RenderError::UnresolvedResource(
                        "invalid reserved font slot",
                    ));
                }
            };
        }
        let mut keys = BTreeSet::new();
        for (key, token) in packet.images {
            if !keys.insert(key) {
                return Err(RenderError::Snapshot("duplicate image key".into()));
            }
            scene
                .image_sources
                .get_mut(&key)
                .ok_or(RenderError::UnresolvedResource("unknown image key"))?
                .data = lookup(&token)?;
        }
        let snapshot = SceneSnapshot::capture_owned(scene, self.limits)?;
        self.entries.extend(additions);
        self.bytes = bytes;
        self.sequence = packet.sequence;
        Ok(snapshot)
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn entries(&self) -> usize {
        self.entries.len()
    }
}
