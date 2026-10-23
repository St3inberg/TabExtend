# Security policy

TabExtend streams your screen over the network and injects input into your
desktop, so security bugs matter. Thank you for reporting them responsibly.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private reporting instead:
[Report a vulnerability](https://github.com/St3inberg/TabExtend/security/advisories/new).

Please include what you found, how to reproduce it, and what an attacker
could do with it. You should get a reply within 7 days. Once a fix is out,
you will be credited in the advisory unless you prefer not to be.

## Supported versions

TabExtend has no releases yet. Security fixes go to the `main` branch. Once
releases start, the latest release will be supported.

## Scope

In scope: anything that lets someone other than the paired tablet see the
screen, inject input, connect without pairing, or run code on the host or
tablet. Also weaknesses in the scripts under `scripts/` that leave a machine
less secure than before they ran.

The design and the threats it defends against are in
[docs/security.md](docs/security.md).
