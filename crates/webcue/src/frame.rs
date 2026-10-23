// SPDX-License-Identifier: MPL-2.0
use crate::{Error, Result, Scope, Surface};
pub use grafting_frame::{FrameSize, OwnedNativeFrame, PixelFormat};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Straight,
    Premultiplied,
    Opaque,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Srgb,
    LinearSrgb,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStamp {
    pub scope: Scope,
    pub allocation_epoch: u64,
    pub content_revision: u64,
}
#[derive(Debug)]
pub struct NativeFrame {
    pub stamp: FrameStamp,
    pub frame: OwnedNativeFrame,
    pub alpha: Alpha,
    pub encoding: Encoding,
}
#[derive(Clone, Debug)]
pub struct CpuFrame {
    pub stamp: FrameStamp,
    pub size: FrameSize,
    pub stride: u32,
    pub rgba: Vec<u8>,
    pub alpha: Alpha,
    pub encoding: Encoding,
}
impl CpuFrame {
    pub fn validate(&self) -> Result<()> {
        let row = self
            .size
            .width
            .checked_mul(4)
            .ok_or_else(|| Error::Invalid("frame width overflow".into()))?;
        let bytes = (self.stride as usize)
            .checked_mul(self.size.height as usize)
            .ok_or_else(|| Error::Invalid("frame length overflow".into()))?;
        if self.size.width == 0
            || self.size.height == 0
            || self.stride < row
            || !self.stride.is_multiple_of(4)
            || self.rgba.len() != bytes
        {
            return Err(Error::Invalid(
                "invalid RGBA dimensions, stride or length".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub enum Frame {
    Native(NativeFrame),
    Cpu(CpuFrame),
    Overlay,
}
/// Only concrete non-blocking supplier paths implement this trait. It has no
/// fallback to a blocking acquire method. Native descriptors remain move-owned.
pub trait FrameSource: Surface {
    fn try_acquire_frame(&mut self) -> Result<Option<Frame>>;
}
