//! `tabextend doctor`: check this machine can run TabExtend.

use std::env;

use tabextend_core::check::{
    Check, DiscoveryCheck, EncoderCheck, RemoteDesktopCheck, Report, ScreenCastCheck, SessionCheck,
    SoundCheck, Status,
};
use tabextend_core::encoder::{EncoderSpec, ProbeReport, default_h264_encoders, probe_all};

use crate::portals;
use crate::probe::GstProbe;

/// Test-encode size: large enough to hit hardware limits a tiny frame would not.
const PROBE_WIDTH: u32 = 1280;
const PROBE_HEIGHT: u32 = 800;

/// Run every check and probe every encoder. Takes a few seconds (test
/// encodes); call it off the UI thread.
pub fn collect(extra_encoder: Option<&str>) -> (Report, Vec<ProbeReport>) {
    let mut candidates = default_h264_encoders();
    if let Some(element) = extra_encoder
        && !candidates.iter().any(|spec| spec.element == element)
    {
        candidates.insert(0, EncoderSpec::custom(element));
    }

    let encoder_reports = match GstProbe::new(PROBE_WIDTH, PROBE_HEIGHT) {
        Ok(probe) => probe_all(&probe, &candidates),
        Err(error) => {
            eprintln!("GStreamer failed to initialise: {error}");
            Vec::new()
        }
    };

    let checks: Vec<Box<dyn Check>> = vec![
        Box::new(SessionCheck {
            session_type: env::var("XDG_SESSION_TYPE").ok(),
            desktop: env::var("XDG_CURRENT_DESKTOP").ok(),
        }),
        Box::new(ScreenCastCheck {
            caps: portals::screencast_caps(),
        }),
        Box::new(RemoteDesktopCheck {
            caps: portals::remote_desktop_caps(),
        }),
        Box::new(EncoderCheck {
            reports: encoder_reports.clone(),
        }),
        Box::new(DiscoveryCheck {
            avahi_running: portals::avahi_running(),
        }),
        Box::new(SoundCheck {
            server: sound_server(),
        }),
    ];

    (Report::run(&checks), encoder_reports)
}

/// `tabextend doctor`: check, print, and return the worst status.
pub fn run(extra_encoder: Option<&str>) -> Status {
    let (report, encoder_reports) = collect(extra_encoder);
    print_report(&report);
    print_encoders(&encoder_reports);
    report.worst()
}

fn label(status: Status) -> &'static str {
    match status {
        Status::Pass => "PASS",
        Status::Warn => "WARN",
        Status::Fail => "FAIL",
    }
}

fn print_report(report: &Report) {
    for (name, finding) in &report.results {
        println!("[{}] {name}: {}", label(finding.status), finding.summary);
        if let Some(hint) = &finding.hint {
            println!("       {hint}");
        }
    }
    println!();
    println!(
        "{}",
        match report.worst() {
            Status::Pass => "This machine is ready for TabExtend.",
            Status::Warn => "TabExtend will run, with the warnings above.",
            Status::Fail => "TabExtend cannot run until the failures above are fixed.",
        }
    );
}

fn print_encoders(reports: &[ProbeReport]) {
    use tabextend_core::encoder::ProbeOutcome;
    println!();
    println!("Encoders, in the order TabExtend tries them:");
    for report in reports {
        let outcome = match &report.outcome {
            ProbeOutcome::Works => "works".to_owned(),
            ProbeOutcome::Limited(limits) => format!("works, ignores: {limits}"),
            ProbeOutcome::Missing => "not installed".to_owned(),
            ProbeOutcome::Failed(reason) => format!("failed: {reason}"),
        };
        println!(
            "  {:<14} {:<26} {outcome}",
            report.spec.element, report.spec.backend
        );
    }
}

/// The sound server as `pactl info` names it.
fn sound_server() -> Result<String, String> {
    let output = std::process::Command::new("pactl")
        .arg("info")
        .output()
        .map_err(|_| "not installed".to_owned())?;
    if !output.status.success() {
        return Err("no sound server answered".into());
    }
    let info = String::from_utf8_lossy(&output.stdout);
    Ok(info
        .lines()
        .find_map(|line| line.strip_prefix("Server Name: "))
        .unwrap_or("unknown server")
        .trim()
        .to_owned())
}
