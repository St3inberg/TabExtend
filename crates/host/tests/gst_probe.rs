//! Probing against the real GStreamer installed on this machine.

use tabextend_core::encoder::{EncoderProbe, ProbeOutcome};
use tabextend_host::probe::GstProbe;

fn probe() -> GstProbe {
    GstProbe::new(320, 240).expect("GStreamer initialises")
}

#[test]
fn unknown_element_is_missing() {
    assert_eq!(
        probe().probe("tabextend-no-such-encoder"),
        ProbeOutcome::Missing
    );
}

#[test]
fn element_that_cannot_take_video_fails_instead_of_hanging() {
    // audioconvert exists everywhere GStreamer does, but rejects video caps.
    let outcome = probe().probe("audioconvert");
    assert!(
        matches!(outcome, ProbeOutcome::Failed(_)),
        "expected a failure, got {outcome:?}"
    );
}

#[test]
fn pass_through_element_completes_the_test_pipeline() {
    // identity is in GStreamer core, so this checks the pipeline itself
    // without depending on which encoders a machine has.
    assert_eq!(probe().probe("identity"), ProbeOutcome::Works);
}
