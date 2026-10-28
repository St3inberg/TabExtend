# TabExtend

Use an Android tablet as a wireless extended monitor for your Linux desktop.

The tablet shows up as a real second screen. Drag windows onto it and use
it with touch, a pen, a mouse or a keyboard; the computer's sound can play
on it too. No cable needed.

> **Status: early releases.** Streaming, input, sound and pairing work and
> are tested on real desktops; see [docs/status.md](docs/status.md) for
> known issues.

## Desktops

| Desktop | How | Tested |
|---|---|---|
| GNOME 46+ | ScreenCast + RemoteDesktop portals, libei input | by hand on real hardware (Fedora 44, GNOME 50) |
| KDE Plasma 6.1+ | same portals; KDE's fixed-size screen is fitted with bars | automatically, headless KWin 6.7 |
| Sway | headless output, screencopy, virtual pointer and keyboard | automatically, on every push (Sway 1.11) |
| Hyprland | same as Sway | automatically, headless Hyprland 0.57 |

X11 is not supported. On Sway and Hyprland, touch drives the pointer
(wlroots has no virtual touch device). [tests/desktops](tests/desktops/)
runs each desktop in a container.

## Install

From [Releases](https://github.com/St3inberg/TabExtend/releases):

- **Tablet** (Android 11+): `tabextend-<version>.apk`.
- **Computer, any distribution (GNOME, KDE):** the Flatpak,
  `tabextend-<version>.flatpak`:

  ```sh
  flatpak install --user tabextend-<version>.flatpak
  flatpak run io.github.st3inberg.TabExtend
  ```

- **Computer, native (also Sway and Hyprland):** the
  `tabextend-app-<version>-linux-x86_64.tar.gz` archive with `install.sh`.

Step by step: [Install a release](docs/setup.md#install-a-release-no-building).

Or build from source:

```sh
git clone https://github.com/St3inberg/TabExtend.git
cd TabExtend
cargo build --release
./target/release/tabextend doctor     # check this computer
./target/release/tabextend-app        # the desktop app
```

The [setup tutorial](docs/setup.md) covers GStreamer and video drivers on
Fedora, Ubuntu and Arch.

## How it works

1. The desktop creates a virtual monitor for the tablet: through the
   ScreenCast portal on GNOME and KDE, as a headless output on Sway and
   Hyprland.
2. TabExtend captures it and encodes it with your GPU's H.264 encoder,
   picked at runtime from whatever works on your machine (Intel, AMD,
   NVIDIA, or software as a fallback).
3. The stream goes to the tablet over Wi-Fi, encrypted.
4. Touch, pen, mouse and keyboard input come back and are injected into
   the virtual monitor only.

## Support

TabExtend is free and open source, made in spare time. If it is useful to
you, you can [buy me a coffee](https://buymeacoffee.com/st3nberg) to keep it going. Thank you!

## Documentation

- [How it works](docs/architecture.md): diagrams of the whole program and its algorithms
- [Status](docs/status.md): what works, what does not and why
- [Setup tutorial](docs/setup.md): install, build and check your machine
- [Development guide](docs/development.md): code layout, tests, tablet debugging
- [Product requirements](docs/PRD.md): goals, requirements, roadmap
- [M0 spike notes](docs/m0-spike.md): measurements on the reference hardware
- [Video pipeline](docs/video-pipeline.md): how frames get to the tablet fast, and why
- [Security design](docs/security.md): threat model and controls; report issues via [SECURITY.md](SECURITY.md)
- [Contributing](CONTRIBUTING.md)

## License

TabExtend is free software under the [GNU General Public License v3.0 or later](LICENSE).
