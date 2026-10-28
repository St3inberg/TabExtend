//! Sway, Hyprland and other wlroots desktops.
//!
//! Their portal cannot create a monitor and has no remote-desktop input, so
//! this backend talks to the compositor directly:
//! - the tablet's monitor is a headless output, made with the compositor's
//!   own command (`swaymsg create_output`, `hyprctl output create headless`)
//! - frames come from the screencopy protocol, which waits for something on
//!   the output to change, and are pushed into the encode pipeline
//! - input goes through a virtual pointer tied to that output and a virtual
//!   keyboard. wlroots has no virtual touch device, so touch is refused and
//!   the server falls back to fingers driving the pointer.

use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gst::prelude::*;
use gstreamer as gst;
use rustix::fs::{MemfdFlags, memfd_create};
use tabextend_core::input::{InputCall, ScrollAxis};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_keyboard, wl_keyboard::WlKeyboard, wl_output, wl_output::WlOutput,
    wl_pointer, wl_registry::WlRegistry, wl_seat, wl_seat::WlSeat, wl_shm, wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::audio::SoundSource;
use crate::capture::{BoxFuture, Capture, CaptureBackend, label};

/// The appsrc this backend pushes frames into.
const SOURCE_NAME: &str = "tabextend-wlroots";

/// Used when the desktop has no keyboard to copy a layout from (a headless
/// test desktop): US layout, compiled from xkeyboard-config (MIT licence)
/// with `xkbcli compile-keymap --layout us`.
const FALLBACK_KEYMAP: &[u8] = include_bytes!("keymap_us.xkb");

/// A wlroots compositor TabExtend knows how to add an output to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    Sway,
    Hyprland,
}

impl Compositor {
    /// The compositor of this session, from the variables it sets.
    pub fn detect() -> Option<Self> {
        Self::from_env(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
    }

    fn from_env(is_set: impl Fn(&str) -> bool) -> Option<Self> {
        if is_set("SWAYSOCK") {
            Some(Self::Sway)
        } else if is_set("HYPRLAND_INSTANCE_SIGNATURE") {
            Some(Self::Hyprland)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sway => "Sway",
            Self::Hyprland => "Hyprland",
        }
    }

    /// Add a headless output of this size right of the others; its name.
    fn create_output(self, width: u32, height: u32) -> Result<String, String> {
        match self {
            Self::Sway => {
                let before = sway_outputs()?;
                run("swaymsg", &["create_output"])?;
                let after = sway_outputs()?;
                let name = new_output(&before, &after)
                    .ok_or("sway did not add an output")?
                    .to_owned();
                let x = right_edge(&before);
                run(
                    "swaymsg",
                    &[
                        "output",
                        &name,
                        "mode",
                        &format!("{width}x{height}@60Hz"),
                        "pos",
                        &x.to_string(),
                        "0",
                        "scale",
                        "1",
                    ],
                )?;
                Ok(name)
            }
            Self::Hyprland => {
                let name = HYPRLAND_OUTPUT.to_owned();
                // A leftover from a crash would make the create fail.
                let _ = run("hyprctl", &["output", "remove", &name]);
                run("hyprctl", &["output", "create", "headless", &name])?;
                // Hyprland 0.57 moved to a Lua config, where `keyword` is
                // refused; older versions only have `keyword`.
                let lua = format!(
                    "hl.monitor({{ output = \"{name}\", mode = \"{width}x{height}@60\", \
                     position = \"auto-right\", scale = 1 }})"
                );
                if run("hyprctl", &["eval", &lua]).is_err() {
                    run(
                        "hyprctl",
                        &[
                            "keyword",
                            "monitor",
                            &format!("{name},{width}x{height}@60,auto-right,1"),
                        ],
                    )?;
                }
                Ok(name)
            }
        }
    }

    fn remove_output(self, name: &str) {
        let _ = match self {
            Self::Sway => run("swaymsg", &["output", name, "unplug"]),
            Self::Hyprland => run("hyprctl", &["output", "remove", name]),
        };
    }
}

const HYPRLAND_OUTPUT: &str = "TABEXTEND-1";

/// One output as `swaymsg -t get_outputs` reports it.
#[derive(Debug, Clone, PartialEq)]
struct SwayOutput {
    name: String,
    active: bool,
    x: i64,
    width: i64,
}

fn sway_outputs() -> Result<Vec<SwayOutput>, String> {
    parse_sway_outputs(&run("swaymsg", &["-t", "get_outputs", "-r"])?)
}

fn parse_sway_outputs(json: &str) -> Result<Vec<SwayOutput>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("swaymsg output: {e}"))?;
    let list = value.as_array().ok_or("swaymsg output: not a list")?;
    Ok(list
        .iter()
        .filter_map(|o| {
            Some(SwayOutput {
                name: o.get("name")?.as_str()?.to_owned(),
                active: o.get("active").and_then(|a| a.as_bool()).unwrap_or(false),
                x: o.pointer("/rect/x")?.as_i64()?,
                width: o.pointer("/rect/width")?.as_i64()?,
            })
        })
        .collect())
}

/// The output that appeared between two listings.
fn new_output<'a>(before: &[SwayOutput], after: &'a [SwayOutput]) -> Option<&'a str> {
    after
        .iter()
        .find(|o| !before.iter().any(|b| b.name == o.name))
        .map(|o| o.name.as_str())
}

/// Where a new output goes so it sits right of every active one.
fn right_edge(outputs: &[SwayOutput]) -> i64 {
    outputs
        .iter()
        .filter(|o| o.active)
        .map(|o| o.x + o.width)
        .max()
        .unwrap_or(0)
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    // swaymsg reports command failures as JSON with exit code 2; hyprctl
    // prints an error with exit code 0.
    let refused = stdout.contains("\"success\": false")
        || (program == "hyprctl"
            && !stdout.trim().is_empty()
            && stdout.trim() != "ok"
            && !args.first().is_some_and(|a| a.starts_with('-')));
    if !output.status.success() || refused {
        return Err(format!(
            "{program} {}: {}{}",
            args.join(" "),
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(stdout)
}

/// What this compositor lacks for TabExtend (empty: nothing).
pub fn missing_features(compositor: Compositor) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    let (tool, args): (&str, &[&str]) = match compositor {
        Compositor::Sway => ("swaymsg", &["-t", "get_version"]),
        Compositor::Hyprland => ("hyprctl", &["version"]),
    };
    if run(tool, args).is_err() {
        missing.push(format!("a working `{tool}`"));
    }
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, _queue) = registry_queue_init::<ProbeState>(&conn).map_err(|e| e.to_string())?;
    let offered = globals.contents().clone_list();
    let needs = [
        (
            ZwlrScreencopyManagerV1::interface().name,
            3,
            "screencopy v3",
        ),
        (
            ZwlrVirtualPointerManagerV1::interface().name,
            2,
            "virtual pointer v2",
        ),
        (
            ZwpVirtualKeyboardManagerV1::interface().name,
            1,
            "virtual keyboard",
        ),
    ];
    for (interface, version, what) in needs {
        if !offered
            .iter()
            .any(|g| g.interface == interface && g.version >= version)
        {
            missing.push(what.to_owned());
        }
    }
    Ok(missing)
}

struct ProbeState;

impl Dispatch<WlRegistry, GlobalListContents> for ProbeState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The backend: one headless output per connected tablet.
pub struct WlrootsDesktop {
    compositor: Compositor,
}

impl WlrootsDesktop {
    pub fn new(compositor: Compositor) -> Self {
        Self { compositor }
    }
}

impl CaptureBackend for WlrootsDesktop {
    fn start(&self, width: u32, height: u32) -> BoxFuture<'_, Result<Box<dyn Capture>, String>> {
        let compositor = self.compositor;
        Box::pin(async move {
            tokio::task::spawn_blocking(move || WlrootsCapture::open(compositor, width, height))
                .await
                .map_err(|e| e.to_string())?
                .map(|c| Box::new(c) as Box<dyn Capture>)
                .map_err(|e| format!("{}: {e}", compositor.name()))
        })
    }
}

struct WlrootsCapture {
    compositor: Compositor,
    output: String,
    size: (u32, u32),
    appsrc: Arc<Mutex<Option<gst::Element>>>,
    stop: Arc<AtomicBool>,
    input: Mutex<mpsc::Sender<InputCall>>,
}

impl WlrootsCapture {
    fn open(compositor: Compositor, width: u32, height: u32) -> Result<Self, String> {
        let output = compositor.create_output(width, height)?;
        let opened = (|| {
            let appsrc = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            let (ready_tx, ready_rx) = mpsc::channel();
            {
                let (output, appsrc, stop) = (output.clone(), appsrc.clone(), stop.clone());
                std::thread::Builder::new()
                    .name("tabextend-screencopy".into())
                    .spawn(move || capture_thread(&output, &appsrc, &stop, ready_tx))
                    .map_err(|e| e.to_string())?;
            }
            wait_ready(&ready_rx, "screen capture")?;

            let (input_tx, input_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            {
                let output = output.clone();
                std::thread::Builder::new()
                    .name("tabextend-wlr-input".into())
                    .spawn(move || input_thread(&output, (width, height), input_rx, ready_tx))
                    .map_err(|e| e.to_string())?;
            }
            wait_ready(&ready_rx, "input")?;
            Ok(Self {
                compositor,
                output: output.clone(),
                size: (width, height),
                appsrc,
                stop,
                input: Mutex::new(input_tx),
            })
        })();
        if opened.is_err() {
            compositor.remove_output(&output);
        }
        opened
    }
}

fn wait_ready(ready: &mpsc::Receiver<Result<(), String>>, what: &str) -> Result<(), String> {
    match ready.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => result,
        Err(_) => Err(format!("{what} did not start")),
    }
}

impl Capture for WlrootsCapture {
    fn source(&self, _gpu_buffers: bool) -> String {
        // Caps are set from the first frame: the compositor picks the format.
        format!(
            "appsrc name={SOURCE_NAME} is-live=true do-timestamp=true format=time ! videoconvert"
        )
    }

    fn attach(&self, pipeline: &gst::Bin) -> Result<(), String> {
        let src = pipeline
            .by_name(SOURCE_NAME)
            .ok_or("the pipeline has no frame source")?;
        *self.appsrc.lock().expect("appsrc") = Some(src);
        Ok(())
    }

    fn input_size(&self) -> (f64, f64) {
        (self.size.0.into(), self.size.1.into())
    }

    fn audio_source(&self, device_name: &str) -> Option<SoundSource> {
        Some(SoundSource::VirtualSpeaker {
            label: label(device_name),
        })
    }

    fn inject(&self, call: InputCall) -> BoxFuture<'_, Result<(), String>> {
        let result = if call.is_touch() {
            Err("wlroots desktops have no virtual touch device".to_owned())
        } else {
            self.input
                .lock()
                .expect("input")
                .send(call)
                .map_err(|_| "input thread stopped".to_owned())
        };
        Box::pin(async move { result })
    }

    fn stop(self: Box<Self>) -> BoxFuture<'static, ()> {
        self.stop.store(true, Ordering::SeqCst);
        *self.appsrc.lock().expect("appsrc") = None;
        // Removing the output fails the pending screencopy frame, which
        // wakes and ends the capture thread. Dropping the sender ends input.
        self.compositor.remove_output(&self.output);
        Box::pin(async {})
    }
}

// ---- Screencopy -----------------------------------------------------------

#[derive(Default)]
struct FrameStatus {
    shm: Option<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct CaptureState {
    outputs: Vec<(WlOutput, Option<String>)>,
    frame: FrameStatus,
}

fn capture_thread(
    output_name: &str,
    appsrc: &Mutex<Option<gst::Element>>,
    stop: &AtomicBool,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let setup = (|| {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<CaptureState>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let mut state = CaptureState::default();
        let output = find_output(&globals, &mut queue, &mut state, output_name, |s| {
            &mut s.outputs
        })?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm")?;
        let manager: ZwlrScreencopyManagerV1 = globals
            .bind(&qh, 3..=3, ())
            .map_err(|_| "the compositor does not offer screencopy (version 3)")?;
        Ok::<_, String>((queue, state, output, shm, manager))
    })();
    let (mut queue, mut state, output, shm, manager) = match setup {
        Ok(parts) => {
            let _ = ready.send(Ok(()));
            parts
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let qh = queue.handle();
    let mut shared: Option<SharedBuffer> = None;
    while !stop.load(Ordering::SeqCst) {
        state.frame = FrameStatus::default();
        let frame = manager.capture_output(1, &output, &qh, ());
        let result = (|| {
            while !state.frame.buffer_done && !state.frame.failed {
                queue
                    .blocking_dispatch(&mut state)
                    .map_err(|e| e.to_string())?;
            }
            if state.frame.failed {
                return Err("capture failed".to_owned());
            }
            let spec = state
                .frame
                .shm
                .ok_or("the compositor offered no shared-memory buffer")?;
            if shared.as_ref().is_none_or(|s| s.spec != spec) {
                shared = Some(SharedBuffer::new(&shm, &qh, spec)?);
            }
            let buffer = shared.as_ref().expect("buffer");
            frame.copy_with_damage(&buffer.buffer);
            while !state.frame.ready && !state.frame.failed {
                queue
                    .blocking_dispatch(&mut state)
                    .map_err(|e| e.to_string())?;
            }
            if state.frame.failed {
                return Err("capture failed".to_owned());
            }
            Ok(())
        })();
        frame.destroy();
        if result.is_err() || stop.load(Ordering::SeqCst) {
            break;
        }
        if let (Some(src), Some(buffer)) = (appsrc.lock().expect("appsrc").clone(), &shared) {
            buffer.push_to(&src);
        }
    }
}

/// Bind every `wl_output` (version 4, for names) and return the named one.
fn find_output<S>(
    globals: &GlobalList,
    queue: &mut EventQueue<S>,
    state: &mut S,
    name: &str,
    outputs: impl Fn(&mut S) -> &mut Vec<(WlOutput, Option<String>)>,
) -> Result<WlOutput, String>
where
    S: Dispatch<WlOutput, ()> + 'static,
{
    let qh = queue.handle();
    for global in globals.contents().clone_list() {
        if global.interface == WlOutput::interface().name && global.version >= 4 {
            let output = globals
                .registry()
                .bind::<WlOutput, _, _>(global.name, 4, &qh, ());
            outputs(state).push((output, None));
        }
    }
    queue.roundtrip(state).map_err(|e| e.to_string())?;
    outputs(state)
        .iter()
        .find(|(_, n)| n.as_deref() == Some(name))
        .map(|(o, _)| o.clone())
        .ok_or_else(|| format!("output {name} not found"))
}

/// A shared-memory buffer the compositor copies frames into.
struct SharedBuffer {
    spec: (wl_shm::Format, u32, u32, u32),
    file: File,
    buffer: WlBuffer,
    _pool: WlShmPool,
}

impl SharedBuffer {
    fn new(
        shm: &WlShm,
        qh: &QueueHandle<CaptureState>,
        spec: (wl_shm::Format, u32, u32, u32),
    ) -> Result<Self, String> {
        let (format, width, height, stride) = spec;
        let size = stride as u64 * height as u64;
        let fd = memfd_create("tabextend-frame", MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
        let file = File::from(fd);
        file.set_len(size).map_err(|e| e.to_string())?;
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            qh,
            (),
        );
        Ok(Self {
            spec,
            file,
            buffer,
            _pool: pool,
        })
    }

    /// Copy the frame out (rows packed tight, as GStreamer expects) and hand
    /// it to the pipeline.
    fn push_to(&self, src: &gst::Element) {
        let (format, width, height, stride) = self.spec;
        let Some(gst_format) = gst_format(format) else {
            return;
        };
        let row = width as usize * 4;
        let mut data = vec![0u8; row * height as usize];
        for (y, out) in data.chunks_exact_mut(row).enumerate() {
            if self
                .file
                .read_exact_at(out, y as u64 * stride as u64)
                .is_err()
            {
                return;
            }
        }
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", gst_format)
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(0, 1))
            .build();
        if src.property::<Option<gst::Caps>>("caps").as_ref() != Some(&caps) {
            src.set_property("caps", &caps);
        }
        let _ = src
            .emit_by_name::<gst::FlowReturn>("push-buffer", &[&gst::Buffer::from_mut_slice(data)]);
    }
}

/// wl_shm formats name bytes from the most significant end; GStreamer from
/// memory order. Little-endian: XRGB8888 is BGRx in memory.
fn gst_format(format: wl_shm::Format) -> Option<&'static str> {
    match format {
        wl_shm::Format::Xrgb8888 => Some("BGRx"),
        wl_shm::Format::Argb8888 => Some("BGRA"),
        wl_shm::Format::Xbgr8888 => Some("RGBx"),
        wl_shm::Format::Abgr8888 => Some("RGBA"),
        _ => None,
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for CaptureState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for CaptureState {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        record_output_name(&mut state.outputs, output, event);
    }
}

fn record_output_name(
    outputs: &mut [(WlOutput, Option<String>)],
    output: &WlOutput,
    event: wl_output::Event,
) {
    if let wl_output::Event::Name { name } = event
        && let Some(entry) = outputs.iter_mut().find(|(o, _)| o == output)
    {
        entry.1 = Some(name);
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => state.frame.shm = Some((format, width, height, stride)),
            Event::BufferDone => state.frame.buffer_done = true,
            Event::Ready { .. } => state.frame.ready = true,
            Event::Failed => state.frame.failed = true,
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($state:ty: $($proxy:ty),*) => {$(
        impl Dispatch<$proxy, ()> for $state {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore_events!(CaptureState: WlShm, WlShmPool, WlBuffer, ZwlrScreencopyManagerV1);

// ---- Input ----------------------------------------------------------------

#[derive(Default)]
struct InputState {
    outputs: Vec<(WlOutput, Option<String>)>,
    has_keyboard: bool,
    keymap: Option<(OwnedFd, u32)>,
}

fn input_thread(
    output_name: &str,
    (width, height): (u32, u32),
    calls: mpsc::Receiver<InputCall>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let setup = (|| {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<InputState>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let mut state = InputState::default();
        let output = find_output(&globals, &mut queue, &mut state, output_name, |s| {
            &mut s.outputs
        })?;
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).map_err(|_| "no seat")?;
        let pointers: ZwlrVirtualPointerManagerV1 = globals
            .bind(&qh, 2..=2, ())
            .map_err(|_| "the compositor does not offer virtual pointers")?;
        let keyboards: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| "the compositor does not offer virtual keyboards")?;

        // Type with the user's own layout when there is a keyboard to copy
        // it from.
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        if state.has_keyboard {
            let keyboard = seat.get_keyboard(&qh, ());
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
            keyboard.release();
        }
        let pointer =
            pointers.create_virtual_pointer_with_output(Some(&seat), Some(&output), &qh, ());
        let keyboard = keyboards.create_virtual_keyboard(&seat, &qh, ());
        let (fd, size) = match state.keymap.take() {
            Some(keymap) => keymap,
            None => fallback_keymap()?,
        };
        keyboard.keymap(wl_keyboard::KeymapFormat::XkbV1.into(), fd.as_fd(), size);
        conn.flush().map_err(|e| e.to_string())?;
        Ok::<_, String>((conn, pointer, keyboard))
    })();
    let (conn, pointer, keyboard) = match setup {
        Ok(parts) => {
            let _ = ready.send(Ok(()));
            parts
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let start = Instant::now();
    while let Ok(call) = calls.recv() {
        let time = start.elapsed().as_millis() as u32;
        send_input(&pointer, &keyboard, call, time, (width, height));
        if conn.flush().is_err() {
            break;
        }
    }
    pointer.destroy();
    keyboard.destroy();
    let _ = conn.flush();
}

fn send_input(
    pointer: &ZwlrVirtualPointerV1,
    keyboard: &ZwpVirtualKeyboardV1,
    call: InputCall,
    time: u32,
    (width, height): (u32, u32),
) {
    let state = |pressed: bool| if pressed { 1 } else { 0 };
    match call {
        InputCall::PointerMotion { x, y } => {
            pointer.motion_absolute(time, x.round() as u32, y.round() as u32, width, height);
            pointer.frame();
        }
        InputCall::PointerButton { code, pressed } => {
            let button_state = if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            pointer.button(time, code as u32, button_state);
            pointer.frame();
        }
        InputCall::Scroll { axis, steps } => {
            let axis = match axis {
                ScrollAxis::Vertical => wl_pointer::Axis::VerticalScroll,
                ScrollAxis::Horizontal => wl_pointer::Axis::HorizontalScroll,
            };
            pointer.axis_source(wl_pointer::AxisSource::Wheel);
            // 15 is the usual distance of one wheel click.
            pointer.axis_discrete(time, axis, f64::from(steps) * 15.0, steps);
            pointer.frame();
        }
        InputCall::Key { code, pressed } => keyboard.key(time, code as u32, state(pressed)),
        // Refused before it gets here: see `WlrootsCapture::inject`.
        InputCall::TouchDown { .. } | InputCall::TouchMotion { .. } | InputCall::TouchUp { .. } => {
        }
    }
}

fn fallback_keymap() -> Result<(OwnedFd, u32), String> {
    let fd = memfd_create("tabextend-keymap", MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
    let file = File::from(fd);
    // xkbcommon reads the keymap as a NUL-terminated string.
    let mut text = FALLBACK_KEYMAP.to_vec();
    text.push(0);
    file.write_all_at(&text, 0).map_err(|e| e.to_string())?;
    Ok((OwnedFd::from(file), text.len() as u32))
}

impl Dispatch<WlRegistry, GlobalListContents> for InputState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for InputState {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        record_output_name(&mut state.outputs, output, event);
    }
}

impl Dispatch<WlSeat, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            state.has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
        }
    }
}

impl Dispatch<WlKeyboard, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap {
            format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = event
        {
            state.keymap = Some((fd, size));
        }
    }
}

ignore_events!(InputState: ZwlrVirtualPointerManagerV1, ZwlrVirtualPointerV1, ZwpVirtualKeyboardManagerV1, ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_compositor_from_its_variables() {
        let only = |name: &'static str| move |v: &str| v == name;
        assert_eq!(
            Compositor::from_env(only("SWAYSOCK")),
            Some(Compositor::Sway)
        );
        assert_eq!(
            Compositor::from_env(only("HYPRLAND_INSTANCE_SIGNATURE")),
            Some(Compositor::Hyprland)
        );
        assert_eq!(Compositor::from_env(|_| false), None);
    }

    #[test]
    fn new_output_goes_right_of_the_active_ones() {
        // Shape of `swaymsg -t get_outputs -r` (trimmed).
        let before = parse_sway_outputs(
            r#"[{"name":"eDP-1","active":true,"rect":{"x":0,"y":0,"width":1920,"height":1080}},
                {"name":"HDMI-A-1","active":false,"rect":{"x":0,"y":0,"width":0,"height":0}}]"#,
        )
        .unwrap();
        let after = parse_sway_outputs(
            r#"[{"name":"eDP-1","active":true,"rect":{"x":0,"y":0,"width":1920,"height":1080}},
                {"name":"HDMI-A-1","active":false,"rect":{"x":0,"y":0,"width":0,"height":0}},
                {"name":"HEADLESS-1","active":true,"rect":{"x":1920,"y":0,"width":1280,"height":720}}]"#,
        )
        .unwrap();
        assert_eq!(new_output(&before, &after), Some("HEADLESS-1"));
        assert_eq!(right_edge(&before), 1920);
        assert_eq!(right_edge(&[]), 0);
    }

    #[test]
    fn shm_formats_map_to_memory_order() {
        assert_eq!(gst_format(wl_shm::Format::Xrgb8888), Some("BGRx"));
        assert_eq!(gst_format(wl_shm::Format::Abgr8888), Some("RGBA"));
        assert_eq!(gst_format(wl_shm::Format::Rgb565), None);
    }

    #[test]
    fn fallback_keymap_is_a_complete_keymap() {
        let text = std::str::from_utf8(FALLBACK_KEYMAP).unwrap();
        assert!(text.starts_with("xkb_keymap {"));
        for section in [
            "xkb_keycodes",
            "xkb_types",
            "xkb_compatibility",
            "xkb_symbols",
        ] {
            assert!(text.contains(section), "missing {section}");
        }
    }
}
