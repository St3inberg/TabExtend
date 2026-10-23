# TabExtend — Product Requirements Document

Oct 22, 2026 · @stephen · revised Oct 23, 2026 (wireless-first, hardware-agnostic)

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

## Existing options and why build

Two working paths already exist on GNOME, so test them before writing code:
they set the bar TabExtend must beat.

| Option | How it works | Gap for this use |
|--------|--------------|------------------|
| GNOME RDP extend mode | Built in since GNOME 42: set screen-share mode to extend, connect from any RDP app | Touch is not mapped as direct input; generic RDP client UX |
| TethrLink | Mutter ScreenCast virtual display → PipeWire → H.264 over USB tethering; GTK4 app | USB only; new project, one client; check pen pressure and packaging |
| Weylus | Browser-based client; touch and pen to PC | Wayland support is experimental; extend mode needs virtual-monitor setup |

**Why build anyway.** A focused app can own the full experience: one-tap
wireless connect, pen pressure and palm rejection, native packaging
(RPM/Flatpak), and tuning for high-refresh tablet panels.

**Decision gate.** Spend one evening on RDP extend mode and TethrLink. If
either meets the latency and input targets below, contribute to it instead of
starting fresh. Progress: [`m0-spike.md`](m0-spike.md).

## Goals, non-goals, target user

**Target user.** A developer or student on GNOME with an Android tablet, who
wants extra screen space for docs, chat or a terminal while travelling.

### Goals (v1)

1. Extend, not mirror: the tablet is a separate monitor in Settings → Displays.
2. No cable needed: connect in under 10 seconds from opening the tablet app.
3. Touch acts as direct pointer input on the tablet's area; pen works with pressure.
4. Smooth enough for reading and light editing (targets below).
5. Runs on any GNOME/Wayland machine: nothing hardcoded to one GPU, encoder,
   tablet model or resolution.

### Non-goals (v1)

- X11, KDE Plasma or other compositors (GNOME/Mutter only first).
- Audio forwarding.
- More than one tablet at a time.
- iPad or Windows clients.
- Use over the internet; local networks and direct links only.

## Functional requirements

| ID  | Requirement | Priority |
|-----|-------------|----------|
| F1  | Host creates a virtual monitor through the ScreenCast portal (virtual source), so GNOME treats it as a real display | Must |
| F2  | Virtual monitor size and refresh come from the tablet's handshake (resolution, density, refresh), never from constants | Must |
| F3  | USB transport (USB tethering or adb reverse) as an optional fast path | Could |
| F4  | Wi-Fi transport with mDNS auto-discovery | Must |
| F5  | Touch on the tablet moves and clicks the pointer inside the virtual monitor only | Must |
| F6  | Pen input with pressure and tilt, exposed as a tablet device for apps like Krita and Xournal++ | Should |
| F7  | Host app: connect, disconnect, quality preset, live latency and FPS | Must |
| F8  | Tablet app: fullscreen immersive mode, keep screen awake, reconnect automatically | Must |
| F9  | Monitor position remembered between sessions (left, right, above) | Should |
| F10 | Quality presets: Battery, Balanced, Sharp (bitrate and resolution) | Could |
| F11 | No-router mode: Wi-Fi Direct, or a laptop hotspot via NetworkManager, when the shared network blocks devices from seeing each other | Should |
| F12 | Encoder chosen at runtime from what works on the machine (VA-API, NVENC, Quick Sync, software), with a user override | Must |
| F13 | Pairing by QR code shown on the host (address, port, certificate fingerprint); 6-digit code as fallback | Must |

## Non-functional requirements

| Area | Target over Wi-Fi 5/6 | Target over USB (stretch) |
|------|-----------------------|---------------------------|
| Glass-to-glass latency | < 90 ms | < 50 ms |
| Frame rate | 30–60 fps while content moves | 60 fps |
| Image quality | Text readable at 100% scale, no smearing on static content, adaptive bitrate | Same |
| Host CPU | < 15% of one core with a hardware encoder | Same |
| Tablet battery | ≥ 3 hours of continuous use | ≥ 4 hours |
| Laptop power | Report extra draw per encoder; prefer the lowest that meets the targets | Same |

**Security.** Pairing is required before any stream starts. The stream is
encrypted (QUIC/TLS or DTLS). The host never listens on interfaces it was not
told to use. Touch input goes only through GNOME's RemoteDesktop portal, so
the user approves access once and can revoke it. Pen pressure may need a
`uinput` fallback (see risks); if so it is opt-in and documented as an
exception.

**Packaging.** RPM via a COPR repo and a Flatpak for the host; APK on GitHub
Releases (F-Droid later).

## Technical architecture

The host asks GNOME for a virtual monitor, captures it, hardware-encodes it
and streams it; the tablet only decodes, displays and sends input back. Input
goes into the virtual monitor only, so the tablet never controls the laptop's
own screen.

**Host (Linux): Rust.** gstreamer-rs, ashpd for the portals, gtk4-rs +
libadwaita for the UI. GStreamer `pipewiresrc` → runtime-selected H.264
encoder. D-Bus portals: `org.freedesktop.portal.ScreenCast`, `RemoteDesktop`.

**Client: Kotlin.** MediaCodec, SurfaceView.

**Transport.** UDP-based: QUIC (`quinn`) or RTP over DTLS. Over Wi-Fi, TCP's
in-order delivery turns one lost packet into a stall for every frame behind
it. Short keyframe interval or forward error correction so a loss costs at
most one frame.

**Code layout.** Logic with no system dependencies (encoder selection, portal
capability decoding, checks) lives in `tabextend-core` and is unit-tested
anywhere. GStreamer and D-Bus code lives in `tabextend-host`.

## Milestones

Each milestone ends with something you can use.

1. **M0 — Spike.** Measure Wi-Fi latency, test GNOME RDP extend mode and
   TethrLink on the tablet. Go/no-go on building. Ships `tabextend doctor`.
2. **M1 — Pixels on the tablet.** Portal virtual monitor → GStreamer →
   encrypted UDP over Wi-Fi. Android app decodes with MediaCodec. Done when
   you can drag a window onto the tablet.
3. **M2 — Touch input.** Tablet sends touch; host injects it through the
   RemoteDesktop portal, mapped to the virtual monitor's stream.
4. **M3 — Real app.** GTK4/Libadwaita host app with presets and stats, QR
   pairing, auto-reconnect, remembered monitor position.
5. **M4 — Pen and no-router mode.** Pressure and tilt; Wi-Fi Direct / hotspot.
6. **M5 — Release.** COPR + Flatpak, APK on GitHub, demo video.

## Risks and open questions

| Risk | Impact | Mitigation |
|------|--------|------------|
| Virtual monitors depend on Mutter; no cross-compositor equivalent | KDE users unsupported | Scope v1 to GNOME; `doctor` reports portal support |
| Portal virtual source vs Mutter's private `RecordVirtual` API | Flatpak may not work if only the private API behaves | Verify in M1 that the portal path shows up in Settings → Displays |
| Portal asks for permission on each session | Annoying UX | Portal restore tokens (ScreenCast v4+, RemoteDesktop v2+) |
| Tablet decoder limits at full resolution | Dropped frames | Start at half resolution with 2× scaling; take limits from MediaCodec capabilities |
| Pen pressure needs a virtual tablet device | Pen works only as mouse | Check libei tablet support; else opt-in `uinput` fallback mapped to the virtual monitor |
| Wi-Fi jitter | Stutter | UDP transport, adaptive bitrate, low-latency encoder settings |
| Hotel/café Wi-Fi isolates clients | Tablet not found | No-router mode (F11) |
| Firewalls differ by distro | Connection refused | `doctor` checks; docs list the port |

### Open questions

- [x] Rust for the Linux host, Kotlin for the tablet app.
- [x] GPU: the app must not depend on it (F12).
- [ ] Kotlin client from scratch, or fork TethrLink's client?
- [ ] QUIC (`quinn`) or RTP over DTLS?
- [x] License: GPL-3.0-or-later.
