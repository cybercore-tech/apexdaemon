mod config;
mod notify;
mod plugin;
mod plugins;
mod state;

use clap::Parser;
use plugin::{Context, Plugin};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "apexdaemon",
    version = "0.1.0",
    about = "Plugin-style background automation: theme sync, fleet health, security watch, vault backup, repo housekeeping"
)]
struct Args {
    /// Path to config.toml. Defaults to $XDG_CONFIG_HOME/apexdaemon/config.toml,
    /// then ~/.config/apexdaemon/config.toml, then ./config.toml.
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Log what every plugin would do without actually restarting a
    /// service, committing/pushing the vault, fetching a repo, or writing
    /// theme files. Always start here after changing config.
    #[arg(long)]
    dry_run: bool,

    /// Print the registered plugins and whether each is enabled, then exit.
    #[arg(long)]
    list_plugins: bool,

    // --- systemd --user service control, same convention as
    // wraithflow/vortexwall's --admin flag ---
    #[arg(long)]
    admin: bool,
    #[arg(long, requires = "admin")]
    start: bool,
    #[arg(long, requires = "admin")]
    stop: bool,
    #[arg(long, requires = "admin")]
    restart: bool,
    #[arg(long, requires = "admin")]
    status: bool,
}

fn run_admin(args: &Args) -> std::io::Result<i32> {
    let action = match (args.start, args.stop, args.restart, args.status) {
        (true, false, false, false) => "start",
        (false, true, false, false) => "stop",
        (false, false, true, false) => "restart",
        (false, false, false, true) => "status",
        (false, false, false, false) => {
            eprintln!("--admin needs exactly one of --start, --stop, --restart, --status");
            return Ok(1);
        }
        _ => {
            eprintln!("--admin takes exactly one of --start, --stop, --restart, --status, not several at once");
            return Ok(1);
        }
    };

    // --user, not sudo systemctl: nothing this daemon does needs root, so
    // it's installed/managed as a user unit throughout.
    println!("[admin] running: systemctl --user {action} apexdaemon");
    let status = std::process::Command::new("systemctl")
        .args(["--user", action, "apexdaemon"])
        .status()?;
    Ok(status.code().unwrap_or(1))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    if args.admin {
        return match run_admin(&args) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("[admin] error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let config_path = match args.config.clone().or_else(config::default_config_path) {
        Some(p) => p,
        None => {
            eprintln!("[Configuration] No config file found — checked --config, $XDG_CONFIG_HOME/apexdaemon, ~/.config/apexdaemon, ./config.toml. Using built-in defaults.");
            std::path::PathBuf::new()
        }
    };

    let app_config = if config_path.as_os_str().is_empty() {
        config::AppConfig::default()
    } else {
        match config::load(&config_path) {
            Ok(cfg) => {
                println!("[Configuration] loaded {}", config_path.display());
                cfg
            }
            Err(e) => {
                eprintln!("[Critical Failure] {e:#}");
                return ExitCode::FAILURE;
            }
        }
    };

    let state_path = config::expand_home("~/.local/state/apexdaemon/state.json");
    let ctx = Context {
        config: Arc::new(app_config.clone()),
        notifier: notify::Notifier::new(app_config.general.notify),
        state: state::StateStore::load(state_path),
        dry_run: args.dry_run,
    };

    let registry: Vec<(bool, Arc<dyn Plugin>)> = vec![
        (app_config.plugins.theme_sync, Arc::new(plugins::theme_sync::ThemeSyncPlugin)),
        (app_config.plugins.fleet_health, Arc::new(plugins::fleet_health::FleetHealthPlugin)),
        (app_config.plugins.security, Arc::new(plugins::security::SecurityPlugin)),
        (app_config.plugins.vault_backup, Arc::new(plugins::vault_backup::VaultBackupPlugin)),
        (
            app_config.plugins.repo_housekeeping,
            Arc::new(plugins::repo_housekeeping::RepoHousekeepingPlugin),
        ),
    ];

    if args.list_plugins {
        for (enabled, p) in &registry {
            println!("{:<20} {}", p.name(), if *enabled { "enabled" } else { "disabled" });
        }
        return ExitCode::SUCCESS;
    }

    if args.dry_run {
        println!("[Configuration] --dry-run: no service will be restarted, no commit/push/fetch/write will actually happen");
    }

    let enabled: Vec<Arc<dyn Plugin>> = registry
        .into_iter()
        .filter_map(|(on, p)| on.then_some(p))
        .collect();

    if enabled.is_empty() {
        eprintln!("[Configuration] every plugin is disabled — nothing to do, exiting");
        return ExitCode::SUCCESS;
    }

    println!(
        "[ApexDaemon] starting with plugins: {}",
        enabled.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
    );

    let mut handles = Vec::new();
    for p in enabled {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(supervise(p, ctx)));
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("[ApexDaemon] shutting down");
        }
        _ = futures_join_all(handles) => {
            // every supervised task loops forever by design (see
            // `supervise` below) — reaching here means something outside
            // that loop panicked unrecoverably in every plugin at once,
            // which realistically only happens if the async runtime
            // itself is going down.
            eprintln!("[ApexDaemon] all plugin tasks ended unexpectedly");
        }
    }

    ExitCode::SUCCESS
}

async fn futures_join_all(handles: Vec<tokio::task::JoinHandle<()>>) {
    for h in handles {
        let _ = h.await;
    }
}

/// Restarts a plugin's `run()` with a backoff if it ever returns — an
/// `Err`, or (shouldn't happen, since every plugin loops forever
/// internally) a clean return. Mirrors vortexwall's
/// `watch_service`/journalctl-restart-on-exit pattern.
async fn supervise(p: Arc<dyn Plugin>, ctx: Context) {
    let mut backoff = Duration::from_secs(5);
    let max_backoff = Duration::from_secs(120);
    loop {
        let name = p.name();
        match p.run(ctx.clone()).await {
            Ok(()) => {
                eprintln!("[{name}] exited cleanly (unexpected) — restarting in {}s", backoff.as_secs());
            }
            Err(e) => {
                eprintln!("[{name}] failed: {e:#} — restarting in {}s", backoff.as_secs());
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}
