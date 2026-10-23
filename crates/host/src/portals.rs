//! Reading portal and service facts off D-Bus.

use tabextend_core::portal::{RemoteDesktopCaps, ScreenCastCaps};
use zbus::{Connection, Proxy};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

async fn portal_u32s(interface: &str, property: &str) -> zbus::Result<(u32, u32)> {
    let connection = Connection::session().await?;
    let proxy = Proxy::new(&connection, PORTAL_BUS, PORTAL_PATH, interface).await?;
    let version = proxy.get_property::<u32>("version").await?;
    let value = proxy.get_property::<u32>(property).await?;
    Ok((version, value))
}

pub fn screencast_caps() -> Result<ScreenCastCaps, String> {
    zbus::block_on(portal_u32s(
        "org.freedesktop.portal.ScreenCast",
        "AvailableSourceTypes",
    ))
    .map(|(version, source_types)| ScreenCastCaps {
        version,
        source_types,
    })
    .map_err(|e| e.to_string())
}

pub fn remote_desktop_caps() -> Result<RemoteDesktopCaps, String> {
    zbus::block_on(portal_u32s(
        "org.freedesktop.portal.RemoteDesktop",
        "AvailableDeviceTypes",
    ))
    .map(|(version, device_types)| RemoteDesktopCaps {
        version,
        device_types,
    })
    .map_err(|e| e.to_string())
}

/// Whether the Avahi mDNS daemon is on the system bus.
pub fn avahi_running() -> Result<bool, String> {
    zbus::block_on(async {
        let connection = Connection::system().await?;
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        dbus.name_has_owner("org.freedesktop.Avahi".try_into()?)
            .await
            .map_err(zbus::Error::from)
    })
    .map_err(|e: zbus::Error| e.to_string())
}
