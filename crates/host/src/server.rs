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
use tabextend_core::input::InputMapper;
use tabextend_core::pairing::{
    Fingerprint, PairingWindow, commitment_matches, confirmation_code, format_code,
};
use tabextend_core::protocol::{
    Codec, HEADER_LEN, MAX_CONTROL_MESSAGE, Message, NONCE_LEN, PROTOCOL_VERSION, PairingRequest,
    RejectReason, check_length,
};
use tabextend_core::stream::{FrameDecision, FrameGate, default_bitrate, stream_size};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

use crate::approve::Approver;
use crate::capture::{Capture, CaptureBackend};
use crate::identity::{HostIdentity, PairedStore, fingerprint};
use crate::pipeline::{EncodePipeline, EncodedFrame, StreamConfig, accepts_gpu_buffers};
use crate::tls;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the tablet has to reveal its nonce after the challenge.
const REVEAL_TIMEOUT: Duration = Duration::from_secs(30);
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
    host: Fingerprint,
    paired: PairedStore,
    pairing: Mutex<Option<PairingWindow>>,
    approver: Arc<dyn Approver>,
    /// One confirmation at a time: one question on the user's screen.
    confirming: AtomicBool,
    capture: Arc<dyn CaptureBackend>,
    options: ServerOptions,
    busy: AtomicBool,
}

impl Server {
    pub fn new(
        identity: &HostIdentity,
        paired: PairedStore,
        approver: Arc<dyn Approver>,
        capture: Arc<dyn CaptureBackend>,
        options: ServerOptions,
    ) -> Result<Self, rustls::Error> {
        Ok(Self {
            tls: TlsAcceptor::from(Arc::new(tls::server_config(identity)?)),
            host: identity.fingerprint,
            paired,
            pairing: Mutex::new(None),
            approver,
            confirming: AtomicBool::new(false),
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
            pairing,
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
        match pairing {
            PairingRequest::Token(token) => {
                if let Err(why) = self.redeem(&token, client, &device_name) {
                    return reject(&mut writer, RejectReason::BadPairingToken, &why).await;
                }
                eprintln!("Paired with {device_name} ({peer}), fingerprint {client}");
            }
            PairingRequest::Confirm { commitment } => {
                let outcome = self
                    .confirm(&mut reader, &mut writer, commitment, client, &device_name)
                    .await?;
                if let Err((reason, why)) = outcome {
                    eprintln!("{peer}: pairing with {device_name} failed: {why}");
                    return reject(&mut writer, reason, &why).await;
                }
                eprintln!("Paired with {device_name} ({peer}), fingerprint {client}");
            }
            PairingRequest::None if !self.paired.load()?.contains(&client) => {
                eprintln!("{peer}: refused unpaired device {client}");
                return reject(
                    &mut writer,
                    RejectReason::NotPaired,
                    "this tablet is not paired; run `tabextend pair` on the computer",
                )
                .await;
            }
            PairingRequest::None => {}
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
            capture.as_ref(),
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

    /// Pairing by code comparison. The tablet committed to a secret nonce in
    /// its hello; the host answers with its own nonce, the tablet reveals
    /// its nonce, and both derive the code from the two certificates and
    /// the two nonces. The user approves here after comparing codes.
    async fn confirm<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
        commitment: [u8; NONCE_LEN],
        client: Fingerprint,
        device_name: &str,
    ) -> io::Result<Result<(), (RejectReason, String)>>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let open = self
            .pairing
            .lock()
            .expect("pairing lock")
            .as_ref()
            .map(|w| w.check_open(Instant::now()));
        match open {
            None => {
                return Ok(Err((
                    RejectReason::PairingDenied,
                    "pairing is not open; run `tabextend pair` on the computer".into(),
                )));
            }
            Some(Err(why)) => return Ok(Err((RejectReason::PairingDenied, why.to_string()))),
            Some(Ok(())) => {}
        }
        let Some(_slot) = BusyGuard::take(&self.confirming) else {
            return Ok(Err((
                RejectReason::Busy,
                "another tablet is pairing; try again in a moment".into(),
            )));
        };

        let mut host_nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut host_nonce).map_err(io::Error::other)?;
        write(writer, &Message::PairChallenge { nonce: host_nonce }).await?;
        let revealed = match tokio::time::timeout(REVEAL_TIMEOUT, read_message(reader)).await {
            Ok(Ok(Message::PairReveal { nonce })) => Some(nonce),
            Ok(Ok(_)) | Err(_) => None,
            Ok(Err(e)) => return Err(e),
        };
        let valid = revealed.filter(|nonce| commitment_matches(&commitment, nonce));
        let approved = match valid {
            Some(nonce) => {
                let code = format_code(confirmation_code(&self.host, &client, &host_nonce, &nonce));
                self.approver.approve(device_name, &code).await
            }
            None => false,
        };
        if let Some(window) = self.pairing.lock().expect("pairing lock").as_mut() {
            window.finish_confirmation(approved);
        }
        if !approved {
            let why = if valid.is_some() {
                "declined on the computer"
            } else {
                "the pairing exchange failed"
            };
            return Ok(Err((RejectReason::PairingDenied, why.into())));
        }
        self.paired.add(client, device_name)?;
        Ok(Ok(()))
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
    capture: &dyn Capture,
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

    let start = |gpu_buffers: bool| {
        let gate = gate.clone();
        let stats = stats.clone();
        let frame_tx = frame_tx.clone();
        let keyframe_tx = keyframe_tx.clone();
        let error_tx = error_tx.clone();
        EncodePipeline::start(
            &capture.source(gpu_buffers),
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
        .map_err(io::Error::other)
    };
    let (input_width, input_height) = capture.input_size();
    let mut input = InputMapper::new(input_width, input_height);
    let mut input_failed = false;
    let mut gpu_buffers = accepts_gpu_buffers(encoder);
    let mut pipeline = Some(start(gpu_buffers)?);
    let mut captured_before = 0;
    let mut sent_total = 0u64;

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
                sent_total += 1;
            }
            Some(()) = keyframe_requests.recv() => {
                if let Some(p) = &pipeline { p.request_keyframe() }
            }
            Some(error) = errors.recv() => {
                // GPU buffers did not negotiate on this machine: retry once
                // with copied frames before giving up.
                if gpu_buffers && sent_total == 0 {
                    eprintln!("  GPU buffers not available ({error}); copying frames instead");
                    gpu_buffers = false;
                    drop(pipeline.take());
                    captured_before = 0;
                    match start(false) {
                        Ok(p) => { pipeline = Some(p); continue; }
                        Err(e) => break Err(e),
                    }
                }
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
                Some(Ok(Message::Input(event))) => {
                    for call in input.map(event) {
                        match capture.inject(call).await {
                            Ok(()) => {}
                            // Some desktops grant pointer but not touch. Detect
                            // it from the first refusal rather than trusting
                            // what the portal reports, and replay this event
                            // as pointer input so the touch is not lost.
                            Err(e) if call.is_touch() && !input.is_emulating_touch() => {
                                eprintln!("  touch not allowed by the desktop ({e}, for {call:?}); fingers now drive the pointer");
                                input.emulate_touch_with_pointer();
                                for call in input.map(event) {
                                    let _ = capture.inject(call).await;
                                }
                                break;
                            }
                            Err(e) => {
                                if !std::mem::replace(&mut input_failed, true) {
                                    eprintln!("  input not delivered: {e} ({call:?})");
                                }
                            }
                        }
                    }
                }
                Some(Ok(Message::RequestKeyframe)) => {
                    gate.lock().expect("gate").reset();
                    if let Some(p) = &pipeline { p.request_keyframe() }
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
                let captured = pipeline.as_ref().map_or(0, EncodePipeline::captured_frames);
                let mut s = stats.lock().expect("stats");
                s.captured = captured - captured_before;
                captured_before = captured;
                eprintln!("{}{}", s.summary(STATS_INTERVAL), if gpu_buffers { " · zero-copy" } else { "" });
                *s = Stats { rtt_us: s.rtt_us, ..Stats::default() };
            }
        }
    };
    reader_task.abort();
    // Nothing may stay pressed on the laptop after the tablet is gone.
    for call in input.release_all() {
        let _ = capture.inject(call).await;
    }
    drop(pipeline);
    match result {
        Err(e) if is_disconnect(&e) => Ok(()),
        other => other,
    }
}

#[derive(Default)]
struct Stats {
    captured: u64,
    frames: u64,
    dropped: u64,
    bytes: u64,
    rtt_us: u64,
}

impl Stats {
    fn summary(&self, over: Duration) -> String {
        let secs = over.as_secs_f64();
        format!(
            "  captured {:.0} fps · sent {:.0} fps · {:.1} Mbit/s · round trip {:.1} ms · dropped {}",
            self.captured as f64 / secs,
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
