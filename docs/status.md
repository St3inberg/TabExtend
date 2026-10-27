# Status

Work in progress. Last updated 2026-10-27, on the reference hardware
(Fedora 44, GNOME 50.6, Lenovo Idea Tab TB336FU).

## Works

- **Pairing three ways**, all over TLS 1.3 with both certificates pinned
  and the tablet's key in Android Keystore: scan the QR code with the
  app's camera scanner, paste the pairing link, or pick the computer from
  the network and compare a 6-digit code (approve on the computer).
- **Extended monitor over Wi-Fi.** GNOME adds a virtual monitor next to the
  laptop screen; windows dragged onto it show on the tablet.
- **Stream sized by the tablet's decoder.** The TB336FU's H.264 decoder
  cannot do 2560×1600, so it asks for 1808×1130, the largest it decodes at
  60 fps, and scales it to the panel.
- **Hardware encoding chosen at runtime** (VA-API on the reference laptop),
  with zero-copy GPU buffers from GNOME where the encoder chain takes them.
- **Reconnects** automatically; GNOME's permission is remembered.

## Does not work yet

### Touch, pen, mouse and keyboard input

The tablet sends input and the host maps it safely (clipped to the virtual
monitor, released on disconnect; see `crates/core/src/input.rs`), but GNOME
rejects it with `Invalid position`.

Cause, found in `xdg-desktop-portal` (`src/remote-desktop.c`,
`check_position`): every absolute pointer or touch position is checked
against the stream size reported when the session starts. A virtual
monitor's size is only negotiated afterwards, so the portal sees 0×0 and
rejects every position. This is not specific to TabExtend; GNOME's own RDP
server avoids it by talking to Mutter directly.

**Next step: input through libei.** The same portal session can hand out a
libei connection (`ConnectToEIS`, RemoteDesktop portal v2). Mutter's EIS
server maps input onto each monitor, virtual ones included, using regions
tagged with the stream's mapping id, and it supports touch
(`meta-eis-client.c`: `EIS_DEVICE_CAP_TOUCH`). The user still approves once
in GNOME's dialog. The `reis` crate implements the libei protocol in Rust.

The current fallback (touch refused, so fingers drive the pointer) stays
for desktops whose EIS lacks touch.

Also worth reporting upstream: the portal could skip or defer the position
check for virtual streams.

### Frame rate

Moving content reaches about 12–16 fps; the network is not the limit (no
drops, round trip under 15 ms). The stats line shows captured and sent fps
separately to locate it. Measure again once input works, since injected
input is the easiest way to produce steady motion.

### Not started

- Finding the paired host again by its id when its address changes.
- Desktops other than GNOME (KDE Plasma next).
- Scale and placement of the virtual monitor (text is small at scale 1).
- Pen pressure (no pen device in the portal or libei yet).

## Try it

```sh
tabextend pair                 # first time: shows a QR code
tabextend serve                # afterwards
tabextend forget-permission    # make GNOME ask again
```

In GNOME's sharing dialog, turn on **Allow remote interaction**.
