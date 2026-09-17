//! Watches a configured systemd unit (vortexwall today) for two things:
//! whether it's still running, and ban events in its journal. This plugin
//! is deliberately observe-and-alert only — it never starts the unit
//! itself. Auto-starting a firewall/auth-ban daemon without a human
//! looking is a different risk category than restarting a stalled web
//! server, and vortexwall itself is explicitly a temporary learning
//! project ahead of fail2ban — so the watched unit lives in config, not
//! code, and swapping it later is a one-line change.

use crate::plugin::{tick_forever, Context, Plugin};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct SecurityPlugin;

impl Plugin for SecurityPlugin {
    fn name(&self) -> &'static str {
        "security"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move { run(ctx).await })
    }
}

async fn run(ctx: Context) -> anyhow::Result<()> {
    let unit = ctx.config.security.unit.clone();

    let status_ctx = ctx.clone();
    let status_unit = unit.clone();
    let period = Duration::from_secs(ctx.config.security.interval_secs.max(10));
    let status_loop = tick_forever("security", period, move || {
        check_unit_status(status_ctx.clone(), status_unit.clone())
    });

    let tail_ctx = ctx.clone();
    let tail_unit = unit.clone();
    let tail_loop = tail_bans(tail_ctx, tail_unit);

    tokio::select! {
        res = status_loop => res,
        res = tail_loop => res,
    }
}

async fn check_unit_status(ctx: Context, unit: String) -> anyhow::Result<()> {
    let active = tokio::process::Command::new("systemctl")
        .args(["is-active", "--quiet", &unit])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    let key = format!("security_unit_active_{unit}");
    let was_active = ctx.state.get(&key).and_then(|v| v.as_bool());

    // Only notify on an actual transition, not on every tick — otherwise
    // "the firewall daemon is down" becomes background noise the moment
    // it stays down for more than one interval.
    match (was_active, active) {
        (Some(true), false) | (None, false) => {
            ctx.notifier.send(
                "ApexDaemon: security watch",
                &format!("{unit} is not active — auth-failure banning is not running"),
            );
        }
        (Some(false), true) => {
            ctx.notifier.send(
                "ApexDaemon: security watch",
                &format!("{unit} is active again"),
            );
        }
        _ => {}
    }

    ctx.state.set(&key, serde_json::Value::Bool(active));
    Ok(())
}

/// Mirrors vortexwall's own `watch_service`: tail the unit's journal,
/// restart the tail with a short backoff if `journalctl` itself exits.
async fn tail_bans(ctx: Context, unit: String) -> anyhow::Result<()> {
    loop {
        let child = tokio::process::Command::new("journalctl")
            .args(["-f", "-u", &unit, "-o", "cat", "--since", "now"])
            .stdout(std::process::Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[security] failed to spawn journalctl for {unit}: {e}");
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        let stdout = child.stdout.take().expect("journalctl stdout was piped");
        let mut lines = BufReader::new(stdout).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            // vortexwall logs successful bans as `[BANNED] <ip> for <n>s
            // (threshold <t> reached)` — surface those as-is rather than
            // re-parsing the IP back out, so the notification always
            // matches whatever vortexwall actually decided to log.
            if line.contains("[BANNED]") {
                ctx.notifier.send("ApexDaemon: IP banned", &line);
            }
        }

        eprintln!("[security] journalctl for {unit} exited — restarting in 10s");
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
