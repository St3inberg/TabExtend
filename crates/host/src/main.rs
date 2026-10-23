use std::env;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use qrcode::QrCode;
use qrcode::render::{svg, unicode};
use tabextend_core::check::Status;
use tabextend_core::encoder::{default_h264_encoders, select_encoder};
use tabextend_core::pairing::{Fingerprint, PairingLink, PairingWindow};
use tabextend_core::protocol::PAIRING_TOKEN_LEN;
use tabextend_host::capture::{CaptureBackend, PortalVirtualMonitor, TestPattern};
use tabextend_host::doctor;
use tabextend_host::identity::{HostIdentity, PairedStore, default_data_dir};
use tabextend_host::probe::GstProbe;
use tabextend_host::server::{Server, ServerOptions};
use tokio::net::TcpListener;

const DEFAULT_PORT: u16 = 47420;

const USAGE: &str = "\
Usage: tabextend <command> [options]

Commands:
  doctor             Check this machine can run TabExtend
  pair               Show a QR code to pair a tablet, then serve it
  serve              Wait for paired tablets and stream to them
  devices            List paired tablets
  unpair <fp>        Forget a tablet (fingerprint, or its first characters)
  forget-permission  Make GNOME ask again for screen sharing and input

Options:
  --encoder NAME     Use this GStreamer encoder element
                     (or set TABEXTEND_ENCODER)
  --listen ADDR      Address to listen on (default: this machine's LAN address)
  --port N           Port to listen on (default: 47420)
  --max-width N      Scale the stream down to at most N pixels wide
  --test-pattern     Stream a test pattern instead of a virtual monitor
  -h, --help         Show this help
  -V, --version      Show the version";

#[derive(Default)]
struct Options {
    command: Option<String>,
    argument: Option<String>,
    encoder: Option<String>,
    listen: Option<IpAddr>,
    port: Option<u16>,
    max_width: Option<u32>,
    test_pattern: bool,
}

fn main() -> ExitCode {
    // libva prints driver-loading chatter on every encoder probe. Keep its
    // errors, drop the info lines, unless the user asked for them.
    if env::var_os("LIBVA_MESSAGING_LEVEL").is_none() {
        // SAFETY: no other threads exist yet.
        unsafe { env::set_var("LIBVA_MESSAGING_LEVEL", "1") };
    }

    let options = match parse_args(env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => return usage_error(&message),
    };

    let result = match options.command.as_deref() {
        Some("doctor") => {
            return match doctor::run(options.encoder.as_deref()) {
                Status::Fail => ExitCode::FAILURE,
                Status::Pass | Status::Warn => ExitCode::SUCCESS,
            };
        }
        Some("pair") => run_server(&options, true),
        Some("serve") => run_server(&options, false),
        Some("devices") => list_devices(),
        Some("unpair") => unpair(options.argument.as_deref()),
        Some("forget-permission") => forget_permission(),
        Some(other) => return usage_error(&format!("unknown command `{other}`")),
        None => return usage_error("no command given"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut options = Options {
        encoder: env::var("TABEXTEND_ENCODER").ok(),
        ..Options::default()
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tabextend {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--encoder" => options.encoder = Some(value("--encoder")?),
            "--listen" => {
                options.listen = Some(
                    value("--listen")?
                        .parse()
                        .map_err(|_| "--listen needs an IP address")?,
                )
            }
            "--port" => {
                options.port = Some(
                    value("--port")?
                        .parse()
                        .map_err(|_| "--port needs a number from 1 to 65535")?,
                )
            }
            "--max-width" => {
                options.max_width = Some(
                    value("--max-width")?
                        .parse()
                        .map_err(|_| "--max-width needs a number")?,
                )
            }
            "--test-pattern" => options.test_pattern = true,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other if options.command.is_none() => options.command = Some(other.to_owned()),
            other if options.argument.is_none() => options.argument = Some(other.to_owned()),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(Some(options))
}

/// The address other devices on the LAN reach this machine at: the source
/// address of the default route. Connecting a UDP socket sends nothing.
fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn run_server(options: &Options, pairing: bool) -> Result<(), String> {
    let data_dir = default_data_dir().map_err(|e| e.to_string())?;
    let identity = HostIdentity::load_or_create(&data_dir).map_err(|e| e.to_string())?;
    let paired = PairedStore::new(&data_dir).map_err(|e| e.to_string())?;
    if !pairing && paired.load().map_err(|e| e.to_string())?.is_empty() {
        return Err("no tablet is paired yet; run `tabextend pair` first".into());
    }

    let probe = GstProbe::new(1280, 800).map_err(|e| e.to_string())?;
    let encoder = select_encoder(&probe, &default_h264_encoders(), options.encoder.as_deref())
        .map_err(|e| format!("{e}; run `tabextend doctor` for details"))?;
    eprintln!("Encoder: {} ({})", encoder.element, encoder.backend);

    let ip = options
        .listen
        .or_else(lan_address)
        .ok_or("cannot find this machine's network address; use --listen")?;
    let address = SocketAddr::new(ip, options.port.unwrap_or(DEFAULT_PORT));

    let capture: Arc<dyn CaptureBackend> = if options.test_pattern {
        Arc::new(TestPattern::default())
    } else {
        let state_dir = data_dir.join("state");
        Arc::new(PortalVirtualMonitor::new(state_dir))
    };
    let server = Server::new(
        &identity,
        paired,
        capture,
        ServerOptions {
            encoder,
            max_width: options.max_width,
        },
    )
    .map_err(|e| e.to_string())?;

    if pairing {
        let mut token = [0u8; PAIRING_TOKEN_LEN];
        getrandom::fill(&mut token).map_err(|e| e.to_string())?;
        server.open_pairing(PairingWindow::open(token, Instant::now()));
        show_pairing(&PairingLink {
            host: match ip {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => format!("[{v6}]"),
            },
            port: address.port(),
            fingerprint: identity.fingerprint,
            token,
        });
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = TcpListener::bind(address)
            .await
            .map_err(|e| format!("cannot listen on {address}: {e}"))?;
        eprintln!(
            "Listening on {address} · host fingerprint {}",
            identity.fingerprint
        );
        if !pairing {
            eprintln!("Waiting for a paired tablet. Ctrl+C to stop.");
        }
        tokio::select! {
            result = Arc::new(server).run(listener) => result.map_err(|e| e.to_string()),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    })
}

fn show_pairing(link: &PairingLink) {
    let uri = link.to_uri();
    match QrCode::new(uri.as_bytes()) {
        Ok(code) => {
            // Light modules drawn as blocks: right way round on dark terminals.
            let text = code
                .render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Light)
                .light_color(unicode::Dense1x2::Dark)
                .quiet_zone(true)
                .build();
            println!("\nScan with the tablet's camera to pair:\n\n{text}");
            if let Some(path) = save_svg(&code) {
                println!("Larger QR code: {path}");
            }
        }
        Err(e) => eprintln!("could not draw QR code: {e}"),
    }
    println!("Or open this link on the tablet:\n{uri}\n");
    println!(
        "Check the tablet shows this fingerprint:\n{}\n",
        link.fingerprint
    );
    println!("The code works once, for 2 minutes.");
}

/// Write the QR code as an SVG in the user's runtime directory, which only
/// they can read, for screens where the terminal one is too small to scan.
fn save_svg(code: &QrCode) -> Option<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = env::var_os("XDG_RUNTIME_DIR")?;
    let path = std::path::Path::new(&dir).join("tabextend-pair.svg");
    let image = code.render::<svg::Color>().min_dimensions(400, 400).build();
    let _ = std::fs::remove_file(&path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    file.write_all(image.as_bytes()).ok()?;
    Some(path.display().to_string())
}

fn list_devices() -> Result<(), String> {
    let data_dir = default_data_dir().map_err(|e| e.to_string())?;
    let devices = PairedStore::new(&data_dir)
        .and_then(|s| s.load())
        .map_err(|e| e.to_string())?;
    if devices.is_empty() {
        println!("No paired tablets. Run `tabextend pair` to add one.");
    }
    for device in devices.iter() {
        println!("{}  {}", device.fingerprint.to_hex(), device.name);
    }
    Ok(())
}

fn unpair(prefix: Option<&str>) -> Result<(), String> {
    let prefix = prefix
        .map(|p| p.replace(':', "").to_ascii_lowercase())
        .filter(|p| p.len() >= 4)
        .ok_or(
            "give at least the first 4 characters of the fingerprint (see `tabextend devices`)",
        )?;
    let data_dir = default_data_dir().map_err(|e| e.to_string())?;
    let store = PairedStore::new(&data_dir).map_err(|e| e.to_string())?;
    let mut devices = store.load().map_err(|e| e.to_string())?;
    let matches: Vec<Fingerprint> = devices
        .iter()
        .filter(|d| d.fingerprint.to_hex().starts_with(&prefix))
        .map(|d| d.fingerprint)
        .collect();
    match matches.as_slice() {
        [] => Err(format!("no paired tablet starts with {prefix}")),
        [one] => {
            devices.remove(one);
            store.save(&devices).map_err(|e| e.to_string())?;
            println!("Forgot {}", one.to_hex());
            Ok(())
        }
        _ => Err(format!(
            "{prefix} matches several tablets; give more characters"
        )),
    }
}

fn forget_permission() -> Result<(), String> {
    let data_dir = default_data_dir().map_err(|e| e.to_string())?;
    let had = PortalVirtualMonitor::new(data_dir.join("state"))
        .forget_permission()
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        if had {
            "Forgot GNOME's saved permission. The next connection shows the sharing dialog."
        } else {
            "There was no saved permission."
        }
    );
    Ok(())
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n\n{USAGE}");
    ExitCode::from(2)
}
