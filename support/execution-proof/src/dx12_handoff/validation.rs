// SPDX-License-Identifier: MPL-2.0
//! Explicit debug-layer admission and error-queue collection in BOTH processes.
use super::*;
use windows::{Win32::Graphics::Direct3D12::*, core::Interface};
pub(super) fn enable() -> io::Result<()> {
    static ENABLED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    match ENABLED.get_or_init(|| {
        let mut debug: Option<ID3D12Debug> = None;
        unsafe { D3D12GetDebugInterface(&mut debug) }.map_err(|e| e.to_string())?;
        unsafe {
            debug.ok_or("D3D12 debug layer missing")?.EnableDebugLayer();
        }
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(bad(error)),
    }
}

pub(super) fn check(host: &grafting::HostWgpuContext) -> io::Result<()> {
    let hal = unsafe { host.device.as_hal::<wgpu::hal::api::Dx12>() }
        .ok_or_else(|| bad("DX12 device required"))?;
    let info: ID3D12InfoQueue = hal.raw_device().cast().map_err(bad)?;
    let mut errors = Vec::new();
    unsafe {
        let count = info.GetNumStoredMessagesAllowedByRetrievalFilter();
        for i in 0..count {
            let mut bytes = 0;
            info.GetMessage(i, None, &mut bytes).map_err(bad)?;
            // Aligned buffer for the header and trailing message description.
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let message = storage.as_mut_ptr().cast::<D3D12_MESSAGE>();
            info.GetMessage(i, Some(message), &mut bytes).map_err(bad)?;
            let message = &*message;
            let text = if message.pDescription.is_null() {
                std::borrow::Cow::Borrowed("<no description>")
            } else {
                std::ffi::CStr::from_ptr(message.pDescription.cast()).to_string_lossy()
            };
            let baseline_warning = message.Severity == D3D12_MESSAGE_SEVERITY_WARNING
                && matches!(
                    message.ID,
                    D3D12_MESSAGE_ID_CREATE_SAMPLER_COMPARISON_FUNC_IGNORED
                        | D3D12_MESSAGE_ID_CLEARRENDERTARGETVIEW_MISMATCHINGCLEARVALUE
                );
            if message.Severity == D3D12_MESSAGE_SEVERITY_ERROR
                || message.Severity == D3D12_MESSAGE_SEVERITY_CORRUPTION
                || (message.Severity == D3D12_MESSAGE_SEVERITY_WARNING && !baseline_warning)
            {
                errors.push(format!(
                    "D3D12 validation error id={:?}: {text}",
                    message.ID
                ));
            } else {
                eprintln!(
                    "D3D12 baseline diagnostic {:?} id={:?}: {text}",
                    message.Severity, message.ID
                );
            }
        }
        info.ClearStoredMessages();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(bad(errors.join("\n")))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires hardware DX12 and the installed D3D12 debug layer"]
    fn debug_queue_detects_injected_error() {
        let (_, _, native) = gpu().unwrap();
        let hal = unsafe { native.device.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
        let info: ID3D12InfoQueue = hal.raw_device().cast().unwrap();
        unsafe {
            info.AddApplicationMessage(
                D3D12_MESSAGE_SEVERITY_ERROR,
                windows::core::s!("cue diagnostic negative control"),
            )
        }
        .unwrap();
        drop(hal);
        let error = check(&native).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cue diagnostic negative control")
        );
        let hal = unsafe { native.device.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
        let info: ID3D12InfoQueue = hal.raw_device().cast().unwrap();
        unsafe {
            info.AddApplicationMessage(
                D3D12_MESSAGE_SEVERITY_WARNING,
                windows::core::s!("cue unexpected warning negative control"),
            )
        }
        .unwrap();
        drop(hal);
        assert!(
            check(&native)
                .unwrap_err()
                .to_string()
                .contains("cue unexpected warning negative control")
        );
        check(&native).unwrap();
        eprintln!("D3D12 error collector negative control passed");
    }
}
