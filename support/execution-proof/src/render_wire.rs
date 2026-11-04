// SPDX-License-Identifier: MPL-2.0
//! Fixture codec shared by the CPU and Metal transports. Not a public wire ABI.
use serde::{Deserialize, Serialize};
use std::io;
use webcue::execution::ExecutionId;
use webcue::*;
use wgpu_cue_netrender::{render_session::RenderRequest, scene_job::*};
const SCHEMA: &str = "netrender-66b2f72e-cpu-v1";
#[derive(Serialize, Deserialize)]
pub(crate) struct WireJob {
    schema: String,
    owner: u128,
    generation: u64,
    allocation: u64,
    revision: u64,
    scene: String,
}
impl WireJob {
    pub(crate) fn from_job(job: &RenderJob) -> io::Result<Self> {
        Ok(Self {
            schema: SCHEMA.into(),
            owner: job.stamp.scope.owner,
            generation: job.stamp.scope.generation,
            allocation: job.stamp.allocation_epoch,
            revision: job.stamp.content_revision,
            scene: String::from_utf8(job.snapshot.bytes().to_vec()).map_err(io::Error::other)?,
        })
    }
    pub(crate) fn job(self) -> io::Result<RenderJob> {
        if self.schema != SCHEMA {
            return Err(io::Error::other("raster schema mismatch"));
        }
        Ok(RenderJob {
            stamp: FrameStamp {
                scope: Scope {
                    owner: self.owner,
                    generation: self.generation,
                },
                allocation_epoch: self.allocation,
                content_revision: self.revision,
            },
            snapshot: SceneSnapshot::from_bytes(self.scene.into_bytes(), RenderLimits::default())
                .map_err(io::Error::other)?,
        })
    }
}

use wgpu_cue_netrender::resource_table::{ResourcePacket, ResourceReceiver, ResourceSender};
#[derive(Serialize, Deserialize)]
pub(crate) enum WireExecutionJob {
    Legacy {
        incarnation: u64,
        job: WireJob,
    },
    Resources {
        allocation: u64,
        revision: u64,
        packet: ResourcePacket,
    },
}
impl WireExecutionJob {
    pub(crate) fn new(request: &RenderRequest) -> io::Result<Self> {
        Ok(Self::Legacy {
            incarnation: request.execution.incarnation,
            job: WireJob::from_job(&request.job)?,
        })
    }
    pub(crate) fn prepare(
        request: &RenderRequest,
        resources: &ResourceSender,
    ) -> io::Result<(Self, Option<ResourceSender>)> {
        if !request.job.snapshot.is_typed() {
            return Ok((Self::new(request)?, None));
        }
        let (packet, next) = resources
            .prepare(&request.job.snapshot)
            .map_err(io::Error::other)?;
        if packet.namespace.execution() != request.execution {
            return Err(io::Error::other("resource sender incarnation mismatch"));
        }
        Ok((
            Self::Resources {
                allocation: request.job.stamp.allocation_epoch,
                revision: request.job.stamp.content_revision,
                packet,
            },
            Some(next),
        ))
    }
    pub(crate) fn request(self) -> io::Result<RenderRequest> {
        match self {
            Self::Legacy { incarnation, job } => {
                let job = job.job()?;
                Ok(RenderRequest {
                    execution: ExecutionId {
                        scope: job.stamp.scope,
                        incarnation,
                    },
                    job,
                })
            }
            Self::Resources { .. } => Err(io::Error::other(
                "resource packet needs an incarnation-owned table",
            )),
        }
    }
    pub(crate) fn request_with(
        self,
        resources: &mut ResourceReceiver,
    ) -> io::Result<RenderRequest> {
        match self {
            Self::Resources {
                allocation,
                revision,
                packet,
            } => {
                let execution = packet.namespace.execution();
                let snapshot = resources.receive(packet).map_err(io::Error::other)?;
                Ok(RenderRequest {
                    execution,
                    job: RenderJob {
                        stamp: FrameStamp {
                            scope: execution.scope,
                            allocation_epoch: allocation,
                            content_revision: revision,
                        },
                        snapshot,
                    },
                })
            }
            other => other.request(),
        }
    }
}
