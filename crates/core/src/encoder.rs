//! Choosing an H.264 encoder at runtime.
//!
//! TabExtend never assumes a GPU vendor. It walks a priority list of known
//! GStreamer encoder elements, asks an [`EncoderProbe`] whether each one
//! actually works on this machine, and picks the first that does. The user
//! can name any element instead, including ones not on the list.

use std::fmt;

/// A GStreamer encoder element TabExtend knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderSpec {
    /// GStreamer element name, e.g. `vah264enc`.
    pub element: String,
    /// Human-readable backend, e.g. "VA-API (Intel/AMD)".
    pub backend: String,
    /// Whether the encoder runs on dedicated hardware.
    pub hardware: bool,
}

impl EncoderSpec {
    pub fn new(element: &str, backend: &str, hardware: bool) -> Self {
        Self {
            element: element.to_owned(),
            backend: backend.to_owned(),
            hardware,
        }
    }

    /// A spec for an element the user named that is not on the built-in list.
    pub fn custom(element: &str) -> Self {
        Self::new(element, "user-specified", false)
    }
}

/// Built-in H.264 encoders, best first.
///
/// Low-power VA-API comes before regular VA-API because it uses a fixed-function
/// block with lower latency and power. Hardware encoders come before software.
pub fn default_h264_encoders() -> Vec<EncoderSpec> {
    vec![
        EncoderSpec::new("vah264lpenc", "VA-API low-power (Intel)", true),
        EncoderSpec::new("vah264enc", "VA-API (Intel/AMD)", true),
        EncoderSpec::new("nvh264enc", "NVENC (NVIDIA)", true),
        EncoderSpec::new("qsvh264enc", "Quick Sync (Intel)", true),
        EncoderSpec::new("x264enc", "x264 (software)", false),
        EncoderSpec::new("openh264enc", "OpenH264 (software)", false),
    ]
}

/// The result of trying one encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The element is not installed.
    Missing,
    /// The element exists but could not encode a test stream.
    Failed(String),
    /// The element encoded a test stream.
    Works,
}

/// Something that can tell whether an encoder works on this machine.
///
/// The host implements this with a real GStreamer pipeline; tests use fakes.
pub trait EncoderProbe {
    fn probe(&self, element: &str) -> ProbeOutcome;
}

/// One encoder and how its probe went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub spec: EncoderSpec,
    pub outcome: ProbeOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// The encoder the user asked for does not work.
    PreferredUnavailable(ProbeReport),
    /// None of the candidates work.
    NoneAvailable(Vec<ProbeReport>),
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectError::PreferredUnavailable(report) => match &report.outcome {
                ProbeOutcome::Missing => {
                    write!(f, "encoder `{}` is not installed", report.spec.element)
                }
                ProbeOutcome::Failed(reason) => {
                    write!(f, "encoder `{}` failed: {reason}", report.spec.element)
                }
                ProbeOutcome::Works => unreachable!("a working encoder is not an error"),
            },
            SelectError::NoneAvailable(_) => write!(f, "no working H.264 encoder found"),
        }
    }
}

impl std::error::Error for SelectError {}

/// Probe every candidate, in order. Used by `tabextend doctor`.
pub fn probe_all(probe: &dyn EncoderProbe, candidates: &[EncoderSpec]) -> Vec<ProbeReport> {
    candidates
        .iter()
        .map(|spec| ProbeReport {
            spec: spec.clone(),
            outcome: probe.probe(&spec.element),
        })
        .collect()
}

/// Pick an encoder.
///
/// With `preferred`, only that element is tried and there is no silent
/// fallback: if the user asked for NVENC and it is broken, they should know.
/// Without it, candidates are tried in order and probing stops at the first
/// one that works.
pub fn select_encoder(
    probe: &dyn EncoderProbe,
    candidates: &[EncoderSpec],
    preferred: Option<&str>,
) -> Result<EncoderSpec, SelectError> {
    if let Some(element) = preferred {
        let spec = candidates
            .iter()
            .find(|spec| spec.element == element)
            .cloned()
            .unwrap_or_else(|| EncoderSpec::custom(element));
        return match probe.probe(element) {
            ProbeOutcome::Works => Ok(spec),
            outcome => Err(SelectError::PreferredUnavailable(ProbeReport {
                spec,
                outcome,
            })),
        };
    }

    let mut tried = Vec::new();
    for spec in candidates {
        let outcome = probe.probe(&spec.element);
        if outcome == ProbeOutcome::Works {
            return Ok(spec.clone());
        }
        tried.push(ProbeReport {
            spec: spec.clone(),
            outcome,
        });
    }
    Err(SelectError::NoneAvailable(tried))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A probe with canned answers that records which elements it was asked about.
    struct FakeProbe {
        answers: HashMap<&'static str, ProbeOutcome>,
        asked: RefCell<Vec<String>>,
    }

    impl FakeProbe {
        fn new(answers: &[(&'static str, ProbeOutcome)]) -> Self {
            Self {
                answers: answers.iter().cloned().collect(),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl EncoderProbe for FakeProbe {
        fn probe(&self, element: &str) -> ProbeOutcome {
            self.asked.borrow_mut().push(element.to_owned());
            self.answers
                .get(element)
                .cloned()
                .unwrap_or(ProbeOutcome::Missing)
        }
    }

    #[test]
    fn picks_first_working_encoder_in_priority_order() {
        let probe = FakeProbe::new(&[
            ("nvh264enc", ProbeOutcome::Works),
            ("x264enc", ProbeOutcome::Works),
        ]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), None).unwrap();
        assert_eq!(chosen.element, "nvh264enc");
    }

    #[test]
    fn stops_probing_after_first_working_encoder() {
        let probe = FakeProbe::new(&[("vah264enc", ProbeOutcome::Works)]);
        select_encoder(&probe, &default_h264_encoders(), None).unwrap();
        assert_eq!(probe.asked(), ["vah264lpenc", "vah264enc"]);
    }

    #[test]
    fn skips_installed_but_broken_encoders() {
        // e.g. Fedora's codec-stripped Intel driver: element present, encode fails.
        let probe = FakeProbe::new(&[
            ("vah264enc", ProbeOutcome::Failed("not negotiated".into())),
            ("openh264enc", ProbeOutcome::Works),
        ]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), None).unwrap();
        assert_eq!(chosen.element, "openh264enc");
        assert!(!chosen.hardware);
    }

    #[test]
    fn reports_every_candidate_when_none_work() {
        let probe = FakeProbe::new(&[]);
        let candidates = default_h264_encoders();
        let err = select_encoder(&probe, &candidates, None).unwrap_err();
        let SelectError::NoneAvailable(reports) = err else {
            panic!("expected NoneAvailable, got {err:?}");
        };
        assert_eq!(reports.len(), candidates.len());
        assert!(reports.iter().all(|r| r.outcome == ProbeOutcome::Missing));
    }

    #[test]
    fn preferred_encoder_wins_over_priority_order() {
        let probe = FakeProbe::new(&[
            ("vah264lpenc", ProbeOutcome::Works),
            ("x264enc", ProbeOutcome::Works),
        ]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), Some("x264enc")).unwrap();
        assert_eq!(chosen.element, "x264enc");
        assert_eq!(probe.asked(), ["x264enc"]);
    }

    #[test]
    fn broken_preferred_encoder_is_an_error_not_a_fallback() {
        let probe = FakeProbe::new(&[
            ("nvh264enc", ProbeOutcome::Failed("no CUDA device".into())),
            ("vah264enc", ProbeOutcome::Works),
        ]);
        let err = select_encoder(&probe, &default_h264_encoders(), Some("nvh264enc")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "encoder `nvh264enc` failed: no CUDA device"
        );
    }

    #[test]
    fn accepts_encoders_not_on_the_built_in_list() {
        let probe = FakeProbe::new(&[("v4l2h264enc", ProbeOutcome::Works)]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), Some("v4l2h264enc")).unwrap();
        assert_eq!(chosen, EncoderSpec::custom("v4l2h264enc"));
    }

    #[test]
    fn probe_all_tries_every_candidate() {
        let probe = FakeProbe::new(&[("vah264lpenc", ProbeOutcome::Works)]);
        let reports = probe_all(&probe, &default_h264_encoders());
        assert_eq!(reports.len(), default_h264_encoders().len());
        assert_eq!(reports[0].outcome, ProbeOutcome::Works);
    }

    #[test]
    fn hardware_encoders_come_before_software() {
        let list = default_h264_encoders();
        let first_software = list.iter().position(|s| !s.hardware).unwrap();
        assert!(list[first_software..].iter().all(|s| !s.hardware));
    }
}
