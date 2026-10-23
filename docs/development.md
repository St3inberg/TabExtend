# Development guide

How the code is organised, how to run the tests, and how to work with a
tablet without a cable.

## Repository layout

```
crates/
  core/    tabextend-core: pure logic, no system libraries
           encoder.rs  pick an H.264 encoder from what works
           portal.rs   decode ScreenCast / RemoteDesktop capabilities
           check.rs    the checks behind `tabextend doctor`
  host/    tabextend-host: the `tabextend` binary
           probe.rs    test-encode with real GStreamer pipelines
           portals.rs  read portal properties over D-Bus
           doctor.rs   gather facts, run checks, print the report
           tests/      integration tests against the installed GStreamer
docs/      PRD, setup tutorial, spike notes, this guide
```

The split is deliberate. Anything that decides something (which encoder,
whether a portal is good enough) lives in `core` and takes plain values, so
it can be tested without a GPU, a desktop session or GStreamer. `host` only
gathers facts and passes them in.

## Prerequisites

Everything in [setup.md](setup.md), plus the Rust tools:

```sh
# Fedora
sudo dnf install cargo rustfmt clippy
# elsewhere
rustup component add rustfmt clippy
```

## Running the tests

```sh
cargo test --workspace          # all tests
cargo test -p tabextend-core    # pure logic only; needs no system libraries
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs the same four commands on every push and pull request
(`.github/workflows/ci.yml`).

What is tested where:

| Tests | Needs | Covers |
|-------|-------|--------|
| `core` unit tests | nothing | encoder priority and fallback, user override, portal bitmasks and versions, every check's pass/warn/fail rules |
| `host/tests/gst_probe.rs` | GStreamer core + base plugins | the probe pipeline: missing elements, elements that reject video, a pipeline that completes |

Hardware encoders are not tested in CI, since CI machines have no GPU. Run
`tabextend doctor` on real hardware to see them.

### Adding a known encoder

Add it to `default_h264_encoders()` in `crates/core/src/encoder.rs`, in
priority order. Hardware before software; a test enforces that. Users can
already pass any element with `--encoder`, so only add elements that should
be tried by default.

### Adding a doctor check

1. In `crates/core/src/check.rs`, add a struct holding the facts it needs and
   implement `Check` for it. Add tests for each pass/warn/fail case.
2. In `crates/host/src/doctor.rs`, gather the facts and add the check to the list.

## Working with the tablet over Wi-Fi

The tablet app does not exist yet, but you will want `adb` for it. Android 11
and later can debug over Wi-Fi with no cable.

1. On the tablet: Settings → About tablet, tap **Build number** seven times.
2. Settings → System → Developer options → **Wireless debugging** → on.
   Allow it for your network when asked.
3. Tap **Pair device with pairing code**. Leave the dialog open; it shows a
   six-digit code and an `IP:port`.
4. On the laptop (same network):

   ```sh
   adb pair <ip>:<pairing-port> <code>
   adb connect $(adb mdns services | awk '/tls-connect/{print $3}')
   adb devices
   ```

Pairing is a one-time step. The connect port changes when wireless debugging
restarts, which is why the command above reads it from mDNS instead of
having you type it.

If `adb mdns services` shows nothing, Avahi may be off
(`systemctl status avahi-daemon`) or your firewall may block mDNS (UDP 5353).
