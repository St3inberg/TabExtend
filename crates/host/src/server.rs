//! The TabExtend server: accepts a paired tablet and streams to it.
//!
//! Order of checks on every connection, before any screen content is sent:
//! 1. TLS 1.3 handshake with a client certificate (proves key possession).
//! 2. `Hello` within the handshake timeout, with a supported version.
//! 3. Either a valid pairing token (then the certificate is remembered), or a
//!    certificate fingerprint already in the paired-devices list.
//! 4. No other tablet is streaming.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tabextend_core::encoder::EncoderSpec;
use tabextend_core::input::{ContentArea, InputMapper};
use tabextend_core::pairing::{
    Fingerprint, PairingWindow, clean_device_name, commitment_matches, confirmation_code,
    format_code,
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
use crate::audio::AudioPipeline;
use crate::capture::{Capture, CaptureBackend};
use crate::events::{Events, ServerEvent, StreamStats, print_events};
use crate::identity::{HostIdentity, PairedStore, fingerprint};
use crate::pipeline::{EncodePipeline, EncodedFrame, StreamConfig, accepts_gpu_buffers};
use crate::tls;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the tablet has to reveal its nonce after the challenge.
const REVEAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Encoded frames waiting to be sent. Small on purpose: anything queued is latency.
const FRAME_QUEUE: usize = 3;
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Audio chunks waiting to be sent (each about 10–20 ms).
const AUDIO_QUEUE: usize = 16;
const STATS_INTERVAL: Duration = Duration::from_secs(5);
/// Keyframes are for recovery; with a reliable transport they are rarely needed.
const KEYFRAME_SECONDS: u32 = 5;
const MAX_FPS: u32 = 60;

pub struct ServerOptions {
    pub encoder: EncoderSpec,
    pub max_width: Option<u32>,
    /// How long the tablet's monitor outlives a dropped connection. A
    /// tablet that comes back in time gets the same monitor, so windows
    /// stay where they were and GNOME sees no monitor unplugged and
    /// plugged in again. Zero removes it at once.
    pub keep_monitor: Duration,
}

/// Connections that have not yet shown they belong to a paired tablet (TLS
/// handshake, hello, pairing). Anyone on the network can open these, so
/// they are capped: per address, so one machine flooding the port cannot
/// lock a tablet out, and in total, so many machines cannot exhaust file
/// descriptors. Authenticated sessions do not count.
const MAX_PENDING_PER_ADDRESS: usize = 8;
const MAX_PENDING: usize = 64;
/// After a failed accept (e.g. out of file descriptors), wait this long and
/// keep serving instead of giving up.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Pending {
    per_address: HashMap<IpAddr, usize>,
    total: usize,
}

/// One unauthenticated connection's place under the caps; freed on drop.
struct PendingSlot {
    pending: Arc<Mutex<Pending>>,
    address: IpAddr,
}

impl PendingSlot {
    fn take(pending: &Arc<Mutex<Pending>>, address: IpAddr) -> Option<Self> {
        let mut p = pending.lock().expect("pending");
        let mine = p.per_address.get(&address).copied().unwrap_or(0);
        if p.total >= MAX_PENDING || mine >= MAX_PENDING_PER_ADDRESS {
            return None;
        }
        p.total += 1;
        *p.per_address.entry(address).or_default() += 1;
        Some(Self {
            pending: pending.clone(),
            address,
        })
    }
}

impl Drop for PendingSlot {
    fn drop(&mut self) {
        let mut p = self.pending.lock().expect("pending");
        p.total -= 1;
        if let Some(count) = p.per_address.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                p.per_address.remove(&self.address);
            }
        }
    }
}

/// How long the monitor waits for its tablet by default.
pub const KEEP_MONITOR: Duration = Duration::from_secs(10);

/// A monitor whose tablet just disconnected, waiting for it to return.
struct Parked {
    id: u64,
    capture: Box<dyn Capture>,
    tablet: Fingerprint,
    size: (u32, u32),
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
    events: Events,
    parked: Arc<Mutex<Option<Parked>>>,
    parked_seq: AtomicU64,
    pending: Arc<Mutex<Pending>>,
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
            events: print_events(),
            parked: Arc::new(Mutex::new(None)),
            parked_seq: AtomicU64::new(0),
            pending: Arc::default(),
        })
    }

    /// Report events here instead of printing them.
    pub fn with_events(mut self, events: Events) -> Self {
        self.events = events;
        self
    }

    fn emit(&self, event: ServerEvent) {
        (self.events)(event);
    }

    /// The monitor kept for this tablet, if it is waiting at this size.
    /// Any other parked monitor is removed: only one tablet streams at once.
    async fn unpark(&self, tablet: Fingerprint, size: (u32, u32)) -> Option<Box<dyn Capture>> {
        let parked = self.parked.lock().expect("parked").take()?;
        if parked.tablet == tablet && parked.size == size {
            return Some(parked.capture);
        }
        parked.capture.stop().await;
        None
    }

    /// Keep a monitor for [`ServerOptions::keep_monitor`], then remove it
    /// unless its tablet came back.
    fn park(&self, capture: Box<dyn Capture>, tablet: Fingerprint, size: (u32, u32)) {
        let wait = self.options.keep_monitor;
        if wait.is_zero() {
            tokio::spawn(capture.stop());
            return;
        }
        let id = self.parked_seq.fetch_add(1, Ordering::Relaxed);
        let replaced = self.parked.lock().expect("parked").replace(Parked {
            id,
            capture,
            tablet,
            size,
        });
        if let Some(old) = replaced {
            tokio::spawn(old.capture.stop());
        }
        let parked = self.parked.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let expired = {
                let mut slot = parked.lock().expect("parked");
                match &*slot {
                    Some(p) if p.id == id => slot.take(),
                    _ => None,
                }
            };
            if let Some(p) = expired {
                p.capture.stop().await;
            }
        });
    }

    /// Accept one pairing with this window's token.
    pub fn open_pairing(&self, window: PairingWindow) {
        *self.pairing.lock().expect("pairing lock") = Some(window);
    }

    /// Stop accepting pairings.
    pub fn close_pairing(&self) {
        *self.pairing.lock().expect("pairing lock") = None;
    }

    pub async fn run(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            let (tcp, peer) = match listener.accept().await {
                Ok(connection) => connection,
                Err(e) => {
                    // Usually out of file descriptors. Returning here would
                    // stop the host for good; wait and keep serving.
                    eprintln!("accept failed: {e}");
                    tokio::time::sleep(ACCEPT_RETRY).await;
                    continue;
                }
            };
            // Over the cap: close at once, without an event per connection
            // (a flood would bury the window in notices).
            let Some(slot) = PendingSlot::take(&self.pending, peer.ip().to_canonical()) else {
                drop(tcp);
                continue;
            };
            let server = self.clone();
            tokio::spawn(async move {
                if let Err(e) = server.handle(tcp, peer, slot).await {
                    server.emit(ServerEvent::Refused {
                        peer: peer.to_string(),
                        reason: e.to_string(),
                    });
                }
            });
        }
    }

    async fn handle(&self, tcp: TcpStream, peer: SocketAddr, slot: PendingSlot) -> io::Result<()> {
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
        // Chosen by whoever connected, not yet trusted: clean it once, here,
        // for every place it is shown or stored.
        let device_name = clean_device_name(&device_name);

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
                self.emit(ServerEvent::Paired {
                    name: device_name.clone(),
                    fingerprint: client,
                });
            }
            PairingRequest::Confirm { commitment } => {
                // Anyone can send this; tell the user only while they are
                // pairing. Otherwise any machine on the network could keep
                // putting messages (with a name it chose) on the screen.
                let user_is_pairing = self.pairing.lock().expect("pairing lock").is_some();
                let outcome = self
                    .confirm(&mut reader, &mut writer, commitment, client, &device_name)
                    .await?;
                if let Err((reason, why)) = outcome {
                    if !user_is_pairing {
                        return reject(&mut writer, reason, &why).await;
                    }
                    self.emit(ServerEvent::Refused {
                        peer: peer.to_string(),
                        reason: format!("pairing with {device_name} failed: {why}"),
                    });
                    return reject(&mut writer, reason, &why).await;
                }
                self.emit(ServerEvent::Paired {
                    name: device_name.clone(),
                    fingerprint: client,
                });
            }
            PairingRequest::None if !self.paired.load()?.contains(&client) => {
                self.emit(ServerEvent::Refused {
                    peer: peer.to_string(),
                    reason: format!("refused unpaired device {client}"),
                });
                return reject(
                    &mut writer,
                    RejectReason::NotPaired,
                    "this tablet is not paired; run `tabextend pair` on the computer",
                )
                .await;
            }
            PairingRequest::None => {}
        }
        // A paired tablet from here on: it no longer counts against the
        // caps on unauthenticated connections.
        drop(slot);

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

        let capture = match self.unpark(client, (width, height)).await {
            Some(capture) => capture,
            None => match self.capture.start(width, height).await {
                Ok(capture) => capture,
                Err(why) => return reject(&mut writer, RejectReason::CaptureFailed, &why).await,
            },
        };
        self.emit(ServerEvent::Streaming {
            name: device_name.clone(),
            width,
            height,
            encoder: self.options.encoder.element.clone(),
        });
        let result = stream_to(
            Stream {
                events: &self.events,
                capture: capture.as_ref(),
                device_name: &device_name,
                encoder: &self.options.encoder,
                config: StreamConfig {
                    bitrate_bps: default_bitrate(width, height, fps),
                    keyframe_interval: fps * KEYFRAME_SECONDS,
                },
                size: (width, height),
            },
            reader,
            &mut writer,
        )
        .await;
        // Only a lost connection is worth waiting out; a capture that
        // failed is not reused.
        let reusable = match &result {
            Ok(()) => true,
            Err(e) => is_disconnect(e),
        };
        if reusable {
            self.park(capture, client, (width, height));
        } else {
            capture.stop().await;
        }
        self.emit(ServerEvent::Disconnected {
            name: device_name.clone(),
        });
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

/// The sound pipeline of one session, which starts in the background.
enum AudioSlot {
    Starting,
    /// Held only to keep the pipeline running until the session ends.
    Running {
        _pipeline: AudioPipeline,
    },
    Stopped,
}

/// What one streaming session needs.
struct Stream<'a> {
    events: &'a Events,
    capture: &'a dyn Capture,
    device_name: &'a str,
    encoder: &'a EncoderSpec,
    config: StreamConfig,
    size: (u32, u32),
}

async fn stream_to<R, W>(stream: Stream<'_>, reader: R, writer: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let Stream {
        events,
        capture,
        device_name,
        encoder,
        config,
        size: (width, height),
    } = stream;
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
            |pipeline| capture.attach(pipeline),
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
    // The desktop may have made its screen another shape than the stream
    // (KDE always uses 1920×1080); the picture then has bars around it.
    let mut input =
        InputMapper::new(input_width, input_height).with_content_area(ContentArea::letterbox(
            (f64::from(width), f64::from(height)),
            (input_width, input_height),
        ));
    let mut input_failed = false;
    let mut gpu_buffers = accepts_gpu_buffers(encoder);
    let mut pipeline = Some(start(gpu_buffers)?);
    let mut captured_before = 0;
    let mut sent_total = 0u64;

    // Sound: optional. Stale audio is worse than a gap, so a full queue drops.
    // Starting a PipeWire source can block until its stream is linked, so it
    // starts on its own thread: the picture never waits for the sound.
    let (audio_tx, mut audio) = mpsc::channel::<(u64, Vec<u8>)>(AUDIO_QUEUE);
    let audio_pipeline: Arc<Mutex<AudioSlot>> = Arc::new(Mutex::new(AudioSlot::Starting));
    if let Some(source) = capture.audio_source(device_name) {
        let slot = audio_pipeline.clone();
        let events = events.clone();
        std::thread::spawn(move || {
            let started = AudioPipeline::start(&source, move |pts, pcm| {
                let _ = audio_tx.try_send((pts, pcm));
            });
            let mut slot = slot.lock().expect("audio slot");
            match (started, &*slot) {
                // The tablet left while the sound was starting.
                (Ok(_pipeline), AudioSlot::Stopped) => {}
                (Ok(pipeline), _) => {
                    *slot = AudioSlot::Running {
                        _pipeline: pipeline,
                    }
                }
                (Err(e), _) => events(ServerEvent::Note(format!("no sound for the tablet: {e}"))),
            }
        });
    }

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
            Some((pts_us, pcm)) = audio.recv() => {
                if let Err(e) = write(writer, &Message::Audio { pts_us, pcm }).await {
                    break Err(e);
                }
            }
            Some(()) = keyframe_requests.recv() => {
                if let Some(p) = &pipeline { p.request_keyframe() }
            }
            Some(error) = errors.recv() => {
                // GPU buffers did not negotiate on this machine: retry once
                // with copied frames before giving up.
                if gpu_buffers && sent_total == 0 {
                    events(ServerEvent::Note(format!("GPU buffers not available ({error}); copying frames instead")));
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
                                events(ServerEvent::Note(format!("touch not allowed by the desktop ({e}); fingers now drive the pointer")));
                                input.emulate_touch_with_pointer();
                                for call in input.map(event) {
                                    let _ = capture.inject(call).await;
                                }
                                break;
                            }
                            Err(e) => {
                                if !std::mem::replace(&mut input_failed, true) {
                                    events(ServerEvent::Note(format!("input not delivered: {e}")));
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
                events(ServerEvent::Stats(s.snapshot(STATS_INTERVAL, gpu_buffers)));
                *s = Stats { rtt_us: s.rtt_us, ..Stats::default() };
            }
        }
    };
    reader_task.abort();
    *audio_pipeline.lock().expect("audio slot") = AudioSlot::Stopped;
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
    fn snapshot(&self, over: Duration, zero_copy: bool) -> StreamStats {
        let secs = over.as_secs_f64();
        StreamStats {
            captured_fps: self.captured as f64 / secs,
            sent_fps: self.frames as f64 / secs,
            mbps: self.bytes as f64 * 8.0 / secs / 1e6,
            round_trip_ms: self.rtt_us as f64 / 1000.0,
            dropped: self.dropped,
            zero_copy,
        }
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
