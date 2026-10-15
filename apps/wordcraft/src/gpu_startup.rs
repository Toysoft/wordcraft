//! macOS: record wgpu errors raised while eframe builds its renderer, instead of letting wgpu's
//! fatal default handler panic. Some Metal compilers reject the shader language version wgpu
//! picks from the OS version (patched macOS on unsupported Macs); egui's pipeline then fails
//! during startup. The wgpu-hal patch in `vendor/wgpu-hal` picks a version the compiler accepts;
//! this turns whatever still goes wrong into a message.

use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui_wgpu::{WgpuSetup, WgpuSetupExisting};

/// Errors wgpu reported on the device created by [`precreate_device`] before the app existed.
pub type StartupErrors = Arc<Mutex<Vec<String>>>;

/// Create the wgpu device ourselves so startup errors are recorded. Returns `None` (setup
/// untouched, eframe reports its own error) when the adapter or device can't be created.
pub fn precreate_device(setup: &mut WgpuSetup) -> Option<StartupErrors> {
    let WgpuSetup::CreateNew(create) = &*setup else {
        return None;
    };
    let create = create.clone();
    let instance = pollster::block_on(setup.new_instance());
    let adapter = match &create.native_adapter_selector {
        Some(select) => {
            let adapters = pollster::block_on(instance.enumerate_adapters(create.instance_descriptor.backends));
            select(&adapters, None).ok()?
        }
        None => pollster::block_on(
            instance.request_adapter(&eframe::wgpu::RequestAdapterOptions { power_preference: create.power_preference, ..Default::default() }),
        )
        .ok()?,
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&(create.device_descriptor)(&adapter))).ok()?;
    let errors = StartupErrors::default();
    let sink = errors.clone();
    device.on_uncaptured_error(Arc::new(move |e: eframe::wgpu::Error| {
        let text = e.to_string();
        log::error!("wgpu error during startup: {text}");
        sink.lock().unwrap_or_else(PoisonError::into_inner).push(text);
    }));
    *setup = WgpuSetup::Existing(WgpuSetupExisting { instance, adapter, device, queue });
    Some(errors)
}

/// The message to stop startup with when `errors` holds a wgpu error, `None` when it is clean.
pub fn startup_failure(errors: &StartupErrors) -> Option<String> {
    let guard = errors.lock().unwrap_or_else(PoisonError::into_inner);
    let first = guard.first()?;
    let detail =
        first.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("note:") && *l != "Caused by:").collect::<Vec<_>>().join(" ");
    Some(format!("The graphics driver rejected WordCraft's window shaders: {detail}. Update macOS or try WGPU_METAL_MAX_MSL=3.1."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_startup_wgpu_error_becomes_a_message_not_a_panic() {
        let errors = StartupErrors::default();
        assert_eq!(startup_failure(&errors), None, "a clean start has no failure");
        let text = "Validation Error\n\nCaused by:\n  In Device::create_render_pipeline, label = 'egui_pipeline'\n    Internal error in ShaderStages(VERTEX) shader: Metal: error: invalid value 'metal3.2' in '-std=metal3.2'\nnote: use 'metal3.1' for 'Metal 3.1' standard\n";
        errors.lock().unwrap().push(text.into());
        let message = startup_failure(&errors).expect("a failure");
        assert!(message.contains("egui_pipeline"), "{message}");
        assert!(message.contains("metal3.2"), "{message}");
        assert!(!message.contains("note:"), "{message}");
    }
}
