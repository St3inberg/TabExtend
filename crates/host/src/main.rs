use std::env;
use std::process::ExitCode;

use tabextend_core::check::Status;
use tabextend_host::doctor;

const USAGE: &str = "\
Usage: tabextend <command> [options]

Commands:
  doctor             Check this machine can run TabExtend

Options:
  --encoder NAME     Also try this GStreamer encoder element first
                     (or set TABEXTEND_ENCODER)
  -h, --help         Show this help
  -V, --version      Show the version";

fn main() -> ExitCode {
    // libva prints driver-loading chatter on every encoder probe. Keep its
    // errors, drop the info lines, unless the user asked for them.
    if env::var_os("LIBVA_MESSAGING_LEVEL").is_none() {
        // SAFETY: no other threads exist yet.
        unsafe { env::set_var("LIBVA_MESSAGING_LEVEL", "1") };
    }

    let args: Vec<String> = env::args().skip(1).collect();
    let mut command = None;
    let mut encoder = env::var("TABEXTEND_ENCODER").ok();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("tabextend {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--encoder" => match iter.next() {
                Some(name) => encoder = Some(name.clone()),
                None => return usage_error("--encoder needs an element name"),
            },
            other if command.is_none() && !other.starts_with('-') => {
                command = Some(other.to_owned())
            }
            other => return usage_error(&format!("unexpected argument `{other}`")),
        }
    }

    match command.as_deref() {
        Some("doctor") => match doctor::run(encoder.as_deref()) {
            Status::Fail => ExitCode::FAILURE,
            Status::Pass | Status::Warn => ExitCode::SUCCESS,
        },
        Some(other) => usage_error(&format!("unknown command `{other}`")),
        None => usage_error("no command given"),
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n\n{USAGE}");
    ExitCode::from(2)
}
