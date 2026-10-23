//! Where the frames come from, and where the tablet's input goes.
//!
//! A [`CaptureBackend`] starts a [`Capture`] sized for a tablet. The real one
//! asks GNOME, through the RemoteDesktop portal, for a virtual monitor and
//! for input devices on it; the test pattern lets the server run where there
//! is no desktop (tests, CI) and records the input it is given.

use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use tabextend_core::input::{InputCall, ScrollAxis};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait CaptureBackend: Send + Sync {
    /// Start capturing a surface of this size.
    fn start(&self, width: u32, height: u32) -> BoxFuture<'_, Result<Box<dyn Capture>, String>>;
}

pub trait Capture: Send + Sync {
    /// The gst-launch fragment producing raw video at the requested size.
    /// With `gpu_buffers`, frames stay in GPU memory (DMA-BUF) where the
    /// source can do that.
    fn source(&self, gpu_buffers: bool) -> String;
    /// The surface's size in logical pixels, the space input positions use.
    fn input_size(&self) -> (f64, f64);
    /// Deliver one input call to the surface.
    fn inject(&self, call: InputCall) -> BoxFuture<'_, Result<(), String>>;
    /// Release the surface (for a virtual monitor: remove it from GNOME).
    fn stop(self: Box<Self>) -> BoxFuture<'static, ()>;
}

/// A moving test pattern, no desktop needed. Input is recorded, not applied.
#[derive(Default)]
pub struct TestPattern {
    injected: Arc<Mutex<Vec<InputCall>>>,
    refuse_touch: bool,
}

impl TestPattern {
    /// Behave like a desktop that grants pointer input but not touch.
    pub fn refusing_touch() -> Self {
        Self {
            refuse_touch: true,
            ..Self::default()
        }
    }

    /// Every input call delivered so far, across sessions.
    pub fn injected(&self) -> Arc<Mutex<Vec<InputCall>>> {
        self.injected.clone()
    }
}

struct TestPatternCapture {
    width: u32,
    height: u32,
    injected: Arc<Mutex<Vec<InputCall>>>,
    refuse_touch: bool,
}

impl CaptureBackend for TestPattern {
    fn start(&self, width: u32, height: u32) -> BoxFuture<'_, Result<Box<dyn Capture>, String>> {
        let injected = self.injected.clone();
        let refuse_touch = self.refuse_touch;
        Box::pin(async move {
            Ok(Box::new(TestPatternCapture {
                width,
                height,
                injected,
                refuse_touch,
            }) as Box<dyn Capture>)
        })
    }
}

impl Capture for TestPatternCapture {
    /// videotestsrc draws in system memory; the GPU flag does not apply.
    fn source(&self, _gpu_buffers: bool) -> String {
        format!(
            "videotestsrc is-live=true pattern=ball ! video/x-raw,width={},height={},framerate=60/1",
            self.width, self.height
        )
    }

    fn input_size(&self) -> (f64, f64) {
        (self.width.into(), self.height.into())
    }

    fn inject(&self, call: InputCall) -> BoxFuture<'_, Result<(), String>> {
        if self.refuse_touch && call.is_touch() {
            return Box::pin(async { Err("touch not allowed".to_owned()) });
        }
        self.injected.lock().expect("injected").push(call);
        Box::pin(async { Ok(()) })
    }

    fn stop(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// A GNOME virtual monitor through a RemoteDesktop portal session with a
/// ScreenCast source.
///
/// One session covers the screen and the tablet's input devices, so the
/// user approves once, and input positions map onto the captured stream.
/// The first time, GNOME asks the user to allow it. The portal then hands
/// back a restore token, stored with user-only permissions, so later
/// sessions start without asking until the user revokes the permission.
pub struct PortalVirtualMonitor {
    token_path: PathBuf,
}

impl PortalVirtualMonitor {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            token_path: state_dir.join("portal-restore-token"),
        }
    }

    /// Forget GNOME's saved answer, so the next connection asks again.
    /// Returns whether there was one.
    pub fn forget_permission(&self) -> std::io::Result<bool> {
        match fs::remove_file(&self.token_path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn open(&self, width: u32, height: u32) -> ashpd::Result<PortalCapture> {
        let remote = RemoteDesktop::new().await?;
        let cast = Screencast::new().await?;
        let session = remote
            .create_session(CreateSessionOptions::default())
            .await?;
        let token = fs::read_to_string(&self.token_path).ok();
        // For remote desktop sessions the portal takes persistence on the
        // device selection, not on the source selection.
        remote
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(
                        DeviceType::Touchscreen | DeviceType::Pointer | DeviceType::Keyboard,
                    )
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(token.as_deref().map(str::trim)),
            )
            .await?
            .response()?;
        cast.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Virtual))
                .set_multiple(false),
        )
        .await?
        .response()?;
        let selected = remote
            .start(&session, None, StartOptions::default())
            .await?
            .response()?;
        if let Some(new_token) = selected.restore_token() {
            self.save_token(new_token);
        }
        let stream = selected.streams().first().ok_or(ashpd::Error::NoResponse)?;
        let node = stream.pipe_wire_node_id();
        // The portal may report the stream's logical size; until the virtual
        // monitor exists it often does not, and at scale 1 the logical size
        // is the requested one.
        let logical = stream
            .size()
            .filter(|(w, h)| *w > 0 && *h > 0)
            .map_or((width.into(), height.into()), |(w, h)| (w.into(), h.into()));
        let fd = cast
            .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
            .await?;
        Ok(PortalCapture {
            remote,
            session,
            fd,
            node,
            width,
            height,
            logical,
        })
    }

    fn save_token(&self, token: &str) {
        let result = (|| {
            if let Some(dir) = self.token_path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = self.token_path.with_extension("tmp");
            let _ = fs::remove_file(&tmp);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(token.as_bytes())?;
            fs::rename(&tmp, &self.token_path)
        })();
        if let Err(e) = result {
            eprintln!("warning: could not save screen-sharing permission: {e}");
        }
    }
}

impl CaptureBackend for PortalVirtualMonitor {
    fn start(&self, width: u32, height: u32) -> BoxFuture<'_, Result<Box<dyn Capture>, String>> {
        Box::pin(async move {
            self.open(width, height)
                .await
                .map(|c| Box::new(c) as Box<dyn Capture>)
                .map_err(|e| format!("screen sharing was not started: {e}"))
        })
    }
}

struct PortalCapture {
    remote: RemoteDesktop,
    session: Session<RemoteDesktop>,
    fd: OwnedFd,
    node: u32,
    width: u32,
    height: u32,
    logical: (f64, f64),
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

impl Capture for PortalCapture {
    fn source(&self, gpu_buffers: bool) -> String {
        // The size caps make Mutter create the virtual monitor at the
        // tablet's size. keepalive-time resends the last frame when nothing
        // on screen changes, so a static desktop still produces frames.
        // Without GPU buffers, Mutter reads every frame back into system
        // memory, which costs frame rate at large sizes.
        let (memory, copy) = if gpu_buffers {
            ("(memory:DMABuf)", "")
        } else {
            ("", " always-copy=true")
        };
        format!(
            "pipewiresrc fd={} path={}{copy} keepalive-time=500 \
             ! video/x-raw{memory},width={},height={}",
            self.fd.as_raw_fd(),
            self.node,
            self.width,
            self.height
        )
    }

    fn input_size(&self) -> (f64, f64) {
        self.logical
    }

    fn inject(&self, call: InputCall) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (remote, session, node) = (&self.remote, &self.session, self.node);
            match call {
                InputCall::TouchDown { slot, x, y } => {
                    remote
                        .notify_touch_down(session, node, slot, x, y, Default::default())
                        .await
                }
                InputCall::TouchMotion { slot, x, y } => {
                    remote
                        .notify_touch_motion(session, node, slot, x, y, Default::default())
                        .await
                }
                InputCall::TouchUp { slot } => {
                    remote
                        .notify_touch_up(session, slot, Default::default())
                        .await
                }
                InputCall::PointerMotion { x, y } => {
                    remote
                        .notify_pointer_motion_absolute(session, node, x, y, Default::default())
                        .await
                }
                InputCall::PointerButton { code, pressed } => {
                    remote
                        .notify_pointer_button(
                            session,
                            code,
                            key_state(pressed),
                            Default::default(),
                        )
                        .await
                }
                InputCall::Scroll { axis, steps } => {
                    let axis = match axis {
                        ScrollAxis::Vertical => Axis::Vertical,
                        ScrollAxis::Horizontal => Axis::Horizontal,
                    };
                    remote
                        .notify_pointer_axis_discrete(session, axis, steps, Default::default())
                        .await
                }
                InputCall::Key { code, pressed } => {
                    remote
                        .notify_keyboard_keycode(
                            session,
                            code,
                            key_state(pressed),
                            Default::default(),
                        )
                        .await
                }
            }
            .map_err(|e| e.to_string())
        })
    }

    fn stop(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = self.session.close().await;
            drop(self.fd);
        })
    }
}
