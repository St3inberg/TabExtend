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

## To do

- [x] `tabextend doctor`: all checks pass on the reference laptop
- [ ] Throughput + latency under load (iperf3 on the tablet, or a small test app)
- [ ] GNOME RDP extend mode: glass-to-glass latency, touch behaviour
- [ ] TethrLink: latency, pen pressure, does it work over Wi-Fi
- [ ] Compare `vah264lpenc` vs `vah264enc` vs `nvh264enc`: encode latency, CPU, power draw
- [ ] Go / no-go
