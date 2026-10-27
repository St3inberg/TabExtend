//! Choices that survive restarts: one `key=value` per line in the data
//! directory, next to the certificate.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// GStreamer encoder element; None picks the best that works.
    pub encoder: Option<String>,
    /// Scale streams down to at most this width; None uses the tablet's best.
    pub max_width: Option<u32>,
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("settings")
    }

    /// Missing file or unknown lines give defaults; never an error.
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(Self::path(dir))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = Self::path(dir).with_extension("tmp");
        fs::write(&tmp, self.serialize())?;
        fs::rename(tmp, Self::path(dir))
    }

    fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "encoder" if is_element_name(value) => settings.encoder = Some(value.to_owned()),
                "max_width" => settings.max_width = value.parse().ok().filter(|w| *w >= 320),
                _ => {}
            }
        }
        settings
    }

    fn serialize(&self) -> String {
        let mut out = String::new();
        if let Some(encoder) = &self.encoder {
            out.push_str(&format!("encoder={encoder}\n"));
        }
        if let Some(width) = self.max_width {
            out.push_str(&format!("max_width={width}\n"));
        }
        out
    }
}

/// GStreamer element names: letters, digits, `-` and `_`. Anything else
/// could change the meaning of a pipeline description.
fn is_element_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            encoder: Some("nvh264enc".into()),
            max_width: Some(1920),
        };
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), settings);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn bad_values_are_ignored() {
        let settings = Settings::parse(
            "encoder=x264enc ! filesink location=/tmp/x\nmax_width=12\nnonsense\ncolour=blue\n",
        );
        assert_eq!(settings, Settings::default());
    }
}
