//! The laptop's sound, captured for the tablet.
//!
//! While a tablet is connected, a virtual speaker named after it appears in
//! the computer's sound settings. It is a null sink (what PipeWire and
//! PulseAudio call a sound card that plays nowhere): always running, with a
//! fixed format, so it can be picked at any time. Its monitor, everything
//! played to it, is what goes to the tablet.

use std::process::Command;

use gst::prelude::*;
use gstreamer as gst;
use tabextend_core::protocol::{AUDIO_CHANNELS, AUDIO_RATE, MAX_AUDIO_CHUNK};

/// The virtual speaker's node name. Stable, so the sound server remembers
/// it if the user made it the default output.
pub const SPEAKER_NODE: &str = "tabextend-audio";

/// Where the tablet's sound comes from.
pub enum SoundSource {
    /// A GStreamer source fragment, used as is (the test pattern's tone).
    Pipeline(String),
    /// A virtual speaker on this computer; `label` is the tablet's name,
    /// already made safe.
    VirtualSpeaker { label: String },
}

/// Captures sound and hands out PCM chunks in the protocol's format
/// (48 kHz stereo S16LE).
pub struct AudioPipeline {
    pipeline: gst::Element,
    /// Removed after the pipeline stops: fields drop after `Drop::drop`.
    _speaker: Option<VirtualSpeaker>,
}

/// A null sink created with `pactl`, removed again on drop.
pub struct VirtualSpeaker {
    module: u32,
}

impl VirtualSpeaker {
    pub fn create(label: &str) -> Result<Self, String> {
        remove_stale_speakers();
        let output = Command::new("pactl")
            .args(null_sink_args(label))
            .output()
            .map_err(|e| format!("pactl is needed for sound on the tablet ({e})"))?;
        if !output.status.success() {
            return Err(format!(
                "could not create the TabExtend speaker: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let module = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .map_err(|_| "pactl did not return a module number".to_owned())?;
        Ok(Self { module })
    }

    /// Records everything played to the speaker. Never follows the
    /// default output elsewhere if the speaker goes away: that would send
    /// the laptop's other sound to the tablet.
    pub fn capture_source(&self) -> String {
        format!(
            "pipewiresrc target-object={SPEAKER_NODE} stream-properties=\"props,\
             stream.capture.sink=true,node.dont-reconnect=true,\
             node.name=tabextend-audio-capture\""
        )
    }
}

impl Drop for VirtualSpeaker {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.module.to_string()])
            .output();
    }
}

/// The `pactl` arguments that create the speaker. Low priority, so the
/// desktop never switches to it by itself: the user picks it.
pub fn null_sink_args(label: &str) -> Vec<String> {
    vec![
        "load-module".into(),
        "module-null-sink".into(),
        format!("sink_name={SPEAKER_NODE}"),
        format!("sink_properties=device.description=\"TabExtend: {label}\" priority.session=0"),
        format!("rate={AUDIO_RATE}"),
        format!("channels={AUDIO_CHANNELS}"),
        "channel_map=front-left,front-right".into(),
    ]
}

/// Remove every TabExtend speaker. Called when the host stops: the sound
/// server keeps a speaker until it is unloaded, even after its process
/// exits, so quitting must not skip this.
pub fn remove_speakers() {
    remove_stale_speakers();
}

/// A speaker left behind by a TabExtend that crashed.
fn remove_stale_speakers() {
    let Ok(output) = Command::new("pactl")
        .args(["list", "modules", "short"])
        .output()
    else {
        return;
    };
    let ours = format!("sink_name={SPEAKER_NODE}");
    for id in stale_modules(&String::from_utf8_lossy(&output.stdout), &ours) {
        let _ = Command::new("pactl").args(["unload-module", id]).output();
    }
}

/// Module ids from `pactl list modules short` that are our null sink.
fn stale_modules<'a>(listing: &'a str, ours: &str) -> Vec<&'a str> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?;
            let name = fields.next()?;
            let args = fields.next().unwrap_or("");
            (name == "module-null-sink" && args.split_whitespace().any(|a| a == ours)).then_some(id)
        })
        .collect()
}

impl AudioPipeline {
    /// `on_chunk` runs on GStreamer's streaming thread with
    /// (timestamp in µs, PCM bytes), each at most `MAX_AUDIO_CHUNK` long.
    pub fn start(
        source: &SoundSource,
        on_chunk: impl Fn(u64, Vec<u8>) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let (speaker, source) = match source {
            SoundSource::Pipeline(fragment) => (None, fragment.clone()),
            SoundSource::VirtualSpeaker { label } => {
                let speaker = VirtualSpeaker::create(label)?;
                let fragment = speaker.capture_source();
                (Some(speaker), fragment)
            }
        };
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
        crate::pipeline::play(&pipeline, "audio pipeline")?;
        Ok(Self {
            pipeline,
            _speaker: speaker,
        })
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_is_named_after_the_tablet_and_never_preferred() {
        let args = null_sink_args("Idea Tab");
        assert_eq!(args[..2], ["load-module", "module-null-sink"]);
        assert!(args.contains(&"sink_name=tabextend-audio".to_owned()));
        assert!(
            args.contains(
                &"sink_properties=device.description=\"TabExtend: Idea Tab\" priority.session=0"
                    .to_owned()
            )
        );
        assert!(args.contains(&"rate=48000".to_owned()));
        assert!(args.contains(&"channels=2".to_owned()));
    }

    #[test]
    fn only_our_null_sinks_count_as_stale() {
        let listing = "536870916\tmodule-null-sink\tsink_name=tabextend-audio rate=48000\t\n\
                       536870917\tmodule-null-sink\tsink_name=tabextend-audio-other\t\n\
                       536870918\tmodule-loopback\tsink_name=tabextend-audio\t\n\
                       12\tmodule-native-protocol-tcp\t\t\n";
        assert_eq!(
            stale_modules(listing, "sink_name=tabextend-audio"),
            ["536870916"]
        );
    }

    #[test]
    fn capture_source_parses_and_stays_on_the_speaker() {
        gst::init().unwrap();
        let speaker = VirtualSpeaker { module: 0 };
        let pipeline = gst::parse::launch(&format!("{} ! fakesink", speaker.capture_source()))
            .expect("parses");
        std::mem::forget(speaker); // module 0 is not ours to unload
        let source = pipeline
            .downcast_ref::<gst::Bin>()
            .unwrap()
            .iterate_sources()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            source
                .property::<Option<String>>("target-object")
                .as_deref(),
            Some(SPEAKER_NODE)
        );
        let props = source.property::<gst::Structure>("stream-properties");
        assert!(props.get::<bool>("node.dont-reconnect").unwrap());
        assert!(props.get::<bool>("stream.capture.sink").unwrap());
    }
}
