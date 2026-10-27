//! Announcing the host on the local network, so tablets can find it.
//!
//! Uses the Avahi daemon over D-Bus. What is announced depends on the mode:
//! while pairing, the computer's name (so the user can pick it from a list);
//! otherwise only a short id derived from the host certificate, so a paired
//! tablet can find the host again when its address changes without telling
//! strangers whose computer it is. See docs/security.md.

use tabextend_core::pairing::Fingerprint;
use zbus::Connection;
use zbus::Proxy;
use zbus::zvariant::OwnedObjectPath;

pub const SERVICE_TYPE: &str = "_tabextend._tcp";

const AVAHI: &str = "org.freedesktop.Avahi";
const IF_UNSPEC: i32 = -1;
const PROTO_INET: i32 = 0;

/// The short id announced for a host: the first 8 bytes of its fingerprint.
pub fn host_id(fingerprint: &Fingerprint) -> String {
    fingerprint.to_hex()[..16].to_owned()
}

/// What to announce. TXT keys are read by the tablet app.
pub struct Announcement {
    pub port: u16,
    pub id: String,
    /// The computer's name; only while pairing.
    pub pairing_name: Option<String>,
}

impl Announcement {
    fn instance_name(&self) -> String {
        match &self.pairing_name {
            Some(name) => format!("TabExtend on {name}"),
            None => format!("TabExtend {}", self.id),
        }
    }

    fn txt(&self) -> Vec<Vec<u8>> {
        let mut txt = vec![b"v=1".to_vec(), format!("id={}", self.id).into_bytes()];
        if self.pairing_name.is_some() {
            txt.push(b"pair=1".to_vec());
        }
        txt
    }
}

/// A live announcement. Avahi withdraws it when this is dropped, because
/// it frees entry groups whose D-Bus client disconnects.
pub struct Advertisement {
    _connection: Connection,
    _group: OwnedObjectPath,
}

pub async fn advertise(announcement: &Announcement) -> zbus::Result<Advertisement> {
    let connection = Connection::system().await?;
    let server = Proxy::new(&connection, AVAHI, "/", "org.freedesktop.Avahi.Server").await?;
    let path: OwnedObjectPath = server.call("EntryGroupNew", &()).await?;
    let group = Proxy::new(
        &connection,
        AVAHI,
        path.clone(),
        "org.freedesktop.Avahi.EntryGroup",
    )
    .await?;
    let txt = announcement.txt();
    let base = announcement.instance_name();
    // Another host on the network may already use the name; add a number.
    let mut last_error = None;
    for attempt in 1..=5 {
        let name = if attempt == 1 {
            base.clone()
        } else {
            format!("{base} ({attempt})")
        };
        let added: zbus::Result<()> = group
            .call(
                "AddService",
                &(
                    IF_UNSPEC,
                    PROTO_INET,
                    0u32,
                    name.as_str(),
                    SERVICE_TYPE,
                    "",
                    "",
                    announcement.port,
                    &txt,
                ),
            )
            .await;
        match added {
            Ok(()) => {
                group.call::<_, _, ()>("Commit", &()).await?;
                return Ok(Advertisement {
                    _connection: connection,
                    _group: path,
                });
            }
            Err(e) => {
                last_error = Some(e);
                group.call::<_, _, ()>("Reset", &()).await?;
            }
        }
    }
    Err(last_error.expect("tried at least once"))
}

/// The name people know this computer by: the "pretty" hostname from
/// Settings → About if set, else the plain hostname.
pub async fn computer_name() -> String {
    async fn read(property: &str) -> zbus::Result<String> {
        let connection = Connection::system().await?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.hostname1",
            "/org/freedesktop/hostname1",
            "org.freedesktop.hostname1",
        )
        .await?;
        proxy.get_property(property).await
    }
    for property in ["PrettyHostname", "Hostname"] {
        if let Ok(name) = read(property).await
            && !name.trim().is_empty()
        {
            return name.trim().to_owned();
        }
    }
    "this computer".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announcement(pairing_name: Option<&str>) -> Announcement {
        Announcement {
            port: 47420,
            id: host_id(&Fingerprint([0xab; 32])),
            pairing_name: pairing_name.map(str::to_owned),
        }
    }

    #[test]
    fn id_is_a_short_fingerprint_prefix() {
        assert_eq!(host_id(&Fingerprint([0xab; 32])), "abababababababab");
    }

    #[test]
    fn name_is_only_announced_while_pairing() {
        let pairing = announcement(Some("stenberg-laptop"));
        assert_eq!(pairing.instance_name(), "TabExtend on stenberg-laptop");
        assert!(pairing.txt().contains(&b"pair=1".to_vec()));

        let serving = announcement(None);
        assert_eq!(serving.instance_name(), "TabExtend abababababababab");
        assert!(!serving.txt().iter().any(|t| t.starts_with(b"pair")));
        assert!(serving.txt().contains(&b"id=abababababababab".to_vec()));
    }
}
