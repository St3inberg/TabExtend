//! The tablet's virtual speaker against the real sound server on this
//! machine. Ignored by default (CI has none): `cargo test -p tabextend-host
//! --test pipewire_sound -- --ignored`.

use std::process::Command;
use std::time::{Duration, Instant};

use tabextend_host::audio::{AudioPipeline, SoundSource};

fn speaker_exists() -> bool {
    let out = Command::new("pactl")
        .args(["list", "sinks", "short"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains("tabextend-audio")
}

fn speaker() -> SoundSource {
    SoundSource::VirtualSpeaker {
        label: "Test Tablet".into(),
    }
}

#[test]
#[ignore = "needs a running PipeWire or PulseAudio"]
fn speaker_starts_at_once_and_is_removed_after() {
    for round in 0..3 {
        let started = Instant::now();
        let pipeline = AudioPipeline::start(&speaker(), |_, _| {})
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        // It used to wait 30 s for someone to play to it, then fail.
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "round {round}: slow start"
        );
        assert!(
            speaker_exists(),
            "round {round}: speaker missing while connected"
        );
        std::thread::sleep(Duration::from_millis(300));
        drop(pipeline);
        assert!(!speaker_exists(), "round {round}: speaker left behind");
    }
}

#[test]
#[ignore = "needs a running PipeWire or PulseAudio"]
fn sound_played_to_the_speaker_arrives() {
    let (tx, rx) = std::sync::mpsc::channel();
    let _pipeline = AudioPipeline::start(&speaker(), move |_, pcm| {
        let _ = tx.send(pcm.len());
    })
    .unwrap();
    // Play a short tone straight to the speaker.
    let mut player = Command::new("gst-launch-1.0")
        .args([
            "-q",
            "audiotestsrc",
            "num-buffers=50",
            "!",
            "audioconvert",
            "!",
            "pulsesink",
            "device=tabextend-audio",
        ])
        .spawn()
        .unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5));
    let _ = player.wait();
    assert!(got.is_ok(), "no sound arrived from the speaker");
}
