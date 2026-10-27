# Security design

TabExtend sends everything on a monitor to another device and lets that
device control the pointer. This document says what it protects, from whom,
and how. Requirements here are binding for the code: a change that weakens
one needs to say so in its pull request.

## What is at stake

| Asset | Why it matters |
|-------|----------------|
| Screen contents | Passwords, messages and documents dragged onto the tablet are visible in the stream |
| Input injection | Whoever controls the input channel can click and type on the host |
| Pairing secrets | A stolen tablet key lets an attacker reconnect as the tablet |
| The host itself | A parsing bug in the stream or input handling could run attacker code |

## Who we defend against

1. **Someone on the same network** (café, hotel, shared flat). Can see and
   send packets, run mDNS, attempt connections. *The main threat*, since
   TabExtend is wireless-first.
2. **A malicious or compromised client** that has been paired, or pretends
   to be. Can send any bytes it wants.
3. **Other local apps** on the host or tablet.

Out of scope: an attacker with root on either device, or physical access to
an unlocked device.

## Controls

### Pairing (F13)

- The host generates a long-term key pair on first run and shows a QR code
  containing its address, port and **certificate fingerprint**, plus a
  one-time pairing secret.
- The tablet pins that fingerprint. It never accepts a host certificate it
  has not pinned, so an attacker on the network cannot impersonate the host.
- Pairing exchanges and stores the tablet's public key on the host. After
  that, only pinned keys can connect: mutual authentication, no passwords.
- The pairing secret is single-use and expires after 2 minutes; 5 wrong
  tokens close the pairing window.
- The host lists paired tablets and can revoke each one.

**Pairing by code** (no camera needed). The tablet finds the host over mDNS,
so it does not know the host's certificate in advance. Both screens show a
6-digit code and the user approves on the computer:

1. The tablet sends a commitment: a hash of a secret random nonce.
2. The host sends its own random nonce.
3. The tablet reveals its nonce; the host checks it against the commitment.
4. Both compute the code from both certificate fingerprints and both nonces
   (domain-separated SHA-256, `crates/core/src/pairing.rs`).

A machine in the middle holds a different certificate on each side, so the
codes differ unless it guesses. Because the tablet commits before seeing the
host's nonce, the attacker cannot grind keys or nonces to make the codes
match: it gets one guess in a million per attempt. Approval happens on the
computer, which grants access, never on the tablet. Only one request is
shown at a time; refusals, failed reveals and timeouts count toward the 5
failures that close the window, so strangers cannot keep asking.

Typing a code into the tablet instead was rejected: without a PAKE, a
machine in the middle would see the typed code and could relay it.

### Transport

- Every byte is encrypted and authenticated with TLS 1.3 (over TCP for now;
  QUIC, also TLS 1.3, if measurements call for it; see PRD Transport).
  No plaintext mode, not even for debugging.
- The host listens only while the user has TabExtend open and waiting for a
  tablet, and only on the interface it was told to use (Wi-Fi, Wi-Fi Direct
  or USB), never `0.0.0.0` by default.
- Unauthenticated peers get no response beyond the TLS handshake failure.
- mDNS announces the service and port. While pairing (`tabextend pair`) it
  also announces the computer's name, so the user can pick it; otherwise
  only a short id derived from the host certificate, so a paired tablet can
  find its host again without strangers learning whose computer it is.

### Input

- Touch input goes through GNOME's RemoteDesktop portal. The user approves
  it in a system dialog and can revoke it; restore tokens are stored with
  user-only permissions.
- Input is clipped to the virtual monitor. Coordinates outside it are
  dropped, so a client cannot move the pointer onto the laptop's own screen.
- The `uinput` fallback for pen pressure, if it is ever needed, is off by
  default, documented as an exception, and still clipped to the virtual
  monitor.

### Parsing untrusted data

Everything the client sends is untrusted.

- Messages are length-prefixed with a hard maximum size; oversized messages
  close the connection.
- Parsers live in `tabextend-core`, in safe Rust, with unit tests for
  malformed input. Fuzzing is planned for the protocol parser before the
  first release.
- No `unsafe` in protocol or input handling.

### Supply chain

- CI runs `cargo deny` on every push: known vulnerabilities (RustSec),
  licenses compatible with GPL-3.0, and crates only from crates.io.
- Dependabot opens pull requests for Cargo and GitHub Actions updates.
- GitHub Actions are pinned to commit SHAs, and workflows get read-only
  tokens unless a job needs more.
- Releases (M5) will be built in CI, with checksums and signed tags.

### Scripts

Scripts in `scripts/` change system settings for testing. Each one must:
back up what it changes with user-only permissions, restore it on `stop`,
roll back if it fails halfway, and keep exposure to the minimum.
`m0-rdp-extend.sh`, for example, allows only the tablet's address through
the firewall, with rules that expire on their own after 2 hours.

## Status

| Control | State |
|---------|-------|
| Security policy and private reporting | done |
| `cargo deny`, Dependabot, pinned actions, read-only CI token | done |
| Test scripts: backup, rollback, firewall restriction | done |
| Pairing by QR code, link or code; TLS 1.3 with pinning; interface binding | done |
| Input clipping, release on disconnect | done (delivery blocked, issue #1) |
| Fuzzing the protocol parser | planned before M5 |
