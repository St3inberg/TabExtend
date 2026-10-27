//! Input through libei, handed out by the RemoteDesktop portal.
//!
//! The portal's own Notify* methods check every absolute position against
//! the stream size reported when the session starts, which is 0×0 for a
//! virtual monitor, so they reject all touch and pointer input there (see
//! docs/status.md). libei avoids that: the compositor describes each
//! monitor as a region tagged with the screencast stream's mapping id, and
//! positions are sent in that region's coordinates. The user approved the
//! same portal session, so consent is unchanged.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use reis::ei;
use reis::event::{Connection, Device, DeviceCapability, EiEvent, Region};
use tabextend_core::input::{InputCall, ScrollAxis};

/// libei counts wheel steps in 120ths of a click.
const DISCRETE_STEP: i32 = 120;

struct Tracked {
    device: Device,
    resumed: bool,
}

struct State {
    connection: Connection,
    devices: Vec<Tracked>,
    sequence: u32,
}

pub struct EisInput {
    context: ei::Context,
    state: Arc<Mutex<State>>,
    /// Ends the event thread when this is dropped.
    _stop: tokio::sync::oneshot::Sender<()>,
    /// The screencast stream's mapping id, to find our monitor's region.
    mapping_id: Option<String>,
    /// The size input positions are expressed in (the stream's logical size).
    size: (f64, f64),
}

impl EisInput {
    /// Connect over the socket from the portal's `ConnectToEIS`.
    ///
    /// reis's event stream is not `Send`, so the handshake and the event
    /// loop run on a dedicated thread with its own small runtime; injecting
    /// input only needs the shared device state and the context.
    pub async fn connect(
        fd: OwnedFd,
        mapping_id: Option<String>,
        size: (f64, f64),
    ) -> Result<Self, String> {
        let context = ei::Context::new(UnixStream::from(fd)).map_err(|e| e.to_string())?;
        let (ready_tx, ready_rx) =
            tokio::sync::oneshot::channel::<Result<Arc<Mutex<State>>, String>>();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread_context = context.clone();
        std::thread::Builder::new()
            .name("tabextend-eis".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                runtime.block_on(run_events(thread_context, ready_tx, stop_rx));
            })
            .map_err(|e| e.to_string())?;
        let state = ready_rx
            .await
            .map_err(|_| "libei thread ended early".to_owned())??;
        Ok(Self {
            context,
            state,
            _stop: stop_tx,
            mapping_id,
            size,
        })
    }

    /// A resumed device that can do `capability`.
    fn device(&self, state: &State, capability: DeviceCapability) -> Option<Device> {
        state
            .devices
            .iter()
            .filter(|t| t.resumed && t.device.has_capability(capability))
            .map(|t| t.device.clone())
            .next()
    }

    /// Our monitor among the device's regions: by mapping id, or the only one.
    fn region<'a>(&self, device: &'a Device) -> Option<&'a Region> {
        let regions = device.regions();
        regions
            .iter()
            .find(|r| r.mapping_id.is_some() && r.mapping_id == self.mapping_id)
            .or_else(|| (regions.len() == 1).then(|| &regions[0]))
    }

    /// A position in the stream's coordinates to the compositor's.
    fn place(&self, region: &Region, x: f64, y: f64) -> (f32, f32) {
        let sx = f64::from(region.width) / self.size.0.max(1.0);
        let sy = f64::from(region.height) / self.size.1.max(1.0);
        (
            (f64::from(region.x) + x * sx) as f32,
            (f64::from(region.y) + y * sy) as f32,
        )
    }

    pub fn inject(&self, call: InputCall) -> Result<(), String> {
        let state = self.state.lock().expect("eis state");
        let capability = match call {
            InputCall::TouchDown { .. }
            | InputCall::TouchMotion { .. }
            | InputCall::TouchUp { .. } => DeviceCapability::Touch,
            InputCall::PointerMotion { .. } => DeviceCapability::PointerAbsolute,
            InputCall::PointerButton { .. } => DeviceCapability::Button,
            InputCall::Scroll { .. } => DeviceCapability::Scroll,
            InputCall::Key { .. } => DeviceCapability::Keyboard,
        };
        let device = self
            .device(&state, capability)
            .ok_or_else(|| format!("no {capability:?} device from the desktop yet"))?;
        let missing = || format!("device lacks the {capability:?} interface");
        let positioned = |x: f64, y: f64| {
            self.region(&device)
                .map(|r| self.place(r, x, y))
                .ok_or_else(|| "the virtual monitor's input region is not known yet".to_owned())
        };
        match call {
            InputCall::TouchDown { slot, x, y } => {
                let (x, y) = positioned(x, y)?;
                device
                    .interface::<ei::Touchscreen>()
                    .ok_or_else(missing)?
                    .down(slot, x, y);
            }
            InputCall::TouchMotion { slot, x, y } => {
                let (x, y) = positioned(x, y)?;
                device
                    .interface::<ei::Touchscreen>()
                    .ok_or_else(missing)?
                    .motion(slot, x, y);
            }
            InputCall::TouchUp { slot } => {
                device
                    .interface::<ei::Touchscreen>()
                    .ok_or_else(missing)?
                    .up(slot);
            }
            InputCall::PointerMotion { x, y } => {
                let (x, y) = positioned(x, y)?;
                device
                    .interface::<ei::PointerAbsolute>()
                    .ok_or_else(missing)?
                    .motion_absolute(x, y);
            }
            InputCall::PointerButton { code, pressed } => {
                let state = if pressed {
                    ei::button::ButtonState::Press
                } else {
                    ei::button::ButtonState::Released
                };
                device
                    .interface::<ei::Button>()
                    .ok_or_else(missing)?
                    .button(code as u32, state);
            }
            InputCall::Scroll { axis, steps } => {
                let (x, y) = match axis {
                    ScrollAxis::Vertical => (0, steps * DISCRETE_STEP),
                    ScrollAxis::Horizontal => (steps * DISCRETE_STEP, 0),
                };
                device
                    .interface::<ei::Scroll>()
                    .ok_or_else(missing)?
                    .scroll_discrete(x, y);
            }
            InputCall::Key { code, pressed } => {
                let state = if pressed {
                    ei::keyboard::KeyState::Press
                } else {
                    ei::keyboard::KeyState::Released
                };
                device
                    .interface::<ei::Keyboard>()
                    .ok_or_else(missing)?
                    .key(code as u32, state);
            }
        }
        device
            .device()
            .frame(state.connection.serial(), monotonic_micros());
        self.context.flush().map_err(|e| e.to_string())
    }
}

/// Handshake, then track seats and devices until disconnected or stopped.
async fn run_events(
    context: ei::Context,
    ready: tokio::sync::oneshot::Sender<Result<Arc<Mutex<State>>, String>>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let (connection, mut events) = match context
        .handshake_tokio("TabExtend", ei::handshake::ContextType::Sender)
        .await
    {
        Ok(done) => done,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = context.flush();
    let state = Arc::new(Mutex::new(State {
        connection,
        devices: Vec::new(),
        sequence: 0,
    }));
    if ready.send(Ok(state.clone())).is_err() {
        return;
    }
    loop {
        let event = tokio::select! {
            event = events.next() => event,
            _ = &mut stop => return,
        };
        let Some(Ok(event)) = event else { return };
        let mut state = state.lock().expect("eis state");
        match event {
            EiEvent::SeatAdded(added) => added.seat.bind_capabilities(
                DeviceCapability::PointerAbsolute
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll
                    | DeviceCapability::Keyboard
                    | DeviceCapability::Touch,
            ),
            EiEvent::DeviceAdded(added) => state.devices.push(Tracked {
                device: added.device,
                resumed: false,
            }),
            EiEvent::DeviceRemoved(removed) => {
                state.devices.retain(|t| t.device != removed.device);
            }
            EiEvent::DeviceResumed(resumed) => {
                state.sequence += 1;
                let sequence = state.sequence;
                if let Some(t) = state
                    .devices
                    .iter_mut()
                    .find(|t| t.device == resumed.device)
                {
                    t.device.device().start_emulating(resumed.serial, sequence);
                    t.resumed = true;
                }
            }
            EiEvent::DevicePaused(paused) => {
                if let Some(t) = state.devices.iter_mut().find(|t| t.device == paused.device) {
                    t.resumed = false;
                }
            }
            EiEvent::Disconnected(_) => return,
            _ => {}
        }
        drop(state);
        let _ = context.flush();
    }
}

/// libei timestamps are microseconds on CLOCK_MONOTONIC.
fn monotonic_micros() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000
}
