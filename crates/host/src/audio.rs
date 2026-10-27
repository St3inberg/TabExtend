//! The laptop's sound, captured for the tablet.

use gst::prelude::*;
use gstreamer as gst;
use tabextend_core::protocol::{AUDIO_CHANNELS, AUDIO_RATE, MAX_AUDIO_CHUNK};

/// Captures from a source fragment and hands out PCM chunks in the
/// protocol's format (48 kHz stereo S16LE).
pub struct AudioPipeline {
    pipeline: gst::Element,
}

impl AudioPipeline {
    /// `on_chunk` runs on GStreamer's streaming thread with
    /// (timestamp in µs, PCM bytes), each at most `MAX_AUDIO_CHUNK` long.
    pub fn start(
        source: &str,
        on_chunk: impl Fn(u64, Vec<u8>) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let description = format!(
            "{source} ! audioconvert ! audioresample \
             ! audio/x-raw,format=S16LE,rate={AUDIO_RATE},channels={AUDIO_CHANNELS},layout=interleaved \
             ! appsink name=sink sync=false max-buffers=8 drop=true"
        );
        let pipeline = gst::parse::launch(&description).map_err(|e| e.to_string())?;
        let sink = pipeline
            .downcast_ref::<gst::Bin>()
            .and_then(|bin| bin.by_name("sink"))
            .ok_or("appsink missing")?;
        sink.set_property("emit-signals", true);
        sink.connect("new-sample", false, move |args| {
            let flow = (|| {
                let sink = args[0].get::<gst::Element>().ok()?;
                let sample = sink.emit_by_name::<Option<gst::Sample>>("pull-sample", &[])?;
                let buffer = sample.buffer()?;
                let map = buffer.map_readable().ok()?;
                let pts = buffer.pts().map_or(0, |t| t.useconds());
                for chunk in map.as_slice().chunks(MAX_AUDIO_CHUNK) {
                    on_chunk(pts, chunk.to_vec());
                }
                Some(gst::FlowReturn::Ok)
            })()
            .unwrap_or(gst::FlowReturn::Error);
            Some(flow.to_value())
        });
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "audio pipeline failed to start".to_owned())?;
        Ok(Self { pipeline })
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
