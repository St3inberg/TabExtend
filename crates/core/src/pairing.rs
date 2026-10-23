//! Pairing a tablet with this host.
//!
//! `tabextend pair` shows a QR code holding a [`PairingLink`]: where to
//! connect, the SHA-256 fingerprint of the host's certificate (so the tablet
//! can pin it), and a single-use token. The tablet connects over TLS with its
//! own certificate and presents the token; the host then remembers the
//! tablet's certificate fingerprint in [`PairedDevices`]. Later connections
//! need no token: both sides check the other's pinned certificate.
//!
//! See docs/security.md for the reasoning.

use std::fmt;
use std::time::{Duration, Instant};

use crate::protocol::{MAX_NAME, PAIRING_TOKEN_LEN};

/// How long a pairing token stays valid.
pub const PAIRING_WINDOW: Duration = Duration::from_secs(120);
/// Wrong tokens allowed before the pairing window closes.
pub const MAX_PAIRING_FAILURES: u32 = 5;

/// A SHA-256 certificate fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        Some(Self(unhex(text)?.try_into().ok()?))
    }
}

impl fmt::Display for Fingerprint {
    /// Colon-separated pairs, as GNOME and most tools show fingerprints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pairs: Vec<String> = self.0.iter().map(|b| format!("{b:02x}")).collect();
        write!(f, "{}", pairs.join(":"))
    }
}

pub type PairingToken = [u8; PAIRING_TOKEN_LEN];

/// What the QR code carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    pub host: String,
    pub port: u16,
    pub fingerprint: Fingerprint,
    pub token: PairingToken,
}

const SCHEME: &str = "tabextend://pair?";

impl PairingLink {
    pub fn to_uri(&self) -> String {
        format!(
            "{SCHEME}host={}&port={}&fp={}&token={}",
            self.host,
            self.port,
            self.fingerprint.to_hex(),
            hex(&self.token)
        )
    }

    pub fn parse(uri: &str) -> Option<Self> {
        let query = uri.strip_prefix(SCHEME)?;
        let (mut host, mut port, mut fp, mut token) = (None, None, None, None);
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            let slot = match key {
                "host" => &mut host,
                "port" => &mut port,
                "fp" => &mut fp,
                "token" => &mut token,
                _ => continue,
            };
            if slot.replace(value).is_some() {
                return None; // duplicate key
            }
        }
        let host = host?;
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".:-[]".contains(c))
        {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            port: port?.parse().ok().filter(|p| *p != 0)?,
            fingerprint: Fingerprint::from_hex(fp?)?,
            token: unhex(token?)?.try_into().ok()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    Expired,
    AlreadyUsed,
    WrongToken,
    TooManyFailures,
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expired => "pairing code expired; run `tabextend pair` again",
            Self::AlreadyUsed => "pairing code already used",
            Self::WrongToken => "wrong pairing code",
            Self::TooManyFailures => "too many wrong pairing codes; pairing closed",
        })
    }
}

impl std::error::Error for PairingError {}

/// One open pairing window: a token, valid once, for a limited time.
#[derive(Debug)]
pub struct PairingWindow {
    token: PairingToken,
    expires_at: Instant,
    failures: u32,
    used: bool,
}

impl PairingWindow {
    pub fn open(token: PairingToken, now: Instant) -> Self {
        Self {
            token,
            expires_at: now + PAIRING_WINDOW,
            failures: 0,
            used: false,
        }
    }

    pub fn token(&self) -> &PairingToken {
        &self.token
    }

    pub fn is_open(&self, now: Instant) -> bool {
        !self.used && self.failures < MAX_PAIRING_FAILURES && now < self.expires_at
    }

    /// Check a presented token. On success the window closes for good.
    pub fn redeem(&mut self, presented: &PairingToken, now: Instant) -> Result<(), PairingError> {
        if self.used {
            return Err(PairingError::AlreadyUsed);
        }
        if self.failures >= MAX_PAIRING_FAILURES {
            return Err(PairingError::TooManyFailures);
        }
        if now >= self.expires_at {
            return Err(PairingError::Expired);
        }
        if !constant_time_eq(&self.token, presented) {
            self.failures += 1;
            return Err(if self.failures >= MAX_PAIRING_FAILURES {
                PairingError::TooManyFailures
            } else {
                PairingError::WrongToken
            });
        }
        self.used = true;
        Ok(())
    }
}

/// Compare without stopping at the first difference, so timing does not
/// reveal how much of a guess was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    pub fingerprint: Fingerprint,
    pub name: String,
    /// Seconds since the Unix epoch.
    pub paired_at: u64,
}

/// The tablets this host trusts. Stored as one line per device:
/// `<fingerprint hex>\t<paired_at>\t<name>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairedDevices(Vec<PairedDevice>);

impl PairedDevices {
    /// Parse the stored file. Malformed lines are skipped and counted, so one
    /// bad line does not lock every tablet out.
    pub fn parse(text: &str) -> (Self, usize) {
        let mut devices = Vec::new();
        let mut skipped = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut fields = line.splitn(3, '\t');
            let parsed = (|| {
                Some(PairedDevice {
                    fingerprint: Fingerprint::from_hex(fields.next()?)?,
                    paired_at: fields.next()?.parse().ok()?,
                    name: fields.next()?.to_owned(),
                })
            })();
            match parsed {
                Some(device) => devices.push(device),
                None => skipped += 1,
            }
        }
        (Self(devices), skipped)
    }

    pub fn serialize(&self) -> String {
        self.0
            .iter()
            .map(|d| format!("{}\t{}\t{}\n", d.fingerprint.to_hex(), d.paired_at, d.name))
            .collect()
    }

    pub fn contains(&self, fingerprint: &Fingerprint) -> bool {
        self.0.iter().any(|d| &d.fingerprint == fingerprint)
    }

    pub fn find(&self, fingerprint: &Fingerprint) -> Option<&PairedDevice> {
        self.0.iter().find(|d| &d.fingerprint == fingerprint)
    }

    /// Add a device, replacing any entry with the same fingerprint. Names are
    /// cleaned so they cannot break the file format.
    pub fn add(&mut self, fingerprint: Fingerprint, name: &str, paired_at: u64) {
        self.remove(&fingerprint);
        let name: String = name
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME)
            .collect();
        self.0.push(PairedDevice {
            fingerprint,
            name: if name.trim().is_empty() {
                "tablet".into()
            } else {
                name
            },
            paired_at,
        });
    }

    pub fn remove(&mut self, fingerprint: &Fingerprint) -> bool {
        let before = self.0.len();
        self.0.retain(|d| &d.fingerprint != fingerprint);
        self.0.len() != before
    }

    pub fn iter(&self) -> impl Iterator<Item = &PairedDevice> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> PairingLink {
        PairingLink {
            host: "10.134.124.2".into(),
            port: 47420,
            fingerprint: Fingerprint([0xab; 32]),
            token: [0x01; PAIRING_TOKEN_LEN],
        }
    }

    #[test]
    fn link_round_trips() {
        let uri = link().to_uri();
        assert!(uri.starts_with("tabextend://pair?host=10.134.124.2&port=47420&fp=abab"));
        assert_eq!(PairingLink::parse(&uri), Some(link()));
    }

    #[test]
    fn link_accepts_ipv6_and_ignores_unknown_keys() {
        let uri = link().to_uri().replace("10.134.124.2", "[fe80::1]") + "&v=2";
        assert_eq!(PairingLink::parse(&uri).unwrap().host, "[fe80::1]");
    }

    #[test]
    fn malformed_links_are_rejected() {
        let good = link().to_uri();
        for bad in [
            good.replace("tabextend://", "https://"),
            good.replace("port=47420", "port=0"),
            good.replace("port=47420", "port=99999"),
            good.replace("&fp=", "&fp=zz"),
            good.replace("&token=", "&token=0"),
            good.replace("10.134.124.2", "evil host/"),
            good.replace("10.134.124.2", ""),
            format!("{good}&token=00"),
            good.replace("&fp=", "&nofp="),
        ] {
            assert_eq!(PairingLink::parse(&bad), None, "accepted {bad}");
        }
    }

    #[test]
    fn fingerprint_display_matches_gnome_style() {
        let fp = Fingerprint([0xe3; 32]);
        assert!(fp.to_string().starts_with("e3:e3:"));
        assert_eq!(fp.to_string().len(), 32 * 3 - 1);
    }

    #[test]
    fn correct_token_pairs_once() {
        let now = Instant::now();
        let mut window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        assert_eq!(window.redeem(&[5; PAIRING_TOKEN_LEN], now), Ok(()));
        assert!(!window.is_open(now));
        assert_eq!(
            window.redeem(&[5; PAIRING_TOKEN_LEN], now),
            Err(PairingError::AlreadyUsed)
        );
    }

    #[test]
    fn token_expires() {
        let now = Instant::now();
        let mut window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        let later = now + PAIRING_WINDOW;
        assert!(!window.is_open(later));
        assert_eq!(
            window.redeem(&[5; PAIRING_TOKEN_LEN], later),
            Err(PairingError::Expired)
        );
    }

    #[test]
    fn wrong_tokens_close_the_window() {
        let now = Instant::now();
        let mut window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        for _ in 1..MAX_PAIRING_FAILURES {
            assert_eq!(
                window.redeem(&[6; PAIRING_TOKEN_LEN], now),
                Err(PairingError::WrongToken)
            );
        }
        assert_eq!(
            window.redeem(&[6; PAIRING_TOKEN_LEN], now),
            Err(PairingError::TooManyFailures)
        );
        // Even the right token is refused now.
        assert_eq!(
            window.redeem(&[5; PAIRING_TOKEN_LEN], now),
            Err(PairingError::TooManyFailures)
        );
        assert!(!window.is_open(now));
    }

    #[test]
    fn paired_devices_round_trip() {
        let mut devices = PairedDevices::default();
        devices.add(Fingerprint([1; 32]), "Idea Tab", 1_700_000_000);
        devices.add(Fingerprint([2; 32]), "Other", 1_700_000_001);
        let (parsed, skipped) = PairedDevices::parse(&devices.serialize());
        assert_eq!(skipped, 0);
        assert_eq!(parsed, devices);
        assert!(parsed.contains(&Fingerprint([1; 32])));
    }

    #[test]
    fn re_pairing_replaces_the_old_entry() {
        let mut devices = PairedDevices::default();
        devices.add(Fingerprint([1; 32]), "old", 1);
        devices.add(Fingerprint([1; 32]), "new", 2);
        assert_eq!(devices.iter().count(), 1);
        assert_eq!(devices.find(&Fingerprint([1; 32])).unwrap().name, "new");
    }

    #[test]
    fn names_cannot_break_the_file_format() {
        let mut devices = PairedDevices::default();
        devices.add(Fingerprint([1; 32]), "evil\n0000\t1\tinjected", 1);
        let (parsed, skipped) = PairedDevices::parse(&devices.serialize());
        assert_eq!(skipped, 0);
        assert_eq!(parsed.iter().count(), 1);
        assert_eq!(parsed.iter().next().unwrap().name, "evil00001injected");
    }

    #[test]
    fn bad_lines_are_skipped_not_fatal() {
        let text = format!(
            "garbage\n{}\t5\tok\nabcd\t1\tshort-fp\n\n",
            Fingerprint([9; 32]).to_hex()
        );
        let (parsed, skipped) = PairedDevices::parse(&text);
        assert_eq!(parsed.iter().count(), 1);
        assert_eq!(skipped, 2);
    }

    #[test]
    fn remove_reports_whether_anything_was_removed() {
        let mut devices = PairedDevices::default();
        devices.add(Fingerprint([1; 32]), "a", 1);
        assert!(devices.remove(&Fingerprint([1; 32])));
        assert!(!devices.remove(&Fingerprint([1; 32])));
        assert!(devices.is_empty());
    }
}
