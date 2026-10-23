//! `tabextend doctor`: check this machine can run TabExtend.

use std::env;

use tabextend_core::check::{
    Check, DiscoveryCheck, EncoderCheck, RemoteDesktopCheck, Report, ScreenCastCheck, SessionCheck,
    Status,
};
use tabextend_core::encoder::{EncoderSpec, default_h264_encoders, probe_all};

use crate::portals;
use crate::probe::GstProbe;

/// Test-encode size: large enough to hit hardware limits a tiny frame would not.
const PROBE_WIDTH: u32 = 1280;
const PROBE_HEIGHT: u32 = 800;

pub fn run(extra_encoder: Option<&str>) -> Status {
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
    ];

    let report = Report::run(&checks);
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

fn print_encoders(reports: &[tabextend_core::encoder::ProbeReport]) {
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
