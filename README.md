# TabExtend

Use an Android tablet as a wireless extended monitor for GNOME on Wayland.

The tablet shows up in Settings → Displays as a real second screen. Drag
windows onto it, and use touch and pen on it. No cable needed.

> **Status: work in progress.** Pairing and the extended monitor work over
> Wi-Fi; touch input is blocked by a portal limitation with a known way
> around it. Details in [docs/status.md](docs/status.md).

## Install

Download the tablet app and the laptop program from
[Releases](https://github.com/St3inberg/TabExtend/releases), then follow
[Install a release](docs/setup.md#install-a-release-no-building).

Or build from source:

```sh
git clone https://github.com/St3inberg/TabExtend.git
cd TabExtend
cargo build --release
./target/release/tabextend doctor
```

The [setup tutorial](docs/setup.md) covers GStreamer and video drivers on
Fedora, Ubuntu and Arch.

## How it works

1. GNOME creates a virtual monitor through the ScreenCast portal.
2. TabExtend captures it and encodes it with your GPU's H.264 encoder,
   picked at runtime from whatever works on your machine (Intel, AMD,
   NVIDIA, or software as a fallback).
3. The stream goes to the tablet over Wi-Fi, encrypted.
4. Touch and pen input come back and are injected through GNOME's
   RemoteDesktop portal, into the virtual monitor only.

## Documentation

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
