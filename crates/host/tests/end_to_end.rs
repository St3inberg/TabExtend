//! A real server (test pattern, real encoder, real TLS) against a test
//! client that behaves like the tablet app.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tabextend_core::encoder::{default_h264_encoders, select_encoder};
use tabextend_core::input::InputCall;
use tabextend_core::pairing::{Fingerprint, PairingWindow};
use tabextend_core::protocol::{
    DisplayInfo, HEADER_LEN, Message, PAIRING_TOKEN_LEN, PROTOCOL_VERSION, PairingRequest,
    RejectReason, check_length,
};
use tabextend_host::capture::TestPattern;
use tabextend_host::identity::{HostIdentity, PairedStore, fingerprint};
use tabextend_host::probe::GstProbe;
use tabextend_host::server::{Server, ServerOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

const TOKEN: [u8; PAIRING_TOKEN_LEN] = [0x42; PAIRING_TOKEN_LEN];
const TIMEOUT: Duration = Duration::from_secs(20);

struct TestHost {
    address: SocketAddr,
    fingerprint: Fingerprint,
    injected: Arc<Mutex<Vec<InputCall>>>,
    _dir: tempfile::TempDir,
}

async fn start_host() -> TestHost {
    start_host_with(TestPattern::default()).await
}

async fn start_host_with(capture: TestPattern) -> TestHost {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let paired = PairedStore::new(dir.path()).unwrap();
    let probe = GstProbe::new(320, 240).unwrap();
    let encoder = select_encoder(&probe, &default_h264_encoders(), None)
        .expect("tests need a working H.264 encoder (e.g. x264enc or openh264enc)");
    let injected = capture.injected();
    let server = Server::new(
        &identity,
        paired,
        Arc::new(capture),
        ServerOptions {
            encoder,
            max_width: None,
        },
    )
    .unwrap();
    server.open_pairing(PairingWindow::open(TOKEN, Instant::now()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(Arc::new(server).run(listener));
    TestHost {
        address,
        fingerprint: identity.fingerprint,
        injected,
        _dir: dir,
    }
}

/// A tablet: its own key pair and certificate.
struct Tablet {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Tablet {
    fn new() -> Self {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["tablet".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        Self {
            cert: cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()),
        }
    }

    async fn connect(&self, host: &TestHost, pin: Fingerprint) -> std::io::Result<Client> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedHost { pin, provider }))
            .with_client_auth_cert(
                vec![self.cert.clone()],
                PrivateKeyDer::Pkcs8(self.key.clone_key()),
            )
            .unwrap();
        let tcp = TcpStream::connect(host.address).await?;
        let stream = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("tabextend").unwrap(), tcp)
            .await?;
        Ok(Client { stream })
    }
}

/// Pins the host certificate by fingerprint, as the Android app does.
#[derive(Debug)]
struct PinnedHost {
    pin: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity) == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "host fingerprint does not match".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

struct Client {
    stream: TlsStream<TcpStream>,
}

impl Client {
    async fn send(&mut self, message: &Message) {
        self.stream.write_all(&message.encode()).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    async fn hello(&mut self, token: Option<[u8; PAIRING_TOKEN_LEN]>) {
        self.send(&Message::Hello {
            version: PROTOCOL_VERSION,
            pairing: token.map_or(PairingRequest::None, PairingRequest::Token),
            device_name: "test tablet".into(),
            display: DisplayInfo {
                width: 640,
                height: 400,
                density_dpi: 160,
                refresh_millihz: 60_000,
            },
        })
        .await;
    }

    /// Next message, or None when the host closed the connection.
    async fn receive(&mut self) -> Option<Message> {
        let read = async {
            let mut head = [0u8; HEADER_LEN + 1];
            self.stream.read_exact(&mut head).await.ok()?;
            let length = u32::from_be_bytes(head[..HEADER_LEN].try_into().unwrap());
            let length = check_length(length, head[HEADER_LEN]).ok()?;
            let mut body = vec![0u8; length];
            body[0] = head[HEADER_LEN];
            self.stream.read_exact(&mut body[1..]).await.ok()?;
            Some(Message::decode(&body).expect("host sent a malformed message"))
        };
        tokio::time::timeout(TIMEOUT, read)
            .await
            .expect("host went quiet")
    }

    /// Skip pings until a message of interest arrives.
    async fn receive_skipping_pings(&mut self) -> Option<Message> {
        loop {
            match self.receive().await? {
                Message::Ping { .. } => continue,
                other => return Some(other),
            }
        }
    }
}

fn has_sps(annex_b: &[u8]) -> bool {
    annex_b
        .windows(4)
        .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 7)
}

async fn expect_stream(client: &mut Client) {
    match client.receive_skipping_pings().await {
        Some(Message::Welcome { width, height, .. }) => assert_eq!((width, height), (640, 400)),
        other => panic!("expected Welcome, got {other:?}"),
    }
    match client.receive_skipping_pings().await {
        Some(Message::Video { keyframe, data, .. }) => {
            assert!(keyframe, "the first frame must be a keyframe");
            assert!(has_sps(&data), "the first keyframe must carry SPS");
        }
        other => panic!("expected video, got {other:?}"),
    }
}

async fn expect_reject(client: &mut Client, reason: RejectReason) {
    match client.receive_skipping_pings().await {
        Some(Message::Reject { reason: got, .. }) => assert_eq!(got, reason),
        other => panic!("expected Reject({reason:?}), got {other:?}"),
    }
    assert_eq!(
        client.receive().await,
        None,
        "host must close after rejecting"
    );
}

#[tokio::test]
async fn pairs_streams_and_reconnects_without_a_token() {
    let host = start_host().await;
    let tablet = Tablet::new();

    let mut client = tablet.connect(&host, host.fingerprint).await.unwrap();
    client.hello(Some(TOKEN)).await;
    expect_stream(&mut client).await;
    drop(client);

    // Paired now: the certificate alone is enough. Retry briefly while the
    // host notices the first connection closed and frees the stream slot.
    for attempt in 0.. {
        let mut client = tablet.connect(&host, host.fingerprint).await.unwrap();
        client.hello(None).await;
        match client.receive_skipping_pings().await {
            Some(Message::Reject {
                reason: RejectReason::Busy,
                ..
            }) if attempt < 20 => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Some(Message::Welcome { .. }) => break,
            other => panic!("expected Welcome on reconnect, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn unpaired_tablet_gets_nothing() {
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(None).await;
    expect_reject(&mut client, RejectReason::NotPaired).await;
}

#[tokio::test]
async fn wrong_token_is_refused() {
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(Some([0x43; PAIRING_TOKEN_LEN])).await;
    expect_reject(&mut client, RejectReason::BadPairingToken).await;
}

#[tokio::test]
async fn token_works_only_once() {
    let host = start_host().await;
    let mut first = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    first.hello(Some(TOKEN)).await;
    expect_stream(&mut first).await;

    let mut second = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    second.hello(Some(TOKEN)).await;
    expect_reject(&mut second, RejectReason::BadPairingToken).await;
}

#[tokio::test]
async fn wrong_host_pin_fails_the_handshake() {
    let host = start_host().await;
    let result = Tablet::new().connect(&host, Fingerprint([0; 32])).await;
    assert!(
        result.is_err(),
        "a tablet must not talk to an unpinned host"
    );
}

#[tokio::test]
async fn second_tablet_is_told_the_host_is_busy() {
    let host = start_host().await;
    let tablet = Tablet::new();
    let mut first = tablet.connect(&host, host.fingerprint).await.unwrap();
    first.hello(Some(TOKEN)).await;
    expect_stream(&mut first).await;

    let mut second = tablet.connect(&host, host.fingerprint).await.unwrap();
    second.hello(None).await;
    expect_reject(&mut second, RejectReason::Busy).await;
}

#[tokio::test]
async fn ping_is_answered_while_streaming() {
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(Some(TOKEN)).await;
    expect_stream(&mut client).await;
    client.send(&Message::Ping { id: 77, sent_us: 5 }).await;
    loop {
        match client.receive().await {
            Some(Message::Pong { id: 77, sent_us: 5 }) => break,
            Some(Message::Video { .. } | Message::Ping { .. }) => continue,
            other => panic!("expected Pong, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn oversized_message_closes_the_connection_and_host_survives() {
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    // A Hello claiming to be 1 GiB long.
    client.stream.write_all(&[0x40, 0, 0, 0, 1]).await.unwrap();
    client.stream.flush().await.unwrap();
    assert_eq!(client.receive().await, None);

    // The host still serves others.
    let mut next = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    next.hello(Some(TOKEN)).await;
    expect_stream(&mut next).await;
}

/// Wait until the host has delivered at least `count` input calls.
async fn injected(host: &TestHost, count: usize) -> Vec<InputCall> {
    for _ in 0..200 {
        let calls = host.injected.lock().unwrap().clone();
        if calls.len() >= count {
            return calls;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "expected {count} input calls, got {:?}",
        host.injected.lock().unwrap()
    );
}

#[tokio::test]
async fn touch_reaches_the_virtual_monitor_clipped_to_it() {
    use tabextend_core::protocol::{InputEvent, TouchPhase};
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(Some(TOKEN)).await;
    expect_stream(&mut client).await;

    let touch = |phase, x, y| {
        Message::Input(InputEvent::Touch {
            slot: 0,
            phase,
            x,
            y,
        })
    };
    client.send(&touch(TouchPhase::Down, 0.5, 0.5)).await;
    // Past the edge, and garbage: clipped, and dropped.
    client.send(&touch(TouchPhase::Move, 2.0, -1.0)).await;
    client
        .send(&Message::Input(InputEvent::PointerMove {
            x: f32::NAN,
            y: 0.5,
        }))
        .await;
    client.send(&touch(TouchPhase::Up, 0.0, 0.0)).await;

    let calls = injected(&host, 3).await;
    assert_eq!(
        calls,
        [
            InputCall::TouchDown {
                slot: 0,
                x: 320.0,
                y: 200.0
            },
            InputCall::TouchMotion {
                slot: 0,
                x: 639.0,
                y: 0.0
            },
            InputCall::TouchUp { slot: 0 },
        ]
    );
}

#[tokio::test]
async fn disconnecting_mid_drag_releases_everything() {
    use tabextend_core::protocol::{InputEvent, MouseButton};
    let host = start_host().await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(Some(TOKEN)).await;
    expect_stream(&mut client).await;

    client
        .send(&Message::Input(InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        }))
        .await;
    client
        .send(&Message::Input(InputEvent::Key {
            code: 42,
            pressed: true,
        }))
        .await;
    injected(&host, 2).await;
    drop(client);

    let calls = injected(&host, 4).await;
    assert_eq!(
        &calls[2..],
        [
            InputCall::PointerButton {
                code: 0x110,
                pressed: false
            },
            InputCall::Key {
                code: 42,
                pressed: false
            },
        ]
    );
}

#[tokio::test]
async fn refused_touch_falls_back_to_driving_the_pointer() {
    use tabextend_core::protocol::{InputEvent, TouchPhase};
    let host = start_host_with(TestPattern::refusing_touch()).await;
    let mut client = Tablet::new()
        .connect(&host, host.fingerprint)
        .await
        .unwrap();
    client.hello(Some(TOKEN)).await;
    expect_stream(&mut client).await;

    let touch = |phase, x, y| {
        Message::Input(InputEvent::Touch {
            slot: 0,
            phase,
            x,
            y,
        })
    };
    client.send(&touch(TouchPhase::Down, 0.5, 0.5)).await;
    client.send(&touch(TouchPhase::Move, 0.25, 0.5)).await;
    client.send(&touch(TouchPhase::Up, 0.25, 0.5)).await;

    // The first, refused touch is replayed as a press, not lost.
    let calls = injected(&host, 4).await;
    assert_eq!(
        calls,
        [
            InputCall::PointerMotion { x: 320.0, y: 200.0 },
            InputCall::PointerButton {
                code: 0x110,
                pressed: true
            },
            InputCall::PointerMotion { x: 160.0, y: 200.0 },
            InputCall::PointerButton {
                code: 0x110,
                pressed: false
            },
        ]
    );
}
