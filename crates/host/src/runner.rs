//! Starting and controlling the host: shared by the command line and the
//! desktop app.

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Instant;

use tabextend_core::encoder::{EncoderSpec, default_h264_encoders, select_encoder};
use tabextend_core::pairing::{Fingerprint, PairingLink, PairingWindow};
use tabextend_core::protocol::PAIRING_TOKEN_LEN;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::approve::Approver;
use crate::capture::{CaptureBackend, PortalVirtualMonitor, TestPattern};
use crate::events::{Events, ServerEvent};
use crate::identity::{HostIdentity, PairedStore, default_data_dir};
use crate::mdns::{self, Advertisement, Announcement};
use crate::probe::GstProbe;
use crate::server::{Server, ServerOptions};

pub const DEFAULT_PORT: u16 = 47420;

#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    /// GStreamer encoder element; None picks the best that works.
    pub encoder: Option<String>,
    /// Address to listen on; None uses this machine's LAN address.
    pub listen: Option<IpAddr>,
    pub port: Option<u16>,
    /// Scale streams down to at most this width.
    pub max_width: Option<u32>,
    /// Stream a test pattern instead of a virtual monitor.
    pub test_pattern: bool,
}

/// The address other devices on the LAN reach this machine at: the source
/// address of the default route. Connecting a UDP socket sends nothing.
pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Pick the encoder: the user's choice, or the best that works here.
/// Test-encodes, so it takes a moment; call it off the UI thread.
pub fn choose_encoder(preferred: Option<&str>) -> Result<EncoderSpec, String> {
    let probe = GstProbe::new(1280, 800).map_err(|e| e.to_string())?;
    select_encoder(&probe, &default_h264_encoders(), preferred)
        .map_err(|e| format!("{e}; run `tabextend doctor` for details"))
}

/// A running host: listening, announced on the network, ready for tablets.
pub struct RunningHost {
    server: Arc<Server>,
    pub address: SocketAddr,
    pub fingerprint: Fingerprint,
    pub encoder: EncoderSpec,
    accept: tokio::task::JoinHandle<io::Result<()>>,
    _announcement: Option<Advertisement>,
    pairing_announcement: Mutex<Option<Advertisement>>,
}

impl RunningHost {
    /// Start listening. `approver` answers pairing-by-code requests.
    pub async fn start(
        config: &HostConfig,
        approver: Arc<dyn Approver>,
        events: Events,
    ) -> Result<Self, String> {
        let data_dir = default_data_dir().map_err(|e| e.to_string())?;
        let identity = HostIdentity::load_or_create(&data_dir).map_err(|e| e.to_string())?;
        let paired = PairedStore::new(&data_dir).map_err(|e| e.to_string())?;

        let preferred = config.encoder.clone();
        let encoder = tokio::task::spawn_blocking(move || choose_encoder(preferred.as_deref()))
            .await
            .map_err(|e| e.to_string())??;

        let ip = config
            .listen
            .or_else(lan_address)
            .ok_or("cannot find this machine's network address; connect to a network or set the listen address")?;
        let address = SocketAddr::new(ip, config.port.unwrap_or(DEFAULT_PORT));

        let capture: Arc<dyn CaptureBackend> = if config.test_pattern {
            Arc::new(TestPattern::default())
        } else {
            Arc::new(PortalVirtualMonitor::new(data_dir.join("state")))
        };
        let server = Server::new(
            &identity,
            paired,
            approver,
            capture,
            ServerOptions {
                encoder: encoder.clone(),
                max_width: config.max_width,
            },
        )
        .map_err(|e| e.to_string())?
        .with_events(events.clone());
        let server = Arc::new(server);

        let listener = TcpListener::bind(address)
            .await
            .map_err(|e| format!("cannot listen on {address}: {e}"))?;
        let accept = tokio::spawn(server.clone().run(listener));

        // Always announce a short id, so paired tablets can find this host
        // again; the name is only announced while pairing.
        let announcement = mdns::advertise(&Announcement {
            port: address.port(),
            id: mdns::host_id(&identity.fingerprint),
            pairing_name: None,
        })
        .await
        .map_err(|e| {
            events(ServerEvent::Note(format!(
                "not announced on the network ({e}); tablets cannot find this computer by scanning"
            )))
        })
        .ok();

        Ok(Self {
            server,
            address,
            fingerprint: identity.fingerprint,
            encoder,
            accept,
            _announcement: announcement,
            pairing_announcement: Mutex::new(None),
        })
    }

    /// Open a 2-minute pairing window: a fresh QR code link, and the
    /// computer's name announced so tablets can list it.
    pub async fn open_pairing(&self) -> Result<(PairingLink, String), String> {
        let mut token = [0u8; PAIRING_TOKEN_LEN];
        getrandom::fill(&mut token).map_err(|e| e.to_string())?;
        self.server
            .open_pairing(PairingWindow::open(token, Instant::now()));
        let name = mdns::computer_name().await;
        let advertisement = mdns::advertise(&Announcement {
            port: self.address.port(),
            id: mdns::host_id(&self.fingerprint),
            pairing_name: Some(name.clone()),
        })
        .await
        .ok();
        *self.pairing_announcement.lock().await = advertisement;
        let link = PairingLink {
            host: match self.address.ip() {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => format!("[{v6}]"),
            },
            port: self.address.port(),
            fingerprint: self.fingerprint,
            token,
        };
        Ok((link, name))
    }

    /// End pairing early: the token stops working and the name is withdrawn.
    pub async fn close_pairing(&self) {
        self.server.close_pairing();
        *self.pairing_announcement.lock().await = None;
    }

    /// Run until the listener fails.
    pub async fn wait(&mut self) -> io::Result<()> {
        (&mut self.accept).await.map_err(io::Error::other)?
    }
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.accept.abort();
    }
}
