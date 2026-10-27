# TabExtend — Product Requirements Document

Oct 22, 2026 · @stephen · revised Oct 23, 2026 after the M0 spike

## Summary

TabExtend turns an Android tablet into a true extended monitor for a Linux
laptop running GNOME on Wayland, the way spacedesk or Duet Display do on
Windows. The tablet appears in GNOME Settings → Displays as a real screen you
can drag windows onto, with touch and pen input sent back to the PC.

It works **without a cable**: Wi-Fi is the main connection, with a direct
laptop-to-tablet link for places without a usable network. USB is optional.

**Problem.** Windows users have polished "tablet as second screen" apps. On
GNOME/Wayland the popular tools either mirror instead of extend, need X11
tricks, or lack touch and pen. The tablet sits unused next to the laptop.

**Reference hardware.** TabExtend must not depend on this hardware; it is
what development and the latency targets are measured on.

| Side   | Device | Key specs |
|--------|--------|-----------|
| Host   | Fedora 44 Workstation | GNOME 50, Wayland only, Intel TigerLake UHD + NVIDIA RTX 3050 Ti |
| Client | Lenovo Idea Tab TB336FU | 11" 2560×1600, 90 Hz, MediaTek Dimensity 6300, 8 GB RAM, Android 16, Lenovo Tab Pen, 1×1 Wi-Fi 5 |

## What the M0 spike changed

Details in [`m0-spike.md`](m0-spike.md).

| Finding | Consequence for this PRD |
|---------|--------------------------|
| Wi-Fi round trip 7.5 ms, 0% loss on the reference network | Network is a small part of the 90 ms budget. TCP's stall-on-loss problem does not show up here, so M1 uses TLS over TCP and QUIC becomes a measured decision (see Transport) |
| GNOME RDP extend mode gave a black screen; the cause was GNOME and PipeWire running stale versions after an update. Retested after re-login: it works, but peaks at 74 % of a core and comes up at scale 1 | **Go.** RDP is the baseline; TabExtend focuses on what it lacks: hardware H.264, pairing, scale, direct touch, pen |
| Windows App for Android offers no H.264 to GNOME's RDP server | Even if RDP works, it costs more host CPU than a dedicated H.264 stream |
| Screen capture can silently break after system updates until the user logs out | `doctor` must capture a real frame and detect stale sessions (F14) |
| Portal virtual monitors work on GNOME 50 and take their size from the consumer | F1/F2 confirmed. No need for Mutter's private `RecordVirtual` |
| All five installed encoders work, but Intel's low-power encoder has no constant-bitrate mode on this driver | "Works" is not enough: encoders that accept every low-latency setting rank first (F12) |
| GNOME has no system tray | F7 becomes a regular app; a Quick Settings toggle is a later option |

## Existing options and why build

| Option | How it works | Gap for this use |
|--------|--------------|------------------|
| GNOME RDP extend mode | Built in since GNOME 42: set screen-share mode to extend, connect from any RDP app | Works after re-login. Android client lacks H.264 (CPU peaks 74 % of a core); scale 1; password login; generic RDP UX |
| TethrLink | Mutter ScreenCast virtual display → PipeWire → H.264 over USB tethering; GTK4 app | USB only, conflicts with wireless-first; not yet tested |
| Weylus | Browser-based client; touch and pen to PC | Wayland support is experimental; extend mode needs virtual-monitor setup |

**Why build anyway.** A focused app can own the full experience: one-tap
wireless connect, direct touch, pen pressure and palm rejection, native
packaging, and tuning for high-refresh tablet panels.

**Decision gate: go.** RDP works but misses the CPU target and the
second-screen UX, so TabExtend is built, focused on what RDP lacks.

## Goals, non-goals, target user

**Target user.** A developer or student on GNOME with an Android tablet, who
wants extra screen space for docs, chat or a terminal while travelling.

### Goals (v1)

1. Extend, not mirror: the tablet is a separate monitor in Settings → Displays.
2. No cable needed: connect in under 10 seconds from opening the tablet app,
   after a one-time pairing and screen-sharing approval.
3. Touch acts as direct pointer input on the tablet's area; pen works with pressure.
4. Smooth enough for reading and light editing (targets below).
5. Runs on any GNOME/Wayland machine: nothing hardcoded to one GPU, encoder,
   tablet model or resolution.

### Non-goals (v1)

- X11, KDE Plasma or other compositors (GNOME/Mutter only first).
- More than one tablet at a time.
- iPad or Windows clients (a browser client could add these later; see Client).
- Use over the internet; local networks and direct links only.

## Functional requirements

| ID  | Requirement | Priority |
|-----|-------------|----------|
| F1  | Host creates a virtual monitor through the portal (virtual source), so GNOME treats it as a real display | Must |
| F2  | Virtual monitor size and refresh come from the tablet's handshake, never from constants | Must |
| F3  | USB transport (USB tethering) as an optional fast path | Could |
| F4  | Wi-Fi transport; mDNS to find the host again when its address changes | Must |
| F5  | Touch on the tablet moves and clicks the pointer inside the virtual monitor only | Must |
| F6  | Pen input with pressure and tilt, exposed as a tablet device for apps like Krita and Xournal++ | Should |
| F7  | Host app (GTK4/libadwaita): pair, connect, disconnect, quality preset, live latency and FPS | Must |
| F8  | Tablet app: fullscreen immersive mode, keep screen awake, reconnect automatically | Must |
| F9  | Monitor position and scale remembered between sessions (left, right, above) | Should |
| F10 | Quality presets: Battery, Balanced, Sharp (bitrate and resolution) | Could |
| F11 | No-router mode: Wi-Fi Direct, or a laptop hotspot via NetworkManager, when the shared network isolates devices | Should |
| F12 | Encoder chosen at runtime: first one that encodes **and** accepts its low-latency settings; then ones with limits; user override | Must |
| F13 | Pairing by QR code shown on the host (address, port, certificate fingerprint, single-use token) | Must |
| F14 | `tabextend doctor` captures a real frame and detects a stale desktop session ("log out and back in") | Must |
| F15 | Sound: while connected, the tablet appears as a sound output in GNOME's settings; the user picks it to hear the laptop on the tablet | Should |

## Non-functional requirements

| Area | Target over Wi-Fi 5/6 | Target over USB (stretch) |
|------|-----------------------|---------------------------|
| Glass-to-glass latency | < 90 ms | < 50 ms |
| Frame rate | 30–60 fps while content moves | 60 fps |
| Image quality | Text readable at 100% scale, no smearing on static content | Same |
| Host CPU | < 15% of one core with a hardware encoder | Same |
| Tablet battery | ≥ 3 hours of continuous use | ≥ 4 hours |
| Laptop power | Report extra draw per encoder; prefer the lowest that meets the targets | Same |

**Security.** Binding rules are in [`security.md`](security.md). In short:
pairing before any stream, TLS 1.3 with both certificates pinned, the host
listens only on the chosen interface, input only through GNOME's
RemoteDesktop portal, untrusted input parsed with hard size limits.

**Packaging.** RPM via COPR first. Flatpak after it: it needs access to
`org.gnome.Mutter.DisplayConfig` for F9, which Flathub reviews case by case.
APK on GitHub Releases; F-Droid later, so the client avoids Google Play
services.

## Technical architecture and stack

The host asks GNOME for a virtual monitor, captures it, hardware-encodes it
and streams it; the tablet only decodes, displays and sends input back.

### Host: Rust (kept)

| Part | Choice | Why / status |
|------|--------|--------------|
| Capture + input permission | **RemoteDesktop portal session with ScreenCast sources** (ashpd) | One permission dialog covers screen and input, and input coordinates map to the stream. Changed from a ScreenCast-only session, which would need a second dialog in M2 |
| Encoding | GStreamer: `pipewiresrc` → GPU converter (`vapostproc`, `cudaconvert`) → runtime-chosen H.264 encoder → `h264parse` | Working; probed by `doctor` with the real settings |
| Monitor placement and scale | `org.gnome.Mutter.DisplayConfig` | Needed for F9 and readable text: a 2560×1600 virtual monitor at scale 1 is tiny. Private but stable; GNOME Settings uses it |
| TLS | rustls (ring provider), TLS 1.3 only, custom verifiers | Working; no C build dependencies |
| Async | tokio | Working |
| UI | gtk4-rs + libadwaita (M3) | No tray on GNOME; a Quick Settings extension is optional later |

Build dependencies stay small: GStreamer core development files only
(`gstreamer-app` is used through signals, not linked).

### Transport: TLS 1.3 over TCP, then measure

M1–M3 use one TLS connection: length-prefixed messages (see
`crates/core/src/protocol.rs`), frames dropped on the host until the next
keyframe when the send queue fills, so latency cannot pile up.

QUIC (`quinn` on the host) replaces it only if measurements under load show
stalls on real networks. Its Android side would be Rust through JNI (needs
rustup's Android targets; Fedora's Rust has none) or kwik (LGPL-3.0, pure
Java). The protocol layer is transport-independent, so the switch is local.

### Client: native Kotlin (kept), browser client as a later option

| | Native Kotlin app (v1) | Browser client (later) |
|---|---|---|
| Decode | MediaCodec with the low-latency flag (Android 11+) | WebCodecs |
| Transport | `SSLSocket`, built in | WebTransport (QUIC) |
| Pen | MotionEvent pressure and tilt | PointerEvent pressure and tilt |
| Install | APK | none; also iPads and other tablets |
| Main risk | — | WebCodecs needs a secure context, and certificate pinning via `serverCertificateHashes` requires certificates valid at most 14 days |

Client decisions:

- Pairing: the QR code is a `tabextend://pair` link. The tablet's camera or
  Lens app opens it in TabExtend, so the app needs no scanner library or
  Google Play services. If a camera app will not open custom links, an
  in-app scanner (ZXing, Apache-2.0) follows.
- Client certificate: an EC key in Android Keystore, which also issues its
  self-signed certificate; the key never leaves the device.
- `minSdk 30` (Android 11): low-latency decoding and TLS 1.3 everywhere.

### Code layout

- `tabextend-core`: protocol, pairing, stream sizing, frame dropping,
  encoder ranking, checks. Pure Rust, unit-tested anywhere.
- `tabextend-host`: GStreamer, portals, TLS server, CLI.
- `android/`: Kotlin client, with JVM unit tests for its protocol code.
- `scripts/`: repeatable spike tests.

## Milestones

Each milestone ends with something you can use.

1. **M0 — Spike.** Done: Wi-Fi latency, encoders, portal virtual monitor,
   `doctor`, RDP baseline. Go. (TethrLink not tried: USB-only.)
2. **M1 — Pixels on the tablet.** Host server (built: pairing, TLS, encode
   pipeline, frame dropping), Android app (decode, pair by link, reconnect),
   RemoteDesktop+ScreenCast session. End-to-end test with a Rust client in
   CI. Done when you can drag a window onto the tablet over Wi-Fi.
3. **M2 — Touch input.** Tablet sends touch; host injects it through the
   same portal session, clipped to the virtual monitor.
4. **M3 — Real app.** GTK4/libadwaita host app, presets and stats, monitor
   placement and scale (DisplayConfig), mDNS reconnect, `doctor` frame check.
5. **M4 — Pen, no-router mode, transport decision.** Pressure and tilt; Wi-Fi
   Direct or hotspot; QUIC only if measurements call for it.
6. **M5 — Release.** COPR, APK on GitHub, Flatpak if accepted, demo video.

## Risks and open questions

| Risk | Impact | Mitigation |
|------|--------|------------|
| Virtual monitors depend on Mutter | KDE users unsupported | Scope v1 to GNOME; `doctor` reports portal support |
| Capture breaks after system updates until re-login | Black screen, looks like a TabExtend bug | `doctor` frame check and stale-session detection (F14) |
| Permission dialog on first connect | User must be at the laptop once | Restore tokens (ScreenCast v4+, RemoteDesktop v2+) |
| Virtual monitor at scale 1 is unreadable on high-density tablets | Tiny text | Set scale from the tablet's density through DisplayConfig (F9) |
| Tablet decoder limits at full resolution | Dropped frames | Read MediaCodec capabilities in the handshake; host scales down (`--max-width`) |
| Pen pressure needs a virtual tablet device; the portal and libei have no tablet-tool support yet | Pen works only as touch | Opt-in `uinput` fallback mapped to the virtual monitor, documented as a security exception |
| Some encoders ignore bitrate settings (constant-quality only) | Bitrate spikes on detailed content | Ranked after fully working encoders (F12) |
| Wi-Fi jitter and loss on busy networks | Stutter with TCP | Frame dropping now; QUIC if measured |
| Hotel/café Wi-Fi isolates clients | Tablet not found | No-router mode (F11) |
| Camera apps that will not open `tabextend://` links | Pairing needs a manual step | In-app scanner fallback |
| Firewalls differ by distro | Connection refused | `doctor` check; docs list the port (47420 by default) |

### Open questions

- [x] Rust for the Linux host, Kotlin for the tablet app.
- [x] GPU: the app must not depend on it (F12).
- [x] License: GPL-3.0-or-later.
- [x] Transport for M1–M3: TLS 1.3 over TCP; QUIC decided in M4 by measurement.
- [x] Go/no-go: go; RDP works but misses the CPU and UX targets.
- [x] Client: native Kotlin. A browser client stays a possible later addition.
