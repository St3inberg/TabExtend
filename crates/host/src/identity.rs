//! The host's long-term certificate and the list of paired tablets.
//!
//! Both live in `$XDG_DATA_HOME/tabextend` (usually `~/.local/share/tabextend`),
//! in a directory only the user can open, with files only the user can read.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tabextend_core::pairing::{Fingerprint, PairedDevices};

const CERT_FILE: &str = "host-cert.der";
const KEY_FILE: &str = "host-key.der";
const PAIRED_FILE: &str = "paired-devices";

pub fn fingerprint(cert: &[u8]) -> Fingerprint {
    Fingerprint(Sha256::digest(cert).into())
}

/// Where TabExtend keeps its state.
pub fn default_data_dir() -> io::Result<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::home_dir()
            .ok_or_else(|| io::Error::other("cannot find the home directory"))?
            .join(".local/share"),
    };
    Ok(base.join("tabextend"))
}

pub struct HostIdentity {
    pub cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    pub fingerprint: Fingerprint,
}

impl HostIdentity {
    /// Load the certificate from `dir`, creating one on first run.
    pub fn load_or_create(dir: &Path) -> io::Result<Self> {
        ensure_private_dir(dir)?;
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if !cert_path.exists() || !key_path.exists() {
            let key = rcgen::KeyPair::generate().map_err(io::Error::other)?;
            let params = rcgen::CertificateParams::new(vec!["tabextend".to_owned()])
                .map_err(io::Error::other)?;
            let cert = params.self_signed(&key).map_err(io::Error::other)?;
            write_private(&key_path, &key.serialize_der())?;
            write_private(&cert_path, cert.der())?;
        }
        let cert = CertificateDer::from(fs::read(&cert_path)?);
        let key = PrivatePkcs8KeyDer::from(fs::read(&key_path)?);
        Ok(Self {
            fingerprint: fingerprint(&cert),
            cert,
            key,
        })
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

/// The paired-devices file.
pub struct PairedStore {
    path: PathBuf,
}

impl PairedStore {
    pub fn new(dir: &Path) -> io::Result<Self> {
        ensure_private_dir(dir)?;
        Ok(Self {
            path: dir.join(PAIRED_FILE),
        })
    }

    pub fn load(&self) -> io::Result<PairedDevices> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let (devices, skipped) = PairedDevices::parse(&text);
                if skipped > 0 {
                    eprintln!(
                        "warning: skipped {skipped} unreadable line(s) in {}",
                        self.path.display()
                    );
                }
                Ok(devices)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(PairedDevices::default()),
            Err(e) => Err(e),
        }
    }

    /// Replace the file atomically, so a crash never leaves it half-written.
    pub fn save(&self, devices: &PairedDevices) -> io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        write_private(&tmp, devices.serialize().as_bytes())?;
        fs::rename(&tmp, &self.path)
    }

    pub fn add(&self, fingerprint: Fingerprint, name: &str) -> io::Result<()> {
        let mut devices = self.load()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        devices.add(fingerprint, name, now);
        self.save(&devices)
    }
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = HostIdentity::load_or_create(dir.path()).unwrap();
        let second = HostIdentity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
    }

    #[test]
    fn key_and_directory_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("tabextend");
        HostIdentity::load_or_create(&state).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&state.join(KEY_FILE)), 0o600);
    }

    #[test]
    fn paired_store_round_trips_and_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairedStore::new(dir.path()).unwrap();
        assert!(store.load().unwrap().is_empty());
        store.add(Fingerprint([3; 32]), "tab").unwrap();
        assert!(store.load().unwrap().contains(&Fingerprint([3; 32])));
        let mode = fs::metadata(dir.path().join(PAIRED_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
