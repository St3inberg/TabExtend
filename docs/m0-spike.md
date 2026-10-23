# M0 — Spike notes

Goal: measure the baseline and test the existing options (GNOME RDP extend
mode, TethrLink) before writing TabExtend. See the decision gate in `PRD.md`.

## Test setup (2026-10-23)

| Side   | Details |
|--------|---------|
| Host   | Fedora 44, GNOME Shell 50.6, Wayland, kernel 7.2.7 |
| GPUs   | Intel TigerLake-H UHD (iGPU) + NVIDIA RTX 3050 Ti Mobile (proprietary 615.71) |
| Client | Lenovo Idea Tab TB336FU, **Android 16** |
| Wi-Fi  | Same AP, 5 GHz ch 149 / 80 MHz. Laptop 2x2 (~870 Mbit/s link), tablet 1x1 (433 Mbit/s max) |

## Hardware encoders available on this host

Probed by `tabextend doctor` (10-frame test encode at 1280×800), plus a
manual 60-frame `gst-launch-1.0` encode at 2560×1600 for `vah264enc`:

| Element        | Backend                 | 1280×800 | 2560×1600 |
|----------------|-------------------------|----------|-----------|
| `vah264lpenc`  | VA-API low-power (iGPU) | ok       | not tested |
| `vah264enc`    | VA-API (iGPU)           | ok       | ok         |
| `nvh264enc`    | NVENC (dGPU)            | ok       | not tested |
| `qsvh264enc`   | Quick Sync (iGPU)       | ok       | not tested |
| `openh264enc`  | software                | ok       | not tested |
| `x264enc`      | software                | not installed | —     |

`vah264enc` needs RPM Fusion's `intel-media-driver`; Fedora's
`libva-intel-media-driver` ships without H.264/HEVC. The app must probe
encoders at runtime rather than assume any of these exist.

## Wi-Fi round-trip time

ICMP, 150 packets at 200 ms intervals, both directions at the same time:

| Direction       | Loss | min  | avg  | max   | mdev |
|-----------------|------|------|------|-------|------|
| tablet → laptop | 0 %  | 3.8  | 7.5  | 14.6  | 1.7  |
| laptop → tablet | 0 %  | 3.1  | 7.4  | 30.4  | 2.4  |

All values in ms. Network one-way is roughly 4 ms, with spikes to ~15 ms.
That leaves most of the 90 ms glass-to-glass budget for capture, encode,
decode and display.

Not yet measured: sustained throughput at stream bitrates (20–40 Mbit/s),
and latency under that load. Small ICMP packets hide queueing delay.

## Dev setup

Wireless adb is paired (Android 11+ TLS pairing), so the cable is not needed
for development:

```
adb connect <tablet-ip>:<port>   # port from: adb mdns services
```

## GNOME RDP extend mode (2026-10-23)

Run with `scripts/m0-rdp-extend.sh start <tablet-ip>`. Client: Microsoft
Windows App 11.0 for Android.

**Result: black screen.** Login, TLS and session setup work, but no frame
ever reaches the tablet; the client gives up after about 5 seconds.

What the server log (`G_MESSAGES_DEBUG=all`) shows:

- gnome-remote-desktop sizes the virtual monitor to the tablet (2560×1600)
  and opens the PipeWire stream, which reaches `paused` and stays there.
  The layout manager waits in `AWAIT_VIDEO_SIZES` forever.
- The client does **not** offer H.264 (`AVC420: false`, `AVC444: false`), so
  even when it works this client gets a CPU-heavier codec.
- The client supports multi-touch (10 contacts), and the app has touch
  and mouse pointer modes. Not testable without a picture.

Client setup pitfalls: a gateway set to the host address makes the app ask
for gateway credentials; a saved account with a mistyped password fails as
`MIC verification failed` in the server log.

### Retest after re-login: works

After a reboot, the same setup works: the tablet shows the desktop and
GNOME adds a 2560×1600 @ 60 Hz virtual monitor (`Meta-0`). The user found it
usable for dragging windows onto the tablet.

Measured over 2 minutes of use (percent of one core):

| Process | Average | Peak |
|---------|---------|------|
| gnome-remote-desktop | 12.3 % | 73.9 % |
| gnome-shell | 11.6 % | 46.0 % |

Gaps against the PRD targets:

- **CPU:** peaks far above 15 % of one core, because the client takes no
  H.264 and the server must use a CPU codec.
- **Scale:** the virtual monitor comes up at scale 1, so text on a 2560×1600
  11" panel is very small.
- **UX:** username/password login, a gateway setting that is easy to get
  wrong, and a generic RDP client rather than a second-screen app.

**Go/no-go: go.** RDP is a working baseline, so TabExtend focuses on what
it does not give: hardware H.264 (low host CPU), pairing instead of
passwords, scale and placement set for the tablet, direct touch and pen.

### Root cause of the black screen

`scripts/m0-virtual-monitor.py` takes RDP and the tablet out of the picture:
it asks Mutter for a virtual monitor and reads it with GStreamer. It hangs
the same way, and so does recording the built-in screen (`--connector eDP-1`).
GNOME Shell's stream offers normal formats, but the consumer never appears
as a PipeWire client.

Cause, confirmed by a reboot: gnome-shell, mutter and wireplumber were
updated during the session, and the running processes were the old versions
(deleted libraries still mapped). After the reboot both the virtual monitor
and `eDP-1` stream.

TabExtend uses the same Mutter virtual-monitor stream, so this check moves
into `tabextend doctor`: a screencast that cannot deliver a frame must be a
FAIL, not a PASS based on portal properties alone.

## To do

- [x] `tabextend doctor`: all checks pass on the reference laptop
- [x] Reboot, then rerun `scripts/m0-virtual-monitor.py`: both stream
- [ ] Throughput + latency under load (iperf3 on the tablet, or a small test app)
- [x] GNOME RDP extend mode: works after re-login; go for TabExtend, see above
- [ ] TethrLink: latency, pen pressure, does it work over Wi-Fi
- [ ] Compare `vah264lpenc` vs `vah264enc` vs `nvh264enc`: encode latency, CPU, power draw
- [x] Go / no-go: go
