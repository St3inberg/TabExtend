//! The TabExtend server: accepts a paired tablet and streams to it.
//!
//! Order of checks on every connection, before any screen content is sent:
//! 1. TLS 1.3 handshake with a client certificate (proves key possession).
//! 2. `Hello` within the handshake timeout, with a supported version.
//! 3. Either a valid pairing token (then the certificate is remembered), or a
//!    certificate fingerprint already in the paired-devices list.
//! 4. No other tablet is streaming.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tabextend_core::encoder::EncoderSpec;
use tabextend_core::pairing::{Fingerprint, PairingWindow};
use tabextend_core::protocol::{
    Codec, HEADER_LEN, MAX_CONTROL_MESSAGE, Message, PROTOCOL_VERSION, RejectReason, check_length,
};
use tabextend_core::stream::{FrameDecision, FrameGate, default_bitrate, stream_size};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

use crate::capture::CaptureBackend;
use crate::identity::{HostIdentity, PairedStore, fingerprint};
use crate::pipeline::{EncodePipeline, EncodedFrame, StreamConfig};
use crate::tls;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Encoded frames waiting to be sent. Small on purpose: anything queued is latency.
const FRAME_QUEUE: usize = 3;
const PING_INTERVAL: Duration = Duration::from_secs(1);
const STATS_INTERVAL: Duration = Duration::from_secs(5);
/// Keyframes are for recovery; with a reliable transport they are rarely needed.
const KEYFRAME_SECONDS: u32 = 5;
const MAX_FPS: u32 = 60;

pub struct ServerOptions {
    pub encoder: EncoderSpec,
    pub max_width: Option<u32>,
}

pub struct Server {
    tls: TlsAcceptor,
    paired: PairedStore,
    pairing: Mutex<Option<PairingWindow>>,
    capture: Arc<dyn CaptureBackend>,
    options: ServerOptions,
    busy: AtomicBool,
}

impl Server {
    pub fn new(
        identity: &HostIdentity,
        paired: PairedStore,
        capture: Arc<dyn CaptureBackend>,
        options: ServerOptions,
    ) -> Result<Self, rustls::Error> {
        Ok(Self {
            tls: TlsAcceptor::from(Arc::new(tls::server_config(identity)?)),
            paired,
            pairing: Mutex::new(None),
            capture,
            options,
            busy: AtomicBool::new(false),
        })
    }

    /// Accept one pairing with this window's token.
    pub fn open_pairing(&self, window: PairingWindow) {
        *self.pairing.lock().expect("pairing lock") = Some(window);
    }

    pub async fn run(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            let (tcp, peer) = listener.accept().await?;
            let server = self.clone();
            tokio::spawn(async move {
                if let Err(e) = server.handle(tcp, peer).await {
                    eprintln!("{peer}: {e}");
                }
            });
        }
    }

    async fn handle(&self, tcp: TcpStream, peer: SocketAddr) -> io::Result<()> {
        tcp.set_nodelay(true)?;
        let stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.tls.accept(tcp))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))??;
        let client = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|cert| fingerprint(cert))
            .ok_or_else(|| io::Error::other("no client certificate"))?;

        let (mut reader, mut writer) = tokio::io::split(stream);
        let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_message(&mut reader))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no hello"))??;
        let Message::Hello {
            version,
            pairing_token,
            device_name,
            display,
        } = hello
        else {
            return Err(io::Error::other("first message was not hello"));
        };

        if version != PROTOCOL_VERSION {
            return reject(
                &mut writer,
                RejectReason::UnsupportedVersion,
                &format!("host speaks version {PROTOCOL_VERSION}, tablet {version}"),
            )
            .await;
        }
        if let Some(token) = pairing_token {
            if let Err(why) = self.redeem(&token, client, &device_name) {
                return reject(&mut writer, RejectReason::BadPairingToken, &why).await;
            }
            eprintln!("Paired with {device_name} ({peer}), fingerprint {client}");
        } else if !self.paired.load()?.contains(&client) {
            eprintln!("{peer}: refused unpaired device {client}");
            return reject(
                &mut writer,
                RejectReason::NotPaired,
                "this tablet is not paired; run `tabextend pair` on the computer",
            )
            .await;
        }

        let Some(_busy) = BusyGuard::take(&self.busy) else {
            return reject(
                &mut writer,
                RejectReason::Busy,
                "another tablet is connected",
            )
            .await;
        };
        let Some((width, height)) = stream_size(&display, self.options.max_width) else {
            return reject(
                &mut writer,
                RejectReason::CaptureFailed,
                "unsupported display size",
            )
            .await;
        };
        let fps = (display.refresh_millihz / 1000).clamp(1, MAX_FPS);

        let capture = match self.capture.start(width, height).await {
            Ok(capture) => capture,
            Err(why) => return reject(&mut writer, RejectReason::CaptureFailed, &why).await,
        };
        eprintln!(
            "Streaming to {device_name} ({peer}): {width}×{height} via {}",
            self.options.encoder.element
        );
        let result = stream_to(
            &capture.source(),
            &self.options.encoder,
            StreamConfig {
                bitrate_bps: default_bitrate(width, height, fps),
                keyframe_interval: fps * KEYFRAME_SECONDS,
            },
            (width, height),
            reader,
            &mut writer,
        )
        .await;
        capture.stop().await;
        eprintln!("{device_name} ({peer}) disconnected");
        result
    }

    fn redeem(
        &self,
        token: &[u8; 16],
        client: Fingerprint,
        device_name: &str,
    ) -> Result<(), String> {
        let mut pairing = self.pairing.lock().expect("pairing lock");
        let window = pairing
            .as_mut()
            .ok_or("no pairing in progress; run `tabextend pair` on the computer")?;
        window
            .redeem(token, Instant::now())
            .map_err(|e| e.to_string())?;
        self.paired
            .add(client, device_name)
            .map_err(|e| format!("could not save pairing: {e}"))
    }
}

/// Holds the one streaming slot; released on drop.
struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    fn take(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn stream_to<R, W>(
    source: &str,
    encoder: &EncoderSpec,
    config: StreamConfig,
    (width, height): (u32, u32),
    reader: R,
    writer: &mut W,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let (frame_tx, mut frames) = mpsc::channel::<EncodedFrame>(FRAME_QUEUE);
    let (keyframe_tx, mut keyframe_requests) = mpsc::unbounded_channel::<()>();
    let (error_tx, mut errors) = mpsc::unbounded_channel::<String>();
    let gate = Arc::new(Mutex::new(FrameGate::new()));
    let stats = Arc::new(Mutex::new(Stats::default()));

    let pipeline = {
        let gate = gate.clone();
        let stats = stats.clone();
        EncodePipeline::start(
            source,
            encoder,
            &config,
            move |frame| {
                let room = frame_tx.capacity() > 0;
                let decision = gate.lock().expect("gate").decide(frame.keyframe, room);
                match decision {
                    FrameDecision::Send => {
                        if frame_tx.try_send(frame).is_err() {
                            gate.lock().expect("gate").reset();
                        }
                    }
                    FrameDecision::Drop => stats.lock().expect("stats").dropped += 1,
                    FrameDecision::DropAndRequestKeyframe => {
                        stats.lock().expect("stats").dropped += 1;
                        let _ = keyframe_tx.send(());
                    }
                }
            },
            move |error| {
                let _ = error_tx.send(error);
            },
        )
        .map_err(io::Error::other)?
    };

    write(
        writer,
        &Message::Welcome {
            version: PROTOCOL_VERSION,
            codec: Codec::H264,
            width,
            height,
        },
    )
    .await?;

    let (incoming_tx, mut incoming) = mpsc::channel::<io::Result<Message>>(8);
    let reader_task = tokio::spawn(read_loop(reader, incoming_tx));
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut report = tokio::time::interval(STATS_INTERVAL);
    report.tick().await;
    let mut ping_id = 0u32;

    let result = loop {
        tokio::select! {
            Some(frame) = frames.recv() => {
                let len = frame.data.len();
                let message = Message::Video { pts_us: frame.pts_us, keyframe: frame.keyframe, data: frame.data };
                if let Err(e) = write(writer, &message).await {
                    break Err(e);
                }
                let mut s = stats.lock().expect("stats");
                s.frames += 1;
                s.bytes += len as u64;
            }
            Some(()) = keyframe_requests.recv() => pipeline.request_keyframe(),
            Some(error) = errors.recv() => {
                let _ = write(writer, &Message::Reject {
                    reason: RejectReason::CaptureFailed,
                    detail: error.clone(),
                }).await;
                break Err(io::Error::other(format!("capture failed: {error}")));
            }
            message = incoming.recv() => match message {
                Some(Ok(Message::Ping { id, sent_us })) => {
                    if let Err(e) = write(writer, &Message::Pong { id, sent_us }).await {
                        break Err(e);
                    }
                }
                Some(Ok(Message::Pong { sent_us, .. })) => {
                    stats.lock().expect("stats").rtt_us = now_us().saturating_sub(sent_us);
                }
                Some(Ok(Message::RequestKeyframe)) => {
                    gate.lock().expect("gate").reset();
                    pipeline.request_keyframe();
                }
                Some(Ok(other)) => {
                    break Err(io::Error::other(format!("unexpected message from tablet: {other:?}")));
                }
                Some(Err(e)) => break Err(e),
                None => break Ok(()),
            },
            _ = ping.tick() => {
                ping_id = ping_id.wrapping_add(1);
                if let Err(e) = write(writer, &Message::Ping { id: ping_id, sent_us: now_us() }).await {
                    break Err(e);
                }
            }
            _ = report.tick() => {
                let mut s = stats.lock().expect("stats");
                eprintln!("{}", s.summary(STATS_INTERVAL));
                *s = Stats { rtt_us: s.rtt_us, ..Stats::default() };
            }
        }
    };
    reader_task.abort();
    drop(pipeline);
    match result {
        Err(e) if is_disconnect(&e) => Ok(()),
        other => other,
    }
}

#[derive(Default)]
struct Stats {
    frames: u64,
    dropped: u64,
    bytes: u64,
    rtt_us: u64,
}

impl Stats {
    fn summary(&self, over: Duration) -> String {
        let secs = over.as_secs_f64();
        format!(
            "  {:.0} fps · {:.1} Mbit/s · round trip {:.1} ms · dropped {}",
            self.frames as f64 / secs,
            self.bytes as f64 * 8.0 / secs / 1e6,
            self.rtt_us as f64 / 1000.0,
            self.dropped
        )
    }
}

fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
    )
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Microseconds on a monotonic clock. Only ever compared with itself.
fn now_us() -> u64 {
    START.elapsed().as_micros() as u64
}

async fn read_loop<R: AsyncRead + Unpin>(mut reader: R, out: mpsc::Sender<io::Result<Message>>) {
    loop {
        let message = read_message(&mut reader).await;
        let failed = message.is_err();
        if out.send(message).await.is_err() || failed {
            return;
        }
    }
}

/// Read one message from the tablet. Video is refused outright, so the
/// largest allocation a tablet can cause is one control message.
pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Message> {
    let mut head = [0u8; HEADER_LEN + 1];
    reader.read_exact(&mut head).await?;
    let length = u32::from_be_bytes(head[..HEADER_LEN].try_into().expect("4 bytes"));
    let tag = head[HEADER_LEN];
    let length = check_length(length, tag).map_err(invalid)?;
    if length > MAX_CONTROL_MESSAGE {
        return Err(invalid("tablet sent an oversized message"));
    }
    let mut body = vec![0u8; length];
    body[0] = tag;
    reader.read_exact(&mut body[1..]).await?;
    Message::decode(&body).map_err(invalid)
}

async fn write<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> io::Result<()> {
    writer.write_all(&message.encode()).await?;
    writer.flush().await
}

async fn reject<W: AsyncWrite + Unpin>(
    writer: &mut W,
    reason: RejectReason,
    detail: &str,
) -> io::Result<()> {
    write(
        writer,
        &Message::Reject {
            reason,
            detail: detail.to_owned(),
        },
    )
    .await?;
    writer.shutdown().await
}

fn invalid(e: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}
