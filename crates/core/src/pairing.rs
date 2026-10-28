//! Pairing a tablet with this host.
//!
//! `tabextend pair` shows a QR code holding a [`PairingLink`]: where to
//! connect, the SHA-256 fingerprint of the host's certificate (so the tablet
//! can pin it), and a single-use token. The tablet connects over TLS with its
//! own certificate and presents the token; the host then remembers the
//! tablet's certificate fingerprint in [`PairedDevices`]. Later connections
//! need no token: both sides check the other's pinned certificate.
//!
//! The other way in is scanning: the tablet finds the host over mDNS, both
//! show a 6-digit [`confirmation_code`], and the user approves on the
//! computer. See docs/security.md for the reasoning.

use std::fmt;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::protocol::{MAX_NAME, NONCE_LEN, PAIRING_TOKEN_LEN};

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

    /// Can a scanning tablet ask for confirmation right now?
    pub fn check_open(&self, now: Instant) -> Result<(), PairingError> {
        if self.used {
            Err(PairingError::AlreadyUsed)
        } else if self.failures >= MAX_PAIRING_FAILURES {
            Err(PairingError::TooManyFailures)
        } else if now >= self.expires_at {
            Err(PairingError::Expired)
        } else {
            Ok(())
        }
    }

    /// Record how a confirmation ended. Approval closes the window like a
    /// redeemed token; a refusal counts toward the failure limit, so a
    /// stranger cannot keep popping up requests.
    pub fn finish_confirmation(&mut self, approved: bool) {
        if approved {
            self.used = true;
        } else {
            self.failures += 1;
        }
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

/// What the tablet sends first in confirm pairing: a hash of its secret
/// nonce. It reveals the nonce only after seeing the host's, so a machine in
/// the middle cannot choose keys or nonces to make two codes match; it gets
/// one blind guess in a million.
pub fn commitment(nonce: &[u8; NONCE_LEN]) -> [u8; NONCE_LEN] {
    let mut hash = Sha256::new();
    hash.update(b"tabextend commitment v1");
    hash.update(nonce);
    hash.finalize().into()
}

/// Does a revealed nonce match the commitment sent earlier?
pub fn commitment_matches(commitment: &[u8; NONCE_LEN], nonce: &[u8; NONCE_LEN]) -> bool {
    constant_time_eq(commitment, &self::commitment(nonce))
}

/// The 6-digit code both screens show. It covers both certificates and both
/// nonces, so if anything sat between the devices, the codes differ.
pub fn confirmation_code(
    host: &Fingerprint,
    client: &Fingerprint,
    host_nonce: &[u8; NONCE_LEN],
    client_nonce: &[u8; NONCE_LEN],
) -> u32 {
    let mut hash = Sha256::new();
    hash.update(b"tabextend confirmation v1");
    hash.update(host.0);
    hash.update(client.0);
    hash.update(host_nonce);
    hash.update(client_nonce);
    let digest = hash.finalize();
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000
}

/// A tablet's self-chosen name, made safe to show and store. The name comes
/// from whoever connects, before they are trusted, so it must not be able
/// to break files or the screen: control characters and invisible
/// formatting characters (direction overrides that reverse text, zero-width
/// spaces that make two names look alike) are removed, whitespace is
/// collapsed and the length capped.
pub fn clean_device_name(name: &str) -> String {
    let invisible = |c: char| {
        c.is_control()
            || matches!(c,
                '\u{00AD}' | '\u{061C}' | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
    };
    // Line breaks and tabs become spaces rather than vanishing, so words
    // stay apart.
    let visible: String = name
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| !invisible(c))
        .collect();
    let name: String = visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME)
        .collect();
    if name.is_empty() {
        "tablet".into()
    } else {
        name
    }
}

/// `123 456`: easier to compare at a glance.
pub fn format_code(code: u32) -> String {
    format!("{:03} {:03}", code / 1000, code % 1000)
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
        self.0.push(PairedDevice {
            fingerprint,
            name: clean_device_name(name),
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

    #[test]
    fn device_names_cannot_hide_or_reverse_text() {
        // A right-to-left override makes "Tablet\u{202E}gnp.exe" read as
        // "Tabletexe.png"; zero-width spaces make two names look alike.
        assert_eq!(clean_device_name("Tablet\u{202E}gnp.exe"), "Tabletgnp.exe");
        assert_eq!(clean_device_name("Lenovo\u{200B} Tab"), "Lenovo Tab");
        assert_eq!(
            clean_device_name("Line\nbreak\tand   spaces "),
            "Line break and spaces"
        );
        assert_eq!(clean_device_name("\u{FEFF}\u{2066}"), "tablet");
        assert_eq!(clean_device_name("   "), "tablet");
        assert_eq!(
            clean_device_name(&"é".repeat(500)).chars().count(),
            MAX_NAME
        );
        // Ordinary names, in any script, are left alone.
        assert_eq!(clean_device_name("Lenovo Idea Tab"), "Lenovo Idea Tab");
        assert_eq!(clean_device_name("タブレット"), "タブレット");
        assert_eq!(clean_device_name("Stephen's <tab>"), "Stephen's <tab>");
    }

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
    fn confirmation_code_known_vector() {
        // Same vector as PairingCodeTest.kt on the tablet.
        let code = confirmation_code(
            &Fingerprint([1; 32]),
            &Fingerprint([2; 32]),
            &[3; NONCE_LEN],
            &[4; NONCE_LEN],
        );
        assert_eq!(format_code(code), "778 062");
        assert_eq!(
            hex(&commitment(&[5; NONCE_LEN])),
            "0db0815fb87772ab13658c18214590e48ab9889ab1aa2f09db39ec051393bb7d"
        );
    }

    #[test]
    fn code_changes_with_every_input() {
        let base = (Fingerprint([1; 32]), Fingerprint([2; 32]), [3; 32], [4; 32]);
        let code = |h: &Fingerprint, c: &Fingerprint, hn: &[u8; 32], cn: &[u8; 32]| {
            confirmation_code(h, c, hn, cn)
        };
        let reference = code(&base.0, &base.1, &base.2, &base.3);
        assert_ne!(
            reference,
            code(&Fingerprint([9; 32]), &base.1, &base.2, &base.3)
        );
        assert_ne!(
            reference,
            code(&base.0, &Fingerprint([9; 32]), &base.2, &base.3)
        );
        assert_ne!(reference, code(&base.0, &base.1, &[9; 32], &base.3));
        assert_ne!(reference, code(&base.0, &base.1, &base.2, &[9; 32]));
        // Swapping host and client is a different code, not the same one.
        assert_ne!(reference, code(&base.1, &base.0, &base.2, &base.3));
    }

    #[test]
    fn commitment_binds_the_nonce() {
        let c = commitment(&[5; NONCE_LEN]);
        assert!(commitment_matches(&c, &[5; NONCE_LEN]));
        assert!(!commitment_matches(&c, &[6; NONCE_LEN]));
    }

    #[test]
    fn codes_always_have_six_digits() {
        assert_eq!(format_code(0), "000 000");
        assert_eq!(format_code(42), "000 042");
        assert_eq!(format_code(999_999), "999 999");
    }

    #[test]
    fn approved_confirmation_closes_the_window() {
        let now = Instant::now();
        let mut window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        assert_eq!(window.check_open(now), Ok(()));
        window.finish_confirmation(true);
        assert_eq!(window.check_open(now), Err(PairingError::AlreadyUsed));
        assert_eq!(
            window.redeem(&[5; PAIRING_TOKEN_LEN], now),
            Err(PairingError::AlreadyUsed)
        );
    }

    #[test]
    fn refused_confirmations_count_as_failures() {
        let now = Instant::now();
        let mut window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        for _ in 0..MAX_PAIRING_FAILURES {
            assert_eq!(window.check_open(now), Ok(()));
            window.finish_confirmation(false);
        }
        assert_eq!(window.check_open(now), Err(PairingError::TooManyFailures));
    }

    #[test]
    fn confirmation_respects_expiry() {
        let now = Instant::now();
        let window = PairingWindow::open([5; PAIRING_TOKEN_LEN], now);
        assert_eq!(
            window.check_open(now + PAIRING_WINDOW),
            Err(PairingError::Expired)
        );
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
        // Separators become plain spaces: one entry, no injected fields.
        assert_eq!(parsed.iter().next().unwrap().name, "evil 0000 1 injected");
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
