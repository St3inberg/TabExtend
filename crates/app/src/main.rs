//! TabExtend for the GNOME desktop.
//!
//! The server runs on a background Tokio runtime; everything it reports
//! (events, approval requests) reaches the window over channels, so the UI
//! thread never blocks.

mod dialogs;
mod window;

use std::sync::{Arc, OnceLock};

use adw::prelude::*;
use gtk::{gio, glib};
use tabextend_host::approve::{ANSWER_TIMEOUT, Approver};
use tabextend_host::capture::BoxFuture;
use tabextend_host::events::ServerEvent;
use tabextend_host::runner::{HostConfig, RunningHost};
use tabextend_host::settings::Settings;
use tokio::sync::{Mutex, oneshot};

use crate::window::Window;

pub const APP_ID: &str = "io.github.st3inberg.TabExtend";

/// The background runtime the server runs on.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
}

/// A tablet asking to pair by code, waiting for the user's answer.
pub struct ApprovalRequest {
    pub device_name: String,
    pub code: String,
    pub answer: oneshot::Sender<bool>,
}

/// Asks through the window: each request becomes a dialog.
struct DialogApprover {
    requests: async_channel::Sender<ApprovalRequest>,
}

impl Approver for DialogApprover {
    fn approve<'a>(&'a self, device_name: &'a str, code: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let (answer, reply) = oneshot::channel();
            let request = ApprovalRequest {
                device_name: device_name.to_owned(),
                code: code.to_owned(),
                answer,
            };
            if self.requests.send(request).await.is_err() {
                return false;
            }
            matches!(
                tokio::time::timeout(ANSWER_TIMEOUT, reply).await,
                Ok(Ok(true))
            )
        })
    }
}

/// The running host, shared between the UI and the runtime.
pub type HostSlot = Arc<Mutex<Option<RunningHost>>>;

/// Channels from the server to the window.
pub struct Channels {
    pub events: async_channel::Receiver<ServerEvent>,
    pub approvals: async_channel::Receiver<ApprovalRequest>,
    events_tx: async_channel::Sender<ServerEvent>,
    approvals_tx: async_channel::Sender<ApprovalRequest>,
}

impl Channels {
    fn new() -> Self {
        let (events_tx, events) = async_channel::unbounded();
        let (approvals_tx, approvals) = async_channel::unbounded();
        Self {
            events,
            approvals,
            events_tx,
            approvals_tx,
        }
    }
}

/// Start (or restart) the host with the saved settings. The result comes
/// back on the UI thread.
pub fn start_host(
    slot: HostSlot,
    channels: &Channels,
    done: impl FnOnce(Result<String, String>) + 'static,
) {
    let events_tx = channels.events_tx.clone();
    let approver = Arc::new(DialogApprover {
        requests: channels.approvals_tx.clone(),
    });
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        // Stop the old host first: it holds the port.
        slot.lock().await.take();
        let settings = tabextend_host::identity::default_data_dir()
            .map(|dir| Settings::load(&dir))
            .unwrap_or_default();
        let config = HostConfig {
            encoder: settings.encoder,
            max_width: settings.max_width,
            ..HostConfig::default()
        };
        let events = Arc::new(move |event| {
            let _ = events_tx.send_blocking(event);
        });
        let result = RunningHost::start(&config, approver, events)
            .await
            .map(|host| {
                let summary = format!("{} · {}", host.address, host.encoder.backend);
                (host, summary)
            });
        let message = match result {
            Ok((host, summary)) => {
                *slot.lock().await = Some(host);
                Ok(summary)
            }
            Err(e) => Err(e),
        };
        let _ = tx.send(message).await;
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            done(result);
        }
    });
}

fn save_screenshot(window: &impl IsA<gtk::Widget>, path: &std::ffi::OsStr) {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let (width, height) = (window.width() as f64, window.height() as f64);
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let Some(node) = snapshot.to_node() else {
        return;
    };
    let Some(renderer) = window.native().and_then(|n| n.renderer()) else {
        return;
    };
    let texture = renderer.render_texture(&node, None);
    if let Err(e) = texture.save_to_png(path) {
        eprintln!("screenshot failed: {e}");
    }
}

fn main() -> glib::ExitCode {
    // libva prints driver chatter on every encoder probe.
    if std::env::var_os("LIBVA_MESSAGING_LEVEL").is_none() {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("LIBVA_MESSAGING_LEVEL", "1") };
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    // The tablet's speaker outlives the process unless removed, so every
    // way of quitting ends here: the window, Ctrl+Q, and signals (logout
    // and `kill` send SIGTERM; Ctrl+C in a terminal sends SIGINT).
    app.connect_shutdown(|_| tabextend_host::audio::remove_speakers());
    let (quit_tx, quit_rx) = async_channel::bounded::<()>(1);
    runtime().spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut hangup), Ok(mut interrupt), Ok(mut terminate)) = (
            signal(SignalKind::hangup()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) else {
            return;
        };
        tokio::select! {
            _ = hangup.recv() => {}
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
        let _ = quit_tx.send(()).await;
    });
    glib::spawn_future_local({
        let app = app.clone();
        async move {
            if quit_rx.recv().await.is_ok() {
                app.quit();
            }
        }
    });
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let window = Window::new(app, Channels::new());
        app.add_action_entries([
            gio::ActionEntry::builder("pair")
                .activate({
                    let window = window.clone();
                    move |_, _, _| dialogs::pair(&window)
                })
                .build(),
            gio::ActionEntry::builder("preferences")
                .activate({
                    let window = window.clone();
                    move |_, _, _| dialogs::preferences(&window)
                })
                .build(),
            gio::ActionEntry::builder("system-check")
                .activate({
                    let window = window.clone();
                    move |_, _, _| dialogs::system_check(&window)
                })
                .build(),
            gio::ActionEntry::builder("about")
                .activate({
                    let window = window.clone();
                    move |_, _, _| dialogs::about(&window)
                })
                .build(),
            gio::ActionEntry::builder("quit")
                .activate(|app: &adw::Application, _, _| app.quit())
                .build(),
        ]);
        app.set_accels_for_action("app.pair", &["<Control>n"]);
        app.set_accels_for_action("app.preferences", &["<Control>comma"]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
        window.present();
        // For documentation and checking the UI: render the window to a PNG.
        // TABEXTEND_SCREENSHOT_DELAY (seconds) and TABEXTEND_SCREENSHOT_PAIR=1
        // (open the pairing dialog first) choose what is captured.
        if let Some(path) = std::env::var_os("TABEXTEND_SCREENSHOT") {
            let delay: u32 = std::env::var("TABEXTEND_SCREENSHOT_DELAY")
                .ok()
                .and_then(|d| d.parse().ok())
                .unwrap_or(4);
            if std::env::var_os("TABEXTEND_SCREENSHOT_PAIR").is_some() {
                let window = window.clone();
                glib::timeout_add_seconds_local_once(delay.saturating_sub(2).max(1), move || {
                    dialogs::pair(&window)
                });
            }
            let gtk_window = window.window.clone();
            glib::timeout_add_seconds_local_once(delay, move || {
                save_screenshot(&gtk_window, &path)
            });
        }
    });
    app.run()
}
