//! The main window: what is happening, live connection numbers, and the
//! tablets paired with this computer.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use tabextend_core::pairing::Fingerprint;
use tabextend_host::events::{ServerEvent, StreamStats};
use tabextend_host::identity::{PairedStore, default_data_dir};
use tokio::sync::Mutex;

use crate::{Channels, HostSlot, dialogs, start_host};

/// The top of the window: an icon, what is happening, and what to do.
/// (AdwStatusPage scrolls internally, so inside this scrolling window it
/// would be squeezed to its icon when the content below grows.)
struct Hero {
    root: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    description: gtk::Label,
}

impl Hero {
    fn new() -> Self {
        let icon = gtk::Image::builder()
            .pixel_size(128)
            .css_classes(["dim-label"])
            .margin_bottom(12)
            .build();
        let title = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["title-1"])
            .build();
        let description = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(36)
            .margin_start(12)
            .margin_end(12)
            .build();
        root.append(&icon);
        root.append(&title);
        root.append(&description);
        Self {
            root,
            icon,
            title,
            description,
        }
    }

    fn set_icon_name(&self, name: Option<&str>) {
        self.icon.set_icon_name(name);
    }

    fn set_title(&self, title: &str) {
        self.title.set_label(title);
    }

    fn set_description(&self, description: Option<&str>) {
        self.description.set_label(description.unwrap_or(""));
    }
}

struct ConnectionRows {
    resolution: adw::ActionRow,
    frame_rate: adw::ActionRow,
    bitrate: adw::ActionRow,
    round_trip: adw::ActionRow,
    dropped: adw::ActionRow,
}

pub struct Inner {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    status: Hero,
    pair_button: gtk::Button,
    retry_button: gtk::Button,
    connection: adw::PreferencesGroup,
    rows: ConnectionRows,
    tablets: adw::PreferencesGroup,
    tablet_rows: RefCell<Vec<gtk::Widget>>,
    pub host: HostSlot,
    channels: Channels,
    /// The open pairing dialog, closed when a tablet pairs.
    pub pair_dialog: RefCell<Option<adw::Dialog>>,
}

#[derive(Clone)]
pub struct Window(Rc<Inner>);

impl std::ops::Deref for Window {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

fn value_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_css_class("property");
    row
}

impl Window {
    pub fn new(app: &adw::Application, channels: Channels) -> Self {
        let menu = gio::Menu::new();
        menu.append(Some("_Preferences"), Some("app.preferences"));
        menu.append(Some("_System Check"), Some("app.system-check"));
        menu.append(Some("_About TabExtend"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let title = adw::WindowTitle::new("TabExtend", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_end(&menu_button);

        let status = Hero::new();
        status.set_icon_name(Some("video-display-symbolic"));
        status.set_title("Starting…");
        status.set_description(Some("Checking which video encoders work on this computer"));
        let pair_button = gtk::Button::builder()
            .label("_Pair a Tablet…")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let retry_button = gtk::Button::builder()
            .label("_Try Again")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .visible(false)
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Vertical, 12);
        buttons.set_margin_top(12);
        buttons.append(&pair_button);
        buttons.append(&retry_button);
        status.root.append(&buttons);

        let rows = ConnectionRows {
            resolution: value_row("Resolution"),
            frame_rate: value_row("Frame Rate"),
            bitrate: value_row("Bitrate"),
            round_trip: value_row("Round Trip"),
            dropped: value_row("Dropped Frames"),
        };
        let connection = adw::PreferencesGroup::builder()
            .title("Connection")
            .visible(false)
            .build();
        for row in [
            &rows.resolution,
            &rows.frame_rate,
            &rows.bitrate,
            &rows.round_trip,
            &rows.dropped,
        ] {
            connection.add(row);
        }
        let tablets = adw::PreferencesGroup::builder()
            .title("Paired Tablets")
            .description("Tablets that can connect to this computer")
            .build();

        let page = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page.append(&status.root);
        let groups = gtk::Box::new(gtk::Orientation::Vertical, 24);
        groups.set_margin_start(12);
        groups.set_margin_end(12);
        groups.set_margin_bottom(24);
        groups.append(&connection);
        groups.append(&tablets);
        page.append(
            &adw::Clamp::builder()
                .maximum_size(560)
                .child(&groups)
                .build(),
        );
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&page)
                .build(),
        ));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("TabExtend")
            .default_width(560)
            .default_height(720)
            .width_request(360)
            .height_request(294)
            .content(&toolbar)
            .build();

        let this = Self(Rc::new(Inner {
            window,
            toasts,
            title,
            status,
            pair_button,
            retry_button,
            connection,
            rows,
            tablets,
            tablet_rows: RefCell::new(Vec::new()),
            host: Arc::new(Mutex::new(None)),
            channels,
            pair_dialog: RefCell::new(None),
        }));
        this.connect_signals();
        this.listen();
        this.refresh_tablets();
        this.restart_host();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn connect_signals(&self) {
        let this = self.clone();
        self.pair_button
            .connect_clicked(move |_| dialogs::pair(&this));
        let this = self.clone();
        self.retry_button
            .connect_clicked(move |_| this.restart_host());
    }

    /// Server events and approval requests, handled on the UI thread.
    fn listen(&self) {
        let this = self.clone();
        let events = self.channels.events.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                this.handle(event);
            }
        });
        let this = self.clone();
        let approvals = self.channels.approvals.clone();
        glib::spawn_future_local(async move {
            while let Ok(request) = approvals.recv().await {
                dialogs::approve(&this, request);
            }
        });
    }

    /// Start or restart the host with the saved settings.
    pub fn restart_host(&self) {
        self.retry_button.set_visible(false);
        self.pair_button.set_sensitive(false);
        self.status.set_icon_name(Some("video-display-symbolic"));
        self.status.set_title("Starting…");
        self.status
            .set_description(Some("Checking which video encoders work on this computer"));
        let this = self.clone();
        start_host(
            self.host.clone(),
            &self.channels,
            move |result| match result {
                Ok(summary) => {
                    this.title.set_subtitle(&summary);
                    this.pair_button.set_sensitive(true);
                    this.show_waiting();
                }
                Err(error) => {
                    this.title.set_subtitle("");
                    this.status.set_icon_name(Some("dialog-warning-symbolic"));
                    this.status.set_title("Could Not Start");
                    this.status.set_description(Some(&error));
                    this.retry_button.set_visible(true);
                }
            },
        );
    }

    fn show_waiting(&self) {
        self.connection.set_visible(false);
        self.status.set_icon_name(Some("video-display-symbolic"));
        if self.tablet_rows.borrow().is_empty() {
            self.status.set_title("No Tablets Yet");
            self.status.set_description(Some(
                "Install TabExtend on your Android tablet, then pair it with this computer",
            ));
        } else {
            self.status.set_title("Ready");
            self.status.set_description(Some(
                "Open TabExtend on your tablet to use it as a second screen",
            ));
        }
    }

    fn handle(&self, event: ServerEvent) {
        if std::env::var_os("TABEXTEND_DEBUG_EVENTS").is_some() {
            eprintln!("event: {event}");
        }
        match event {
            ServerEvent::Streaming {
                name,
                width,
                height,
                encoder,
            } => {
                self.status.set_icon_name(Some("video-display-symbolic"));
                self.status.set_title(&format!("Streaming to {name}"));
                self.status.set_description(Some(
                    "Drag windows onto the tablet: it is the screen to the right",
                ));
                self.rows
                    .resolution
                    .set_subtitle(&format!("{width} × {height}"));
                self.rows.frame_rate.set_subtitle("–");
                self.rows.bitrate.set_subtitle(&format!("– · {encoder}"));
                self.rows.round_trip.set_subtitle("–");
                self.rows.dropped.set_subtitle("0");
                self.connection.set_visible(true);
            }
            ServerEvent::Stats(stats) => self.show_stats(&stats),
            ServerEvent::Disconnected { .. } => self.show_waiting(),
            ServerEvent::Paired { name, .. } => {
                if let Some(dialog) = self.pair_dialog.borrow_mut().take() {
                    dialog.close();
                }
                self.refresh_tablets();
                self.toast(&format!("Paired with {name}"));
            }
            ServerEvent::Refused { reason, .. } if reason.contains("pairing") => {
                self.toast(&reason);
            }
            event => eprintln!("{event}"),
        }
    }

    fn show_stats(&self, stats: &StreamStats) {
        self.rows
            .frame_rate
            .set_subtitle(&format!("{:.0} fps", stats.sent_fps));
        let encoder = self.rows.bitrate.subtitle().unwrap_or_default();
        let encoder = encoder.split(" · ").nth(1).unwrap_or("").to_owned();
        self.rows
            .bitrate
            .set_subtitle(&format!("{:.1} Mbit/s · {encoder}", stats.mbps));
        self.rows
            .round_trip
            .set_subtitle(&format!("{:.0} ms", stats.round_trip_ms));
        self.rows.dropped.set_subtitle(&stats.dropped.to_string());
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    pub fn add_toast(&self, toast: adw::Toast) {
        self.toasts.add_toast(toast);
    }

    /// Rebuild the list of paired tablets from disk.
    pub fn refresh_tablets(&self) {
        for row in self.tablet_rows.borrow_mut().drain(..) {
            self.tablets.remove(&row);
        }
        let devices = default_data_dir()
            .and_then(|dir| PairedStore::new(&dir))
            .and_then(|store| store.load())
            .unwrap_or_default();
        let mut rows = self.tablet_rows.borrow_mut();
        if devices.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No tablets paired")
                .subtitle("Use “Pair a Tablet” to add one")
                .build();
            self.tablets.add(&row);
            rows.push(row.upcast());
        }
        for device in devices.iter() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&device.name))
                .subtitle(format!("Key {}", short(&device.fingerprint)))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("video-display-symbolic"));
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let this = self.clone();
            let name = device.name.clone();
            let fingerprint = device.fingerprint;
            remove.connect_clicked(move |_| dialogs::confirm_remove(&this, &name, fingerprint));
            row.add_suffix(&remove);
            self.tablets.add(&row);
            rows.push(row.upcast());
        }
        drop(rows);
        if self.title.subtitle().is_empty() {
            return;
        }
        if !self.connection.is_visible() {
            self.show_waiting();
        }
    }
}

/// The first bytes of a fingerprint, for telling tablets apart.
fn short(fingerprint: &Fingerprint) -> String {
    fingerprint.to_string()[..23].to_owned()
}
