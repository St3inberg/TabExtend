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
    /// Elements to try, in order, for converting captured frames into what the
    /// encoder takes. The first one installed is used; `videoconvert` is the
    /// CPU fallback that always works.
    pub converters: Vec<String>,
    /// Fixed low-latency settings as (property, value). Properties the
    /// installed element does not have are skipped.
    pub tuning: Vec<(String, String)>,
    /// Bitrate property and how many bits per second one unit of it is.
    pub bitrate: Option<(String, u32)>,
    /// Property for the maximum distance between keyframes, in frames.
    pub keyframe_interval: Option<String>,
}

const CPU_CONVERTER: &str = "videoconvert";

impl EncoderSpec {
    pub fn new(element: &str, backend: &str, hardware: bool) -> Self {
        Self {
            element: element.to_owned(),
            backend: backend.to_owned(),
            hardware,
            converters: vec![CPU_CONVERTER.to_owned()],
            tuning: Vec::new(),
            bitrate: None,
            keyframe_interval: None,
        }
    }

    /// Prefer this converter (a gst-launch fragment) over the CPU fallback.
    fn converter(mut self, fragment: &str) -> Self {
        self.converters
            .insert(self.converters.len() - 1, fragment.to_owned());
        self
    }

    fn tune(mut self, settings: &[(&str, &str)]) -> Self {
        self.tuning.extend(
            settings
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        self
    }

    fn bitrate_property(mut self, name: &str, bits_per_unit: u32) -> Self {
        self.bitrate = Some((name.to_owned(), bits_per_unit));
        self
    }

    fn keyframe_property(mut self, name: &str) -> Self {
        self.keyframe_interval = Some(name.to_owned());
        self
    }

    /// Every property to set for a stream, as (property, value).
    pub fn settings(&self, bitrate_bps: u32, keyframe_interval: u32) -> Vec<(String, String)> {
        let mut settings = self.tuning.clone();
        if let Some((name, bits_per_unit)) = &self.bitrate {
            settings.push((
                name.clone(),
                (bitrate_bps / bits_per_unit).max(1).to_string(),
            ));
        }
        if let Some(name) = &self.keyframe_interval {
            settings.push((name.clone(), keyframe_interval.to_string()));
        }
        settings
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
///
/// Settings favour latency over compression: no B-frames (they make the
/// decoder wait for later frames), one reference frame (so decoders that
/// size their buffer from the stream hold almost nothing), constant
/// bitrate, fastest presets.
pub fn default_h264_encoders() -> Vec<EncoderSpec> {
    let va = |element: &str, backend: &str| {
        EncoderSpec::new(element, backend, true)
            .converter("vapostproc")
            .tune(&[
                ("rate-control", "cbr"),
                ("b-frames", "0"),
                ("ref-frames", "1"),
                ("target-usage", "7"),
            ])
            .bitrate_property("bitrate", 1000)
            .keyframe_property("key-int-max")
    };
    vec![
        va("vah264lpenc", "VA-API low-power (Intel)"),
        va("vah264enc", "VA-API (Intel/AMD)"),
        EncoderSpec::new("nvh264enc", "NVENC (NVIDIA)", true)
            .converter("cudaupload ! cudaconvert")
            .tune(&[
                ("tune", "ultra-low-latency"),
                ("zerolatency", "true"),
                ("bframes", "0"),
                ("rc-mode", "cbr"),
            ])
            .bitrate_property("bitrate", 1000)
            .keyframe_property("gop-size"),
        EncoderSpec::new("qsvh264enc", "Quick Sync (Intel)", true)
            .tune(&[
                ("b-frames", "0"),
                ("ref-frames", "1"),
                ("target-usage", "7"),
                ("rate-control", "cbr"),
            ])
            .bitrate_property("bitrate", 1000)
            .keyframe_property("gop-size"),
        EncoderSpec::new("x264enc", "x264 (software)", false)
            .tune(&[
                ("tune", "zerolatency"),
                ("speed-preset", "ultrafast"),
                ("bframes", "0"),
                ("ref", "1"),
            ])
            .bitrate_property("bitrate", 1000)
            .keyframe_property("key-int-max"),
        EncoderSpec::new("openh264enc", "OpenH264 (software)", false)
            .tune(&[("complexity", "low"), ("rate-control", "bitrate")])
            .bitrate_property("bitrate", 1)
            .keyframe_property("gop-size"),
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
    /// It encoded, but rejected some of its low-latency settings (e.g. a
    /// driver that has no constant-bitrate mode). Usable, ranked after
    /// encoders that took every setting.
    Limited(String),
}

impl ProbeOutcome {
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Works | Self::Limited(_))
    }
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
    PreferredUnavailable(Box<ProbeReport>),
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
                ProbeOutcome::Works | ProbeOutcome::Limited(_) => {
                    unreachable!("a usable encoder is not an error")
                }
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
/// one that fully works; if none does, the first limited one is used.
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
            outcome if outcome.is_usable() => Ok(spec),
            outcome => Err(SelectError::PreferredUnavailable(Box::new(ProbeReport {
                spec,
                outcome,
            }))),
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
    match tried.iter().find(|r| r.outcome.is_usable()) {
        Some(limited) => Ok(limited.spec.clone()),
        None => Err(SelectError::NoneAvailable(tried)),
    }
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
    fn fully_working_encoder_beats_a_limited_one_ranked_higher() {
        let probe = FakeProbe::new(&[
            (
                "vah264lpenc",
                ProbeOutcome::Limited("rate-control=cbr rejected".into()),
            ),
            ("vah264enc", ProbeOutcome::Works),
        ]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), None).unwrap();
        assert_eq!(chosen.element, "vah264enc");
    }

    #[test]
    fn limited_encoder_is_used_when_nothing_fully_works() {
        let probe = FakeProbe::new(&[
            ("nvh264enc", ProbeOutcome::Limited("tune rejected".into())),
            (
                "openh264enc",
                ProbeOutcome::Limited("complexity rejected".into()),
            ),
        ]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), None).unwrap();
        assert_eq!(chosen.element, "nvh264enc");
    }

    #[test]
    fn limited_preferred_encoder_is_accepted() {
        let probe = FakeProbe::new(&[("vah264lpenc", ProbeOutcome::Limited("cqp only".into()))]);
        let chosen = select_encoder(&probe, &default_h264_encoders(), Some("vah264lpenc")).unwrap();
        assert_eq!(chosen.element, "vah264lpenc");
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
    fn bitrate_is_converted_to_each_encoders_unit() {
        let encoders = default_h264_encoders();
        let find = |name: &str| encoders.iter().find(|s| s.element == name).unwrap();
        let value = |spec: &EncoderSpec, key: &str| {
            spec.settings(20_000_000, 120)
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
        };
        assert_eq!(
            value(find("vah264enc"), "bitrate").as_deref(),
            Some("20000")
        );
        assert_eq!(
            value(find("openh264enc"), "bitrate").as_deref(),
            Some("20000000")
        );
        assert_eq!(value(find("nvh264enc"), "gop-size").as_deref(), Some("120"));
    }

    #[test]
    fn every_encoder_disables_b_frames_or_is_low_latency_by_design() {
        // openh264 has no B-frames at all.
        for spec in default_h264_encoders() {
            let has = |k: &str| spec.tuning.iter().any(|(key, _)| key == k);
            assert!(
                has("b-frames") || has("bframes") || spec.element == "openh264enc",
                "{} may produce B-frames",
                spec.element
            );
        }
    }

    #[test]
    fn cpu_converter_is_always_the_last_resort() {
        for spec in default_h264_encoders() {
            assert_eq!(
                spec.converters.last().map(String::as_str),
                Some(CPU_CONVERTER)
            );
        }
        let va = &default_h264_encoders()[0];
        assert_eq!(va.converters, ["vapostproc", "videoconvert"]);
    }

    #[test]
    fn custom_encoders_get_no_guessed_settings() {
        let spec = EncoderSpec::custom("v4l2h264enc");
        assert!(spec.settings(10_000_000, 60).is_empty());
        assert_eq!(spec.converters, ["videoconvert"]);
    }

    #[test]
    fn hardware_encoders_come_before_software() {
        let list = default_h264_encoders();
        let first_software = list.iter().position(|s| !s.hardware).unwrap();
        assert!(list[first_software..].iter().all(|s| !s.hardware));
    }
}
