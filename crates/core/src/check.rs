//! Host readiness checks, as shown by `tabextend doctor`.
//!
//! Each check is built from facts the host has already gathered (environment
//! variables, D-Bus properties, encoder probes), so evaluating one does no I/O
//! and can be tested with plain values.

use crate::encoder::{ProbeOutcome, ProbeReport};
use crate::portal::{RemoteDesktopCaps, ScreenCastCaps};

/// How serious a finding is. Ordered from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    /// Works, but something is degraded or optional is missing.
    Warn,
    /// TabExtend cannot work until this is fixed.
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub status: Status,
    pub summary: String,
    /// What to do about it, when not passing.
    pub hint: Option<String>,
}

impl Finding {
    pub fn pass(summary: impl Into<String>) -> Self {
        Self {
            status: Status::Pass,
            summary: summary.into(),
            hint: None,
        }
    }

    pub fn warn(summary: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            status: Status::Warn,
            summary: summary.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn fail(summary: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            status: Status::Fail,
            summary: summary.into(),
            hint: Some(hint.into()),
        }
    }
}

pub trait Check {
    fn name(&self) -> &str;
    fn evaluate(&self) -> Finding;
}

/// Results of a full run, in the order the checks were given.
pub struct Report {
    pub results: Vec<(String, Finding)>,
}

impl Report {
    pub fn run(checks: &[Box<dyn Check>]) -> Self {
        Self {
            results: checks
                .iter()
                .map(|check| (check.name().to_owned(), check.evaluate()))
                .collect(),
        }
    }

    pub fn worst(&self) -> Status {
        self.results
            .iter()
            .map(|(_, finding)| finding.status)
            .max()
            .unwrap_or(Status::Pass)
    }
}

/// Wayland session on GNOME.
pub struct SessionCheck {
    /// `XDG_SESSION_TYPE`
    pub session_type: Option<String>,
    /// `XDG_CURRENT_DESKTOP`, colon-separated
    pub desktop: Option<String>,
}

impl Check for SessionCheck {
    fn name(&self) -> &str {
        "Desktop session"
    }

    fn evaluate(&self) -> Finding {
        let session = self.session_type.as_deref().unwrap_or("unknown");
        let desktop = self.desktop.as_deref().unwrap_or("unknown");
        if session != "wayland" {
            return Finding::fail(
                format!("{session} session"),
                "Log in to a Wayland session. TabExtend does not support X11.",
            );
        }
        if !desktop.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")) {
            return Finding::warn(
                format!("Wayland on {desktop}"),
                "TabExtend is developed on GNOME. Other desktops may lack the virtual-monitor portal.",
            );
        }
        Finding::pass(format!("Wayland on {desktop}"))
    }
}

/// ScreenCast portal with virtual-monitor support.
pub struct ScreenCastCheck {
    pub caps: Result<ScreenCastCaps, String>,
}

impl Check for ScreenCastCheck {
    fn name(&self) -> &str {
        "ScreenCast portal"
    }

    fn evaluate(&self) -> Finding {
        let caps = match &self.caps {
            Ok(caps) => caps,
            Err(error) => {
                return Finding::fail(
                    format!("not available ({error})"),
                    "Install xdg-desktop-portal and your desktop's backend (xdg-desktop-portal-gnome).",
                );
            }
        };
        if !caps.supports_virtual_monitor() {
            return Finding::fail(
                format!("version {}, no virtual monitors", caps.version),
                "Your portal backend can only mirror screens. Update xdg-desktop-portal-gnome.",
            );
        }
        if !caps.supports_restore_tokens() {
            return Finding::warn(
                format!("version {}, virtual monitors", caps.version),
                "This portal cannot remember permission, so you will be asked on every connect.",
            );
        }
        Finding::pass(format!(
            "version {}, virtual monitors, restore tokens",
            caps.version
        ))
    }
}

/// RemoteDesktop portal with touch input.
pub struct RemoteDesktopCheck {
    pub caps: Result<RemoteDesktopCaps, String>,
}

impl Check for RemoteDesktopCheck {
    fn name(&self) -> &str {
        "RemoteDesktop portal"
    }

    fn evaluate(&self) -> Finding {
        let caps = match &self.caps {
            Ok(caps) => caps,
            Err(error) => {
                return Finding::fail(
                    format!("not available ({error})"),
                    "Install xdg-desktop-portal and your desktop's backend (xdg-desktop-portal-gnome).",
                );
            }
        };
        if caps.supports_touchscreen() {
            return Finding::pass(format!("version {}, touchscreen input", caps.version));
        }
        if caps.supports_pointer() {
            return Finding::warn(
                format!("version {}, pointer only", caps.version),
                "Touch will be emulated with the mouse pointer; multi-touch gestures will not work.",
            );
        }
        Finding::fail(
            format!("version {}, no pointer or touch input", caps.version),
            "Update your portal backend.",
        )
    }
}

/// At least one working H.264 encoder.
pub struct EncoderCheck {
    pub reports: Vec<ProbeReport>,
}

impl Check for EncoderCheck {
    fn name(&self) -> &str {
        "H.264 encoder"
    }

    fn evaluate(&self) -> Finding {
        let working: Vec<_> = self
            .reports
            .iter()
            .filter(|r| r.outcome.is_usable())
            .collect();
        // Same rule as select_encoder: fully working first, then limited.
        let best = working
            .iter()
            .find(|r| r.outcome == ProbeOutcome::Works)
            .or(working.first());
        let Some(best) = best else {
            return Finding::fail(
                "none working",
                "Install your GPU's VA-API driver or NVIDIA's driver, or a software encoder \
                 (x264 / OpenH264). See docs/setup.md.",
            );
        };
        let names: Vec<_> = working.iter().map(|r| r.spec.element.as_str()).collect();
        let summary = format!(
            "{} via {} (working: {})",
            best.spec.element,
            best.spec.backend,
            names.join(", ")
        );
        if let ProbeOutcome::Limited(limits) = &best.outcome {
            Finding::warn(
                summary,
                format!(
                    "The best encoder ignores some low-latency settings ({limits}); \
                     bitrate may vary. Try --encoder with another one from the list below."
                ),
            )
        } else if best.spec.hardware {
            Finding::pass(summary)
        } else {
            Finding::warn(
                summary,
                "Only software encoding works. Expect high CPU use; install your GPU's \
                 hardware video driver. See docs/setup.md.",
            )
        }
    }
}

/// mDNS for finding the tablet on Wi-Fi without typing addresses.
pub struct DiscoveryCheck {
    /// Whether `org.freedesktop.Avahi` is on the system bus.
    pub avahi_running: Result<bool, String>,
}

impl Check for DiscoveryCheck {
    fn name(&self) -> &str {
        "mDNS discovery"
    }

    fn evaluate(&self) -> Finding {
        match &self.avahi_running {
            Ok(true) => Finding::pass("Avahi running"),
            Ok(false) => Finding::warn(
                "Avahi not running",
                "Start it with `systemctl enable --now avahi-daemon`, or connect by IP address.",
            ),
            Err(error) => Finding::warn(
                format!("could not check ({error})"),
                "Connect by IP address if the tablet is not found automatically.",
            ),
        }
    }
}

/// Sound on the tablet needs `pactl` to create its virtual speaker.
pub struct SoundCheck {
    /// What `pactl info` reported: the sound server's name, or why it failed.
    pub server: Result<String, String>,
}

impl Check for SoundCheck {
    fn name(&self) -> &str {
        "Sound on the tablet"
    }

    fn evaluate(&self) -> Finding {
        match &self.server {
            Ok(server) => Finding::pass(format!("pactl works ({server})")),
            Err(error) => Finding::warn(
                format!("pactl unavailable ({error})"),
                "Install pactl (pulseaudio-utils on most distributions) to hear the computer \
                 on the tablet. The picture works without it.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::EncoderSpec;
    use crate::portal::{device_type, source_type};

    #[test]
    fn sound_without_pactl_is_a_warning_not_a_failure() {
        let ok = SoundCheck {
            server: Ok("PulseAudio (on PipeWire 1.6.9)".into()),
        };
        assert_eq!(ok.evaluate().status, Status::Pass);
        let missing = SoundCheck {
            server: Err("not installed".into()),
        };
        assert_eq!(missing.evaluate().status, Status::Warn);
    }

    fn session(session_type: &str, desktop: &str) -> Finding {
        SessionCheck {
            session_type: Some(session_type.into()),
            desktop: Some(desktop.into()),
        }
        .evaluate()
    }

    #[test]
    fn x11_session_fails() {
        assert_eq!(session("x11", "GNOME").status, Status::Fail);
    }

    #[test]
    fn gnome_wayland_passes_including_ubuntu_style_desktop_names() {
        assert_eq!(session("wayland", "GNOME").status, Status::Pass);
        assert_eq!(session("wayland", "ubuntu:GNOME").status, Status::Pass);
    }

    #[test]
    fn other_wayland_desktops_warn() {
        assert_eq!(session("wayland", "KDE").status, Status::Warn);
    }

    #[test]
    fn missing_session_variables_fail() {
        let finding = SessionCheck {
            session_type: None,
            desktop: None,
        }
        .evaluate();
        assert_eq!(finding.status, Status::Fail);
    }

    fn screencast(version: u32, source_types: u32) -> Finding {
        ScreenCastCheck {
            caps: Ok(ScreenCastCaps {
                version,
                source_types,
            }),
        }
        .evaluate()
    }

    #[test]
    fn screencast_without_virtual_fails() {
        assert_eq!(screencast(5, source_type::MONITOR).status, Status::Fail);
    }

    #[test]
    fn screencast_without_restore_tokens_warns() {
        assert_eq!(screencast(3, source_type::VIRTUAL).status, Status::Warn);
    }

    #[test]
    fn screencast_with_virtual_and_tokens_passes() {
        assert_eq!(screencast(5, source_type::VIRTUAL).status, Status::Pass);
    }

    #[test]
    fn unreachable_portal_fails() {
        let finding = ScreenCastCheck {
            caps: Err("no such interface".into()),
        }
        .evaluate();
        assert_eq!(finding.status, Status::Fail);
        assert!(finding.summary.contains("no such interface"));
    }

    #[test]
    fn remote_desktop_pointer_only_warns() {
        let finding = RemoteDesktopCheck {
            caps: Ok(RemoteDesktopCaps {
                version: 2,
                device_types: device_type::POINTER,
            }),
        }
        .evaluate();
        assert_eq!(finding.status, Status::Warn);
    }

    fn report(element: &str, hardware: bool, outcome: ProbeOutcome) -> ProbeReport {
        ProbeReport {
            spec: EncoderSpec::new(element, "test", hardware),
            outcome,
        }
    }

    #[test]
    fn hardware_encoder_passes_and_lists_all_working() {
        let finding = EncoderCheck {
            reports: vec![
                report("vah264enc", true, ProbeOutcome::Works),
                report("nvh264enc", true, ProbeOutcome::Works),
                report("x264enc", false, ProbeOutcome::Missing),
            ],
        }
        .evaluate();
        assert_eq!(finding.status, Status::Pass);
        assert!(finding.summary.starts_with("vah264enc"));
        assert!(finding.summary.contains("vah264enc, nvh264enc"));
    }

    #[test]
    fn fully_working_encoder_is_preferred_over_a_limited_one_listed_first() {
        let finding = EncoderCheck {
            reports: vec![
                report(
                    "vah264lpenc",
                    true,
                    ProbeOutcome::Limited("cqp only".into()),
                ),
                report("vah264enc", true, ProbeOutcome::Works),
            ],
        }
        .evaluate();
        assert_eq!(finding.status, Status::Pass);
        assert!(finding.summary.starts_with("vah264enc"));
        assert!(finding.summary.contains("vah264lpenc, vah264enc"));
    }

    #[test]
    fn only_limited_encoders_warn() {
        let finding = EncoderCheck {
            reports: vec![report(
                "vah264lpenc",
                true,
                ProbeOutcome::Limited("cqp only".into()),
            )],
        }
        .evaluate();
        assert_eq!(finding.status, Status::Warn);
        assert!(finding.hint.unwrap().contains("cqp only"));
    }

    #[test]
    fn software_only_encoder_warns() {
        let finding = EncoderCheck {
            reports: vec![
                report("vah264enc", true, ProbeOutcome::Failed("no codec".into())),
                report("openh264enc", false, ProbeOutcome::Works),
            ],
        }
        .evaluate();
        assert_eq!(finding.status, Status::Warn);
    }

    #[test]
    fn no_encoder_fails() {
        let finding = EncoderCheck {
            reports: vec![report("vah264enc", true, ProbeOutcome::Missing)],
        }
        .evaluate();
        assert_eq!(finding.status, Status::Fail);
    }

    #[test]
    fn missing_avahi_only_warns() {
        let finding = DiscoveryCheck {
            avahi_running: Ok(false),
        }
        .evaluate();
        assert_eq!(finding.status, Status::Warn);
    }

    struct Fixed(Finding);

    impl Check for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn evaluate(&self) -> Finding {
            self.0.clone()
        }
    }

    #[test]
    fn report_takes_the_worst_status() {
        let checks: Vec<Box<dyn Check>> = vec![
            Box::new(Fixed(Finding::pass("ok"))),
            Box::new(Fixed(Finding::fail("bad", "fix it"))),
            Box::new(Fixed(Finding::warn("meh", "maybe"))),
        ];
        let report = Report::run(&checks);
        assert_eq!(report.results.len(), 3);
        assert_eq!(report.worst(), Status::Fail);
    }

    #[test]
    fn empty_report_passes() {
        assert_eq!(Report::run(&[]).worst(), Status::Pass);
    }
}
