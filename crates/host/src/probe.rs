//! Encoder probing with real GStreamer pipelines.

use gst::prelude::*;
use gstreamer as gst;
use tabextend_core::encoder::{EncoderProbe, EncoderSpec, ProbeOutcome, default_h264_encoders};
use tabextend_core::stream::default_bitrate;

use crate::pipeline::{apply_settings, encoder_fragment};

/// How long one test encode may take before it counts as failed.
const PROBE_TIMEOUT_SECS: u64 = 10;

/// Probes an encoder by encoding a few frames of a test pattern.
///
/// Checking only that the element exists is not enough: Fedora ships
/// `vah264enc` with an Intel driver that cannot encode H.264, and `nvh264enc`
/// is present on machines without an NVIDIA GPU.
///
/// The test uses the same converter and settings as a real stream, so a
/// setting an encoder rejects shows up here rather than mid-session.
pub struct GstProbe {
    width: u32,
    height: u32,
    known: Vec<EncoderSpec>,
}

impl GstProbe {
    pub fn new(width: u32, height: u32) -> Result<Self, gst::glib::Error> {
        gst::init()?;
        Ok(Self {
            width,
            height,
            known: default_h264_encoders(),
        })
    }

    fn spec_for(&self, element: &str) -> EncoderSpec {
        self.known
            .iter()
            .find(|spec| spec.element == element)
            .cloned()
            .unwrap_or_else(|| EncoderSpec::custom(element))
    }

    /// Encode a few frames; on success, return the settings the encoder rejected.
    fn run_test_encode(&self, element: &str) -> Result<Vec<String>, String> {
        let spec = self.spec_for(element);
        let description = format!(
            "videotestsrc num-buffers=10 ! video/x-raw,width={},height={},framerate=30/1 \
             ! {} ! fakesink",
            self.width,
            self.height,
            encoder_fragment(&spec)
        );
        let pipeline = gst::parse::launch(&description).map_err(|e| e.to_string())?;
        let bitrate = default_bitrate(self.width, self.height, 30);
        let skipped = apply_settings(&pipeline, &spec.settings(bitrate, 30));
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "could not start pipeline".to_owned())?;

        let bus = pipeline.bus().expect("a pipeline always has a bus");
        let result = match bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(PROBE_TIMEOUT_SECS),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            Some(message) => match message.view() {
                gst::MessageView::Eos(_) => Ok(()),
                gst::MessageView::Error(error) => Err(error.error().to_string()),
                _ => unreachable!("filtered to EOS and errors"),
            },
            None => Err(format!("timed out after {PROBE_TIMEOUT_SECS} s")),
        };
        let _ = pipeline.set_state(gst::State::Null);
        result.map(|()| skipped)
    }
}

impl EncoderProbe for GstProbe {
    fn probe(&self, element: &str) -> ProbeOutcome {
        if gst::ElementFactory::find(element).is_none() {
            return ProbeOutcome::Missing;
        }
        match self.run_test_encode(element) {
            Ok(skipped) if skipped.is_empty() => ProbeOutcome::Works,
            Ok(skipped) => ProbeOutcome::Limited(skipped.join(", ")),
            Err(reason) => ProbeOutcome::Failed(reason),
        }
    }
}
