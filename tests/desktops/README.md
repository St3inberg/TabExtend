# Desktop tests

TabExtend against real desktops, each running headless in a container, so
nothing is installed on your machine. One end-to-end test
(`real_desktop_streams_takes_input_and_cleans_up` in
`crates/host/tests/end_to_end.rs`) runs inside each and checks: pairing,
the tablet's monitor at the stream size, frames, sound through the virtual
speaker, pointer, button, key and touch input with no errors, and the
monitor removed after the grace period.

| Desktop | Container | Backend | GPU | In CI |
|---|---|---|---|---|
| Sway 1.11 | Fedora 44 | wlroots (screencopy, virtual input) | no | yes |
| Hyprland 0.57 | Arch Linux, nested in headless labwc | wlroots | yes (Mesa) | no |
| KDE Plasma 6.7 (KWin) | Fedora 44 | portal (ScreenCast, RemoteDesktop, libei) | yes (Mesa) | no |
| GNOME 50 | — | portal | — | no |

```sh
tests/desktops/run.sh            # every desktop
tests/desktops/run.sh sway       # one
```

Needs Podman (rootless is fine). Desktops that need a GPU get the first
render node with a Mesa driver; without one they are skipped.

Notes:
- KDE's portal is pre-approved for host apps (`kde-authorized` /
  `remote-desktop` in the permission store), the way KDE supports headless
  machines, because nobody is there to click "Share".
- GNOME's portal has no such switch, so it asks every time and cannot be
  tested unattended. It is tested by hand on the reference machine
  (Fedora 44, GNOME 50).
- Hyprland has no headless mode yet, so it runs as a window inside a
  headless labwc.
