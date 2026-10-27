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
- **Touch, pointer, buttons, scroll and keys** reach the virtual monitor,
  through libei.
- **Sound on the tablet:** pick "TabExtend: <tablet>" as the output in
  GNOME's Sound settings while connected.

## Does not work yet

### Input: fixed

The portal's own input methods reject every position on a virtual monitor
(they check against a 0×0 stream size); input now goes through libei from
the same portal session. Pen pressure is still missing (#7).

### Frame rate: fixed

Moving content now captures and sends 60 fps, with about 10 ms in the
tablet's decoder. The causes and fixes (decoder buffering, an output loop
that held finished frames back, encoder reference frames) are written up in
[video-pipeline.md](video-pipeline.md).

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
