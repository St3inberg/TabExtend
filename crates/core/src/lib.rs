//! Platform-independent logic for TabExtend.
//!
//! Nothing here talks to GStreamer, D-Bus or the network, so it builds and
//! tests on any machine. The `tabextend-host` crate supplies the real I/O.

pub mod check;
pub mod encoder;
pub mod portal;
