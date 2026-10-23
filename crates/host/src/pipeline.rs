//! The GStreamer pipeline: captured frames in, H.264 access units out.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gstreamer as gst;
use tabextend_core::encoder::EncoderSpec;

/// Element name the encoder gets inside the pipeline.
const ENCODER_NAME: &str = "encoder";

/// The first of the spec's converters whose elements are all installed.
pub fn converter_for(spec: &EncoderSpec) -> &str {
    spec.converters
        .iter()
        .find(|fragment| {
            fragment
                .split('!')
                .map(|e| e.split_whitespace().next().unwrap_or(""))
                .all(|name| gst::ElementFactory::find(name).is_some())
        })
        .map_or("videoconvert", String::as_str)
}

/// Can this encoder chain take frames as GPU buffers (DMA-BUF) straight from
/// the compositor? Then no frame is copied through system memory. Only the
/// VA-API converter imports them; other chains get copied frames.
pub fn accepts_gpu_buffers(spec: &EncoderSpec) -> bool {
    converter_for(spec) == "vapostproc"
}

/// `converter ! encoder name=encoder`, for the middle of a pipeline.
pub fn encoder_fragment(spec: &EncoderSpec) -> String {
    format!(
        "{} ! {} name={ENCODER_NAME}",
        converter_for(spec),
        spec.element
    )
}

/// Set the encoder's properties, skipping ones this element version lacks.
/// Returns the settings that were skipped or rejected, for logging.
pub fn apply_settings(pipeline: &gst::Element, settings: &[(String, String)]) -> Vec<String> {
    let Some(encoder) = pipeline
        .downcast_ref::<gst::Bin>()
        .and_then(|bin| bin.by_name(ENCODER_NAME))
    else {
        return vec!["encoder element not found".into()];
    };
    let mut skipped = Vec::new();
    for (name, value) in settings {
        let Some(pspec) = encoder.find_property(name) else {
            skipped.push(format!("{name} (not supported)"));
            continue;
        };
        match gst::glib::Value::deserialize_with_pspec(value, &pspec) {
            Ok(parsed) => encoder.set_property_from_value(name, &parsed),
            Err(_) => skipped.push(format!("{name}={value} (rejected)")),
        }
    }
    skipped
}

/// One encoded access unit, Annex B, SPS/PPS included before keyframes.
pub struct EncodedFrame {
    pub pts_us: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

pub struct StreamConfig {
    pub bitrate_bps: u32,
    pub keyframe_interval: u32,
}

/// A running capture → encode pipeline.
pub struct EncodePipeline {
    pipeline: gst::Element,
    sink: gst::Element,
    captured: Arc<AtomicU64>,
}

impl EncodePipeline {
    /// Build and start. `on_frame` runs on GStreamer's streaming thread for
    /// every encoded frame; `on_error` once if the pipeline fails.
    pub fn start(
        source: &str,
        spec: &EncoderSpec,
        config: &StreamConfig,
        on_frame: impl Fn(EncodedFrame) + Send + Sync + 'static,
        on_error: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        // queue leaky=downstream: if the encoder falls behind, drop raw
        // frames rather than build up latency in front of it.
        let description = format!(
            "{source} ! queue name=capture max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream \
             ! {} ! h264parse config-interval=-1 \
             ! video/x-h264,stream-format=byte-stream,alignment=au \
             ! appsink name=sink sync=false max-buffers=2 drop=true",
            encoder_fragment(spec)
        );
        let pipeline = gst::parse::launch(&description).map_err(|e| e.to_string())?;
        for skipped in apply_settings(
            &pipeline,
            &spec.settings(config.bitrate_bps, config.keyframe_interval),
        ) {
            eprintln!("note: {}: {skipped}", spec.element);
        }

        // Count frames as they arrive from the capture source, before any
        // dropping, to tell a slow compositor from a slow encoder.
        let captured = Arc::new(AtomicU64::new(0));
        if let Some(pad) = pipeline
            .downcast_ref::<gst::Bin>()
            .and_then(|bin| bin.by_name("capture"))
            .and_then(|queue| queue.static_pad("sink"))
        {
            let captured = captured.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                captured.fetch_add(1, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            });
        }

        // appsink through its signals, so no gstreamer-app development
        // package is needed to build TabExtend.
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
                on_frame(EncodedFrame {
                    pts_us: buffer.pts().map_or(0, |t| t.useconds()),
                    keyframe: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
                    data: map.as_slice().to_vec(),
                });
                Some(gst::FlowReturn::Ok)
            })()
            .unwrap_or(gst::FlowReturn::Error);
            Some(flow.to_value())
        });

        let bus = pipeline.bus().ok_or("pipeline has no bus")?;
        let on_error = Arc::new(Mutex::new(Some(on_error)));
        bus.set_sync_handler(move |_, message| {
            if let gst::MessageView::Error(err) = message.view() {
                let text = match err.debug() {
                    Some(debug) => format!("{} ({debug})", err.error()),
                    None => err.error().to_string(),
                };
                if let Some(callback) = on_error.lock().ok().and_then(|mut f| f.take()) {
                    callback(text);
                }
            }
            gst::BusSyncReply::Drop
        });

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "pipeline failed to start".to_owned())?;
        Ok(Self {
            pipeline,
            sink,
            captured,
        })
    }

    /// Frames received from the capture source so far.
    pub fn captured_frames(&self) -> u64 {
        self.captured.load(Ordering::Relaxed)
    }

    /// Ask the encoder for a keyframe, with SPS/PPS, as soon as possible.
    pub fn request_keyframe(&self) {
        let event = gst::event::CustomUpstream::new(
            gst::Structure::builder("GstForceKeyUnit")
                .field("all-headers", true)
                .build(),
        );
        self.sink.send_event(event);
    }
}

impl Drop for EncodePipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
