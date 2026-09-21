//! Polls a configured list of services and brings a down one back up by
//! *starting* it — never by killing/restarting an existing process.
//! Deliberate scope limit: `pkill -f` has taken down the user's own
//! :8080 cyberdeck twice before (wrong process matched the pattern), so
//! this plugin has no stop/kill path at all, only an opt-in `start_cmd`.
//! A hung-but-still-listening process is out of scope here; only "nothing
//! answered on the configured check" triggers a start attempt.

use crate::config::{HealthCheck, UnitScope, WatchedService};
use crate::plugin::{tick_forever, Context, Plugin};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

pub struct FleetHealthPlugin;

impl Plugin for FleetHealthPlugin {
    fn name(&self) -> &'static str {
        "fleet-health"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.fleet_health.interval_secs.max(5));
            tick_forever("fleet-health", period, || check_all(&ctx)).await
        })
    }
}

async fn check_all(ctx: &Context) -> anyhow::Result<()> {
    // A handful of checks (TCP connect / curl / systemctl), each already
    // bounded by its own short timeout — sequential is simple and plenty
    // fast; not worth fighting borrow lifetimes to run them concurrently.
    for svc in &ctx.config.fleet_health.services {
        check_one(ctx, svc).await;
    }
    Ok(())
}

async fn check_one(ctx: &Context, svc: &WatchedService) {
    let healthy = match &svc.check {
        HealthCheck::Tcp { addr } => check_tcp(addr).await,
        HealthCheck::Http { url } => check_http(url).await,
        HealthCheck::SystemdUnit { unit, scope } => check_systemd_unit(unit, scope).await,
    };

    if healthy {
        return;
    }

    eprintln!("[fleet-health] {} is down", svc.name);

    let Some(start_cmd) = &svc.start_cmd else {
        ctx.notifier.send(
            "ApexDaemon: service down",
            &format!(
                "{} failed its health check (no start_cmd configured — alert only)",
                svc.name
            ),
        );
        return;
    };

    let backoff_key = format!("fleet_health_last_restart_{}", svc.name);
    let backoff_secs = ctx.config.fleet_health.restart_backoff_secs;
    let now = chrono::Utc::now().timestamp();
    let last_attempt = ctx.state.get(&backoff_key).and_then(|v| v.as_i64());
    if !should_attempt_restart(last_attempt, now, backoff_secs) {
        // Already attempted a restart recently — don't hammer a
        // service that's down for a real reason.
        return;
    }

    if ctx.dry_run {
        println!(
            "[fleet-health] (dry-run) would run start_cmd for {}: {start_cmd}",
            svc.name
        );
        return;
    }

    println!("[fleet-health] starting {}: {start_cmd}", svc.name);
    let spawn_result = std::process::Command::new("sh")
        .arg("-c")
        .arg(start_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    ctx.state.set(&backoff_key, serde_json::Value::from(now));

    match spawn_result {
        Ok(_child) => {
            // Deliberately not `.wait()`-ed: dropping the handle here does
            // not kill the process (Rust's `Child` isn't killed on drop),
            // so this is a genuine fire-and-forget launch whether or not
            // `start_cmd` backgrounds itself.
            ctx.notifier.send(
                "ApexDaemon: service restarted",
                &format!("started {}", svc.name),
            );
        }
        Err(e) => {
            ctx.notifier.send(
                "ApexDaemon: restart failed",
                &format!("{} start_cmd failed: {e}", svc.name),
            );
        }
    }
}

async fn check_tcp(addr: &str) -> bool {
    let connect = tokio::net::TcpStream::connect(addr);
    matches!(
        tokio::time::timeout(Duration::from_secs(3), connect).await,
        Ok(Ok(_))
    )
}

async fn check_http(url: &str) -> bool {
    // No HTTP client dependency for what's effectively one GET with a
    // timeout — curl is present on every Omarchy install already.
    tokio::process::Command::new("curl")
        .args(["-fsS", "--max-time", "3", "-o", "/dev/null", url])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn check_systemd_unit(unit: &str, scope: &UnitScope) -> bool {
    // Read-only status query — works without sudo either way. `--user`
    // queries the invoking user's own session manager, a completely
    // separate unit namespace from the system manager plain `systemctl`
    // talks to; a unit installed in one scope never shows as active when
    // queried in the other.
    let mut cmd = tokio::process::Command::new("systemctl");
    if *scope == UnitScope::User {
        cmd.arg("--user");
    }
    cmd.args(["is-active", "--quiet", unit])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether enough time has passed since the last restart attempt (if
/// any) to try again. Pure decision logic, split out from the real
/// state-store read around it so the backoff rule itself is directly
/// unit-testable without needing a real `StateStore`.
fn should_attempt_restart(last_attempt: Option<i64>, now: i64, backoff_secs: u64) -> bool {
    match last_attempt {
        None => true,
        Some(last) => now - last >= backoff_secs as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_prior_attempt_always_allows_a_restart() {
        assert!(should_attempt_restart(None, 1_000, 300));
    }

    #[test]
    fn within_the_backoff_window_is_refused() {
        assert!(!should_attempt_restart(Some(1_000), 1_100, 300));
    }

    #[test]
    fn exactly_at_the_backoff_boundary_is_allowed() {
        assert!(should_attempt_restart(Some(1_000), 1_300, 300));
    }

    #[test]
    fn well_past_the_backoff_window_is_allowed() {
        assert!(should_attempt_restart(Some(1_000), 10_000, 300));
    }
}
