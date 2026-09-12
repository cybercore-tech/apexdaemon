//! Desktop notifications. Nothing in the existing Rust fleet calls
//! `notify-send` directly — the only precedent is `cyberfleet-toggle` /
//! `cyberplug-toggle` (bash launcher scripts), which both do a best-effort
//! `notify-send` guarded by `command -v`, swallow failure, and always also
//! log to stderr as a fallback. Mirrored here rather than pulling in a
//! notify-rust/D-Bus binding for what's fundamentally a "pop up a message,
//! and don't be a problem if that fails" operation.

use std::process::Command;

#[derive(Clone)]
pub struct Notifier {
    enabled: bool,
}

impl Notifier {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn send(&self, summary: &str, body: &str) {
        eprintln!("[ApexDaemon] {summary}: {body}");
        if !self.enabled {
            return;
        }
        // Best-effort only — a headless session (no mako/notify-send) must
        // never turn a notification failure into anything louder than the
        // stderr line above.
        let _ = Command::new("notify-send")
            .arg("--app-name=ApexDaemon")
            .arg(summary)
            .arg(body)
            .status();
    }
}
