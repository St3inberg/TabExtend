//! Encoder probing with real GStreamer pipelines.

use gst::prelude::*;
use gstreamer as gst;
use tabextend_core::encoder::{EncoderProbe, ProbeOutcome};

/// How long one test encode may take before it counts as failed.
const PROBE_TIMEOUT_SECS: u64 = 10;

/// Probes an encoder by encoding a few frames of a test pattern.
///
/// Checking only that the element exists is not enough: Fedora ships
/// `vah264enc` with an Intel driver that cannot encode H.264, and `nvh264enc`
/// is present on machines without an NVIDIA GPU.
pub struct GstProbe {
    width: u32,
    height: u32,
}

impl GstProbe {
    pub fn new(width: u32, height: u32) -> Result<Self, gst::glib::Error> {
        gst::init()?;
        Ok(Self { width, height })
    }

    fn run_test_encode(&self, element: &str) -> Result<(), String> {
        let description = format!(
            "videotestsrc num-buffers=10 ! video/x-raw,width={},height={},framerate=30/1 \
             ! videoconvert ! {element} ! fakesink",
            self.width, self.height
        );
        let pipeline = gst::parse::launch(&description).map_err(|e| e.to_string())?;
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
        result
    }
}

impl EncoderProbe for GstProbe {
    fn probe(&self, element: &str) -> ProbeOutcome {
        if gst::ElementFactory::find(element).is_none() {
            return ProbeOutcome::Missing;
        }
        match self.run_test_encode(element) {
            Ok(()) => ProbeOutcome::Works,
            Err(reason) => ProbeOutcome::Failed(reason),
        }
    }
}
