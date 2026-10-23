//! What the desktop's XDG portals can do.
//!
//! TabExtend needs two portals: ScreenCast, to create and capture a virtual
//! monitor, and RemoteDesktop, to inject touch input into it. Both publish
//! their capabilities as D-Bus properties; this module turns those raw values
//! into answers. Reading them off the bus is the host's job.
//!
//! Spec: <https://flatpak.github.io/xdg-desktop-portal/docs/>

/// ScreenCast `AvailableSourceTypes` bits.
pub mod source_type {
    pub const MONITOR: u32 = 1;
    pub const WINDOW: u32 = 2;
    pub const VIRTUAL: u32 = 4;
}

/// RemoteDesktop `AvailableDeviceTypes` bits.
pub mod device_type {
    pub const KEYBOARD: u32 = 1;
    pub const POINTER: u32 = 2;
    pub const TOUCHSCREEN: u32 = 4;
}

/// First ScreenCast version with `persist_mode` and restore tokens.
pub const SCREENCAST_PERSIST_VERSION: u32 = 4;
/// First RemoteDesktop version with `persist_mode` and restore tokens.
pub const REMOTE_DESKTOP_PERSIST_VERSION: u32 = 2;

/// The ScreenCast portal's properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCastCaps {
    pub version: u32,
    pub source_types: u32,
}

impl ScreenCastCaps {
    /// Can it create a virtual monitor? Without this, TabExtend can only mirror.
    pub fn supports_virtual_monitor(&self) -> bool {
        self.source_types & source_type::VIRTUAL != 0
    }

    /// Can it remember the user's permission between sessions?
    pub fn supports_restore_tokens(&self) -> bool {
        self.version >= SCREENCAST_PERSIST_VERSION
    }
}

/// The RemoteDesktop portal's properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteDesktopCaps {
    pub version: u32,
    pub device_types: u32,
}

impl RemoteDesktopCaps {
    pub fn supports_touchscreen(&self) -> bool {
        self.device_types & device_type::TOUCHSCREEN != 0
    }

    pub fn supports_pointer(&self) -> bool {
        self.device_types & device_type::POINTER != 0
    }

    pub fn supports_restore_tokens(&self) -> bool {
        self.version >= REMOTE_DESKTOP_PERSIST_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_monitor_needs_the_virtual_bit() {
        let mirror_only = ScreenCastCaps {
            version: 5,
            source_types: source_type::MONITOR | source_type::WINDOW,
        };
        assert!(!mirror_only.supports_virtual_monitor());

        let with_virtual = ScreenCastCaps {
            version: 5,
            source_types: source_type::MONITOR | source_type::VIRTUAL,
        };
        assert!(with_virtual.supports_virtual_monitor());
    }

    #[test]
    fn screencast_restore_tokens_start_at_version_4() {
        let caps = |version| ScreenCastCaps {
            version,
            source_types: 0,
        };
        assert!(!caps(3).supports_restore_tokens());
        assert!(caps(4).supports_restore_tokens());
    }

    #[test]
    fn touch_and_pointer_are_separate_bits() {
        let pointer_only = RemoteDesktopCaps {
            version: 2,
            device_types: device_type::KEYBOARD | device_type::POINTER,
        };
        assert!(pointer_only.supports_pointer());
        assert!(!pointer_only.supports_touchscreen());
    }

    #[test]
    fn remote_desktop_restore_tokens_start_at_version_2() {
        let caps = |version| RemoteDesktopCaps {
            version,
            device_types: 0,
        };
        assert!(!caps(1).supports_restore_tokens());
        assert!(caps(2).supports_restore_tokens());
    }
}
