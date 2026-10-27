//! Dialogs: pairing, approving a tablet, removing one, preferences, the
//! system check and About.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use qrcode::{Color, QrCode};
use tabextend_core::check::Status;
use tabextend_core::encoder::{ProbeOutcome, default_h264_encoders};
use tabextend_core::pairing::{Fingerprint, PAIRING_WINDOW};
use tabextend_host::approve::ANSWER_TIMEOUT;
use tabextend_host::capture::PortalVirtualMonitor;
use tabextend_host::identity::{PairedStore, default_data_dir};
use tabextend_host::settings::Settings;

use crate::window::Window;
use crate::{ApprovalRequest, runtime};

/// A dialog with a header bar around `content`.
fn framed(title: &str, width: i32, content: &impl IsA<gtk::Widget>) -> adw::Dialog {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(content));
    adw::Dialog::builder()
        .title(title)
        .content_width(width)
        .child(&toolbar)
        .build()
}

fn spinner_page(text: &str) -> adw::StatusPage {
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .build();
    adw::StatusPage::builder()
        .title(text)
        .child(&spinner)
        .build()
}

/// A QR code as a texture: dark modules on white, with the quiet zone
/// scanners need, at 8 pixels per module so it stays sharp.
fn qr_texture(text: &str) -> Option<gdk::MemoryTexture> {
    const SCALE: usize = 8;
    const QUIET: usize = 4;
    let code = QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let side = (modules + 2 * QUIET) * SCALE;
    let mut pixels = vec![255u8; side * side * 3];
    for (i, color) in colors.iter().enumerate() {
        if *color != Color::Dark {
            continue;
        }
        let (mx, my) = (i % modules + QUIET, i / modules + QUIET);
        for y in my * SCALE..(my + 1) * SCALE {
            let row = y * side * 3;
            pixels[row + mx * SCALE * 3..row + (mx + 1) * SCALE * 3].fill(0);
        }
    }
    Some(gdk::MemoryTexture::new(
        side as i32,
        side as i32,
        gdk::MemoryFormat::R8g8b8,
        &glib::Bytes::from_owned(pixels),
        side * 3,
    ))
}

/// Show a QR code for 2 minutes; closes by itself when a tablet pairs.
pub fn pair(window: &Window) {
    let stack = gtk::Stack::new();
    stack.add_named(&spinner_page("Preparing…"), Some("loading"));
    let dialog = framed("Pair a Tablet", 440, &stack);
    window.pair_dialog.replace(Some(dialog.clone()));

    let window_for_close = window.clone();
    dialog.connect_closed(move |_| {
        window_for_close.pair_dialog.replace(None);
        let host = window_for_close.host.clone();
        runtime().spawn(async move {
            if let Some(host) = host.lock().await.as_ref() {
                host.close_pairing().await;
            }
        });
    });
    dialog.present(Some(&window.window));
    open_window(window, &stack);
}

/// Open a pairing window on the host and show its code in `stack`.
fn open_window(window: &Window, stack: &gtk::Stack) {
    stack.set_visible_child_name("loading");
    let (tx, rx) = async_channel::bounded(1);
    let host = window.host.clone();
    runtime().spawn(async move {
        let result = match host.lock().await.as_ref() {
            Some(host) => host.open_pairing().await,
            None => Err("TabExtend is not running".to_owned()),
        };
        let _ = tx.send(result).await;
    });
    let stack = stack.clone();
    let window = window.clone();
    glib::spawn_future_local(async move {
        let Ok(result) = rx.recv().await else { return };
        let (link, name) = match result {
            Ok(opened) => opened,
            Err(error) => {
                window.toast(&error);
                return;
            }
        };
        if let Some(old) = stack.child_by_name("code") {
            stack.remove(&old);
        }
        stack.add_named(
            &code_page(&window, &stack, &link.to_uri(), &name, &link.fingerprint),
            Some("code"),
        );
        stack.set_visible_child_name("code");
    });
}

fn code_page(
    window: &Window,
    stack: &gtk::Stack,
    uri: &str,
    name: &str,
    fingerprint: &Fingerprint,
) -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.set_margin_top(12);
    page.set_margin_bottom(24);
    page.set_margin_start(24);
    page.set_margin_end(24);

    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .width_request(280)
        .height_request(280)
        .halign(gtk::Align::Center)
        .alternative_text("Pairing QR code")
        .build();
    if let Some(texture) = qr_texture(uri) {
        picture.set_paintable(Some(&texture));
    }
    picture.add_css_class("card");
    page.append(&picture);

    let steps = gtk::Label::builder()
        .use_markup(true)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .label(format!(
            "In the TabExtend app on your tablet, tap <b>Scan QR Code</b>.\n\
             Or tap <b>Find Computers</b> and pick <b>{}</b>.",
            glib::markup_escape_text(name)
        ))
        .build();
    page.append(&steps);

    let expires = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .build();
    page.append(&expires);

    let details = adw::PreferencesGroup::new();
    let link_row = adw::ActionRow::builder()
        .title("Pairing Link")
        .subtitle("For “Paste Pairing Link” on the tablet")
        .build();
    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy Link")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let uri_owned = uri.to_owned();
    let window_for_copy = window.clone();
    copy.connect_clicked(move |button| {
        button.clipboard().set_text(&uri_owned);
        window_for_copy.toast("Pairing link copied");
    });
    link_row.add_suffix(&copy);
    details.add(&link_row);
    let key_row = adw::ActionRow::builder()
        .title("This Computer’s Key")
        .subtitle(fingerprint.to_string()[..47].to_owned() + "…")
        .subtitle_selectable(true)
        .build();
    key_row.add_css_class("property");
    details.add(&key_row);
    page.append(&details);

    let renew = gtk::Button::builder()
        .label("_New Code")
        .use_underline(true)
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .visible(false)
        .build();
    let window_for_renew = window.clone();
    let stack_for_renew = stack.clone();
    renew.connect_clicked(move |_| open_window(&window_for_renew, &stack_for_renew));
    page.append(&renew);

    // Count down the pairing window; offer a new code when it runs out.
    let left = Rc::new(Cell::new(PAIRING_WINDOW.as_secs()));
    let tick = {
        let expires = expires.clone();
        let picture = picture.clone();
        move || {
            let secs = left.get();
            if secs == 0 {
                expires.set_label("This code has expired");
                picture.set_opacity(0.2);
                renew.set_visible(true);
                return glib::ControlFlow::Break;
            }
            expires.set_label(&format!("Works once, for {}:{:02}", secs / 60, secs % 60));
            left.set(secs - 1);
            glib::ControlFlow::Continue
        }
    };
    tick();
    let page_weak = page.downgrade();
    glib::timeout_add_seconds_local(1, move || {
        if page_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        tick()
    });
    page.upcast()
}

/// "Pair this tablet?" with the code to compare. Closing or not answering
/// in time counts as no.
pub fn approve(window: &Window, request: ApprovalRequest) {
    let ApprovalRequest {
        device_name,
        code,
        answer,
    } = request;
    let dialog = adw::AlertDialog::new(
        Some(&format!("Pair “{device_name}”?")),
        Some(
            "Allow only if the tablet shows the same code. It will be able to show this computer’s screen and control it.",
        ),
    );
    let code_label = gtk::Label::builder()
        .label(&code)
        .css_classes(["title-1", "numeric"])
        .selectable(false)
        .build();
    dialog.set_extra_child(Some(&code_label));
    dialog.add_responses(&[("deny", "_Deny"), ("allow", "_Allow")]);
    dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("deny"));
    dialog.set_close_response("deny");

    let answer = Rc::new(Cell::new(Some(answer)));
    let answer_for_response = answer.clone();
    dialog.connect_response(None, move |_, response| {
        if let Some(answer) = answer_for_response.take() {
            let _ = answer.send(response == "allow");
        }
    });
    let dialog_weak = dialog.downgrade();
    glib::timeout_add_local_once(ANSWER_TIMEOUT, move || {
        if let Some(dialog) = dialog_weak.upgrade() {
            dialog.close();
        }
    });
    dialog.present(Some(&window.window));
}

pub fn confirm_remove(window: &Window, name: &str, fingerprint: Fingerprint) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Remove “{name}”?")),
        Some("It will no longer be able to connect until it is paired again."),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), ("remove", "_Remove")]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window_for_response = window.clone();
    let name = name.to_owned();
    dialog.connect_response(Some("remove"), move |_, _| {
        let removed = default_data_dir()
            .and_then(|dir| PairedStore::new(&dir))
            .and_then(|store| {
                let mut devices = store.load()?;
                devices.remove(&fingerprint);
                store.save(&devices)
            });
        match removed {
            Ok(()) => window_for_response.toast(&format!("Removed {name}")),
            Err(e) => window_for_response.toast(&format!("Could not remove {name}: {e}")),
        }
        window_for_response.refresh_tablets();
    });
    dialog.present(Some(&window.window));
}

const RESOLUTIONS: [(&str, Option<u32>); 3] = [
    ("Best the Tablet Can Show", None),
    ("Up to 1920 Pixels Wide", Some(1920)),
    ("Up to 1280 Pixels Wide", Some(1280)),
];

pub fn preferences(window: &Window) {
    let Ok(dir) = default_data_dir() else { return };
    let settings = Settings::load(&dir);
    let dialog = adw::PreferencesDialog::new();
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let video = adw::PreferencesGroup::builder()
        .title("Video")
        .description("Changes apply the next time TabExtend starts streaming")
        .build();
    let encoders = default_h264_encoders();
    let mut labels = vec!["Automatic".to_owned()];
    labels.extend(encoders.iter().map(|e| e.backend.clone()));
    let encoder_row = adw::ComboRow::builder()
        .title("Encoder")
        .subtitle("Automatic picks the best one that works on this computer")
        .model(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    let selected = settings
        .encoder
        .as_ref()
        .and_then(|name| encoders.iter().position(|e| &e.element == name))
        .map_or(0, |i| i + 1);
    encoder_row.set_selected(selected as u32);
    video.add(&encoder_row);

    let resolution_row = adw::ComboRow::builder()
        .title("Resolution")
        .subtitle("Lower uses less Wi-Fi and battery")
        .model(&gtk::StringList::new(&RESOLUTIONS.map(|(label, _)| label)))
        .build();
    let selected = RESOLUTIONS
        .iter()
        .position(|(_, width)| *width == settings.max_width)
        .unwrap_or(0);
    resolution_row.set_selected(selected as u32);
    video.add(&resolution_row);
    page.add(&video);

    let save = {
        let window = window.clone();
        let encoder_row = encoder_row.clone();
        let resolution_row = resolution_row.clone();
        let dir = dir.clone();
        move || {
            let encoder = match encoder_row.selected() {
                0 => None,
                i => default_h264_encoders()
                    .get(i as usize - 1)
                    .map(|e| e.element.clone()),
            };
            let max_width = RESOLUTIONS
                .get(resolution_row.selected() as usize)
                .and_then(|(_, w)| *w);
            let settings = Settings { encoder, max_width };
            if let Err(e) = settings.save(&dir) {
                window.toast(&format!("Could not save settings: {e}"));
                return;
            }
            let toast = adw::Toast::builder()
                .title("Saved. Restart to apply")
                .button_label("_Restart Now")
                .build();
            let window_for_restart = window.clone();
            toast.connect_button_clicked(move |_| window_for_restart.restart_host());
            window.add_toast(toast);
        }
    };
    let save_on_encoder = save.clone();
    encoder_row.connect_selected_notify(move |_| save_on_encoder());
    resolution_row.connect_selected_notify(move |_| save());

    let sharing = adw::PreferencesGroup::builder()
        .title("Screen Sharing")
        .build();
    let reset_row = adw::ActionRow::builder()
        .title("Ask for Permission Again")
        .subtitle("GNOME shows its sharing dialog on the next connection")
        .build();
    let reset = gtk::Button::builder()
        .label("_Reset")
        .use_underline(true)
        .valign(gtk::Align::Center)
        .build();
    let window_for_reset = window.clone();
    reset.connect_clicked(move |_| {
        let Ok(dir) = default_data_dir() else { return };
        let message = match PortalVirtualMonitor::new(dir.join("state")).forget_permission() {
            Ok(true) => "GNOME will ask again on the next connection".to_owned(),
            Ok(false) => "There was no saved permission".to_owned(),
            Err(e) => format!("Could not reset: {e}"),
        };
        window_for_reset.toast(&message);
    });
    reset_row.add_suffix(&reset);
    sharing.add(&reset_row);
    page.add(&sharing);

    dialog.add(&page);
    dialog.present(Some(&window.window));
}

fn status_icon(status: Status) -> gtk::Image {
    let (icon, class) = match status {
        Status::Pass => ("emblem-ok-symbolic", "success"),
        Status::Warn => ("dialog-warning-symbolic", "warning"),
        Status::Fail => ("dialog-error-symbolic", "error"),
    };
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class(class);
    image
}

/// The `tabextend doctor` checks, in a dialog.
pub fn system_check(window: &Window) {
    let stack = gtk::Stack::new();
    stack.add_named(&spinner_page("Checking This Computer…"), Some("loading"));
    let dialog = framed("System Check", 520, &stack);
    dialog.set_content_height(600);
    dialog.present(Some(&window.window));

    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn_blocking(move || {
        let _ = tx.send_blocking(tabextend_host::doctor::collect(None));
    });
    glib::spawn_future_local(async move {
        let Ok((report, encoders)) = rx.recv().await else {
            return;
        };
        let page = adw::PreferencesPage::new();
        let checks = adw::PreferencesGroup::builder().title("Checks").build();
        for (name, finding) in &report.results {
            let mut subtitle = glib::markup_escape_text(&finding.summary).to_string();
            if let Some(hint) = &finding.hint {
                subtitle.push('\n');
                subtitle.push_str(&glib::markup_escape_text(hint));
            }
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(name))
                .subtitle(subtitle)
                .build();
            row.add_prefix(&status_icon(finding.status));
            checks.add(&row);
        }
        page.add(&checks);
        let video = adw::PreferencesGroup::builder()
            .title("Video Encoders")
            .description("In the order TabExtend tries them")
            .build();
        for encoder in &encoders {
            let (status, outcome) = match &encoder.outcome {
                ProbeOutcome::Works => (Status::Pass, "Works".to_owned()),
                ProbeOutcome::Limited(limits) => {
                    (Status::Warn, format!("Works, ignores: {limits}"))
                }
                ProbeOutcome::Missing => (Status::Fail, "Not installed".to_owned()),
                ProbeOutcome::Failed(reason) => (Status::Fail, format!("Failed: {reason}")),
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&encoder.spec.backend))
                .subtitle(glib::markup_escape_text(&format!(
                    "{} · {outcome}",
                    encoder.spec.element
                )))
                .build();
            row.add_prefix(&status_icon(status));
            video.add(&row);
        }
        page.add(&video);
        stack.add_named(&page, Some("results"));
        stack.set_visible_child_name("results");
    });
}

pub fn about(window: &Window) {
    adw::AboutDialog::builder()
        .application_name("TabExtend")
        .application_icon(crate::APP_ID)
        .developer_name("Stephen Stenberg")
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/St3inberg/TabExtend")
        .issue_url("https://github.com/St3inberg/TabExtend/issues")
        .license_type(gtk::License::Gpl30)
        .comments("Use an Android tablet as a wireless extended monitor")
        .build()
        .present(Some(&window.window));
}
