# Setting up TabExtend

This tutorial gets a laptop and a tablet ready to use TabExtend: from a
release download, or built from source.

> **Status: work in progress.** The tablet works as an extended screen over
> Wi-Fi. Touch and pen input do not work yet ([#1](https://github.com/St3inberg/TabExtend/issues/1));
> see [status.md](status.md).

## What you need

- A Linux laptop running **GNOME on Wayland**. TabExtend is developed on
  Fedora 44 with GNOME 50. Older GNOME releases may work; `doctor` will tell
  you.
- An **Android tablet**, Android 11 or later.
- Both on the same Wi-Fi network. No cable is needed.

## Install a release (no building)

Downloads are on the [Releases page](https://github.com/St3inberg/TabExtend/releases).
Early releases are pre-releases: read the notes for what works.

**On the tablet** (Android 11 or later, any brand):

1. Download `tabextend-<version>.apk` on the tablet.
2. Open it. Android asks to allow installs from your browser or file app;
   allow it for this one install.
3. To check the download is genuine, compare the signing certificate in the
   release notes with what `apksigner verify --print-certs` reports.

If you installed a build from source before, uninstall it first: releases
are signed with a different key, so Android will not update over it. You
will need to pair again.

**On the laptop** (GNOME on Wayland, x86-64):

1. Install GStreamer and your GPU's video driver: steps 1 and 2 below
   (the runtime packages are enough; skip `-devel`/`-dev` and the Rust tools).
2. Download the desktop app, `tabextend-app-<version>-linux-x86_64.tar.gz`
   (needs libadwaita 1.5 or newer: Fedora 40+, Ubuntu 24.04+, Debian 13+),
   then check and install it for your user:

   ```sh
   sha256sum -c tabextend-app-<version>-linux-x86_64.tar.gz.sha256
   tar xzf tabextend-app-<version>-linux-x86_64.tar.gz
   cd tabextend-app-<version>-linux-x86_64
   ./install.sh            # ./install.sh --uninstall removes it
   ```

   On an older distribution, take `tabextend-<version>-linux-x86_64.tar.gz`
   instead: the same host as a command-line program (`./tabextend doctor`,
   `./tabextend pair`, `./tabextend serve`).

3. Open **TabExtend** from your app list and choose **Pair a Tablet**.
   On the tablet, tap **Scan QR code** and point the camera at the QR code.
   Or tap **Find computers**, pick your computer, check both screens show
   the same 6-digit code, and choose **Allow** on the computer. The first
   time, GNOME asks to share the screen: turn on **Allow remote
   interaction**, then share. After that, opening TabExtend on both is enough.

## Build from source

## 1. Install the build tools and GStreamer

Pick your distribution. Only Fedora is tested so far; the other package
names are best-effort, and corrections are welcome.

**Fedora**

```sh
sudo dnf install git cargo rustfmt clippy gstreamer1-devel \
    gstreamer1-plugins-base gstreamer1-plugins-bad-free pipewire-gstreamer \
    pulseaudio-utils
```

**Ubuntu / Debian**

```sh
sudo apt install git cargo libgstreamer1.0-dev gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-bad gstreamer1.0-pipewire pulseaudio-utils
```

**Arch**

```sh
sudo pacman -S git rust gstreamer gst-plugins-base gst-plugins-bad gst-plugin-pipewire libpulse
```

`pulseaudio-utils` (`libpulse` on Arch) provides `pactl`, which TabExtend
uses to add the tablet as a speaker. It works with PipeWire too.

## 2. Install a hardware video encoder

TabExtend encodes your screen as H.264. It works with whatever encoder your
machine has and picks the best one automatically, so install the driver for
**your** GPU. If you have two GPUs (common on laptops), installing both
gives `doctor` more to choose from.

Not sure what you have? Run `lspci | grep -Ei 'vga|3d'`.

| GPU | Fedora | Ubuntu / Debian | Arch |
|-----|--------|-----------------|------|
| Intel | `intel-media-driver` from RPM Fusion (see below) | `intel-media-va-driver-non-free` | `intel-media-driver` |
| AMD | `mesa-va-drivers-freeworld` from RPM Fusion (see below) | `mesa-va-drivers` | Mesa's VA-API driver |
| NVIDIA | the proprietary driver (`akmod-nvidia` from RPM Fusion) | the proprietary driver | `nvidia` |
| None / fallback | `gstreamer1-plugin-openh264` | `gstreamer1.0-plugins-ugly` | `gst-plugins-ugly` |

**Fedora note.** Fedora's own Intel and AMD video drivers are built without
H.264 encoding for patent reasons. The encoder element still shows up, but
it cannot encode, so `doctor` reports it as failed. Replace the driver with
RPM Fusion's:

```sh
# Enable RPM Fusion (once)
sudo dnf install \
  https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm \
  https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm

# Intel
sudo dnf swap libva-intel-media-driver intel-media-driver --allowerasing
# AMD
sudo dnf swap mesa-va-drivers mesa-va-drivers-freeworld
```

## 3. Build TabExtend

```sh
git clone https://github.com/St3inberg/TabExtend.git
cd TabExtend
cargo build --release
```

The binary is `target/release/tabextend`.

## 4. Check your machine

```sh
./target/release/tabextend doctor
```

Run it from inside your desktop session, not over SSH: it needs your
session's D-Bus to reach the portals. It takes a few seconds while it
test-encodes with each encoder.

Example output from the reference laptop (Intel + NVIDIA):

```
[PASS] Desktop session: Wayland on GNOME
[PASS] ScreenCast portal: version 5, virtual monitors, restore tokens
[PASS] RemoteDesktop portal: version 2, touchscreen input
[PASS] H.264 encoder: vah264lpenc via VA-API low-power (Intel) (working: vah264lpenc, vah264enc, nvh264enc, qsvh264enc, openh264enc)
[PASS] mDNS discovery: Avahi running

This machine is ready for TabExtend.

Encoders, in the order TabExtend tries them:
  vah264lpenc    VA-API low-power (Intel)   works
  vah264enc      VA-API (Intel/AMD)         works
  nvh264enc      NVENC (NVIDIA)             works
  qsvh264enc     Quick Sync (Intel)         works
  x264enc        x264 (software)            not installed
  openh264enc    OpenH264 (software)        works
```

Each line is `PASS`, `WARN` or `FAIL`:

- **PASS**: ready.
- **WARN**: TabExtend will run, but something is degraded. The line below it
  says what and how to fix it.
- **FAIL**: TabExtend cannot run until this is fixed. `doctor` exits with
  status 1 if anything fails, so you can use it in scripts.

### Choosing an encoder yourself

`doctor` lists every encoder it tried, in the order TabExtend prefers them.
To force one, or try one that is not on the list:

```sh
./target/release/tabextend doctor --encoder nvh264enc
# or
TABEXTEND_ENCODER=nvh264enc ./target/release/tabextend doctor
```

## Troubleshooting

| `doctor` says | What to do |
|---------------|------------|
| Desktop session: x11 session | Log out, pick "GNOME" (not "GNOME on Xorg") on the login screen. |
| ScreenCast portal: not available | Install `xdg-desktop-portal` and `xdg-desktop-portal-gnome`, then log out and back in. |
| ScreenCast portal: no virtual monitors | Your portal backend is too old to create extra monitors. Update GNOME. |
| H.264 encoder: none working | Install a driver from step 2. |
| H.264 encoder: `vah264enc` failed | On Fedora, the codec-stripped driver is installed; see the Fedora note in step 2. |
| H.264 encoder: only software works | It will run, but with high CPU use. Install your GPU's driver from step 2. |
| mDNS discovery: Avahi not running | `sudo systemctl enable --now avahi-daemon`. |
| Sound on the tablet: pactl unavailable | Install `pulseaudio-utils` (`libpulse` on Arch). The picture works without it. |

## Next

The tablet app and streaming arrive in milestone M1. If you want to help
build them, see [development.md](development.md), which also covers connecting
to the tablet over Wi-Fi for debugging.
