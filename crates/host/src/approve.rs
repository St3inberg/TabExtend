//! Asking the user to approve a tablet that paired by scanning.
//!
//! Both screens show the same 6-digit code; the user compares them and
//! answers on the computer. Approval must happen here, not on the tablet:
//! the computer is the one granting access to its screen and input.

use std::io::{self, BufRead, Write};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use crate::capture::BoxFuture;

/// How long the user has to answer before the request is refused.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(90);

pub trait Approver: Send + Sync {
    /// Ask whether `device_name`, showing `code`, may pair. False on no, on
    /// no answer within [`ANSWER_TIMEOUT`], or if nobody can be asked.
    fn approve<'a>(&'a self, device_name: &'a str, code: &'a str) -> BoxFuture<'a, bool>;
}

/// Always gives the same answer. For tests, and for `serve`, which never
/// approves anything (pairing is only open in `pair`).
pub struct FixedAnswer(pub bool);

impl Approver for FixedAnswer {
    fn approve<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, bool> {
        let answer = self.0;
        Box::pin(async move { answer })
    }
}

/// Asks on the terminal: `Allow? [y/N]`.
///
/// A single thread reads standard input for the life of the program, so an
/// answer typed for one request can never be picked up by a later one: old
/// lines are discarded before each question.
pub struct TerminalPrompt {
    lines: Mutex<mpsc::Receiver<String>>,
}

impl TerminalPrompt {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            lines: Mutex::new(rx),
        }
    }
}

impl Default for TerminalPrompt {
    fn default() -> Self {
        Self::new()
    }
}

/// Only an explicit yes counts.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

impl Approver for TerminalPrompt {
    fn approve<'a>(&'a self, device_name: &'a str, code: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            {
                let lines = self.lines.lock().expect("prompt lock");
                while lines.try_recv().is_ok() {}
            }
            println!(
                "\n{device_name} wants to pair.\n\
                 Check the tablet shows this code:  {code}\n\
                 Allow? [y/N] "
            );
            let _ = io::stdout().flush();
            // The blocking receive runs off the async threads, and times out
            // by itself: an async timeout could not interrupt it.
            let answer = tokio::task::block_in_place(|| {
                self.lines
                    .lock()
                    .expect("prompt lock")
                    .recv_timeout(ANSWER_TIMEOUT)
                    .unwrap_or_default()
            });
            let yes = is_yes(&answer);
            println!("{}", if yes { "Allowed." } else { "Refused." });
            yes
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_yes_approves() {
        for yes in ["y", "Y", "yes", " YES "] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "n", "no", "yeah", "1", "y y"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
