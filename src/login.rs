use anyhow::{Result, bail};
use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

pub fn status() -> SMAppServiceStatus {
    // SAFETY: ServiceManagement is available on the minimum supported macOS 13.
    unsafe { SMAppService::mainAppService().status() }
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    if !NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
    {
        bail!(
            "Launch at login requires the tinyRDP.app bundle. Run scripts/bundle.sh and open dist/tinyRDP.app first."
        );
    }
    // SAFETY: These calls configure this application's own login item.
    let result = unsafe {
        let service = SMAppService::mainAppService();
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|error| {
        anyhow::anyhow!(
            "Could not update launch at login: {}",
            error.localizedDescription()
        )
    })
}

pub fn open_settings() {
    // SAFETY: This opens Apple's Login Items settings on macOS 13+.
    unsafe { SMAppService::openSystemSettingsLoginItems() }
}
