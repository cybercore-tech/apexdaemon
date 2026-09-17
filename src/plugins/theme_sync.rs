//! Watches Omarchy's active-theme state dir and regenerates a cybercore
//! CYBERGRID theme file from it. This is genuinely new — nothing in the
//! existing fleet (cyberdeck, cyberdesk, cybercore itself) does any
//! filesystem watching at all, so ApexDaemon is the first "theme changed"
//! signal source rather than duplicating one.
//!
//! Important limitation, surfaced via the notification sent on every
//! successful sync rather than left implicit: cybercore embeds its theme
//! schema at *compile time* via `build.rs` (`include_str!` of a merged
//! JSON). Writing a new file under `schema/themes/<family>/` does not
//! live-update any already-running cybercore-consuming binary — those
//! need a rebuild to pick up the new theme. This plugin's job ends at
//! "the source-of-truth file is up to date," not "every consumer already
//! sees it."

use crate::config::expand_home;
use crate::plugin::{Context, Plugin};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::time::Duration;

pub struct ThemeSyncPlugin;

impl Plugin for ThemeSyncPlugin {
    fn name(&self) -> &'static str {
        "theme-sync"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move { run(ctx).await })
    }
}

/// The "Darkstar theme-gen" shape `ansi2cybergrid.py` accepts as input.
#[derive(Serialize)]
struct ThemeGenInput {
    name: String,
    background: String,
    foreground: String,
    cursor: String,
    colors: Vec<String>,
}

/// `colors.toml`'s shape, as written by Omarchy under
/// `~/.local/state/omarchy/current/theme/colors.toml` — flat, a handful of
/// semantic keys plus a 16-slot ANSI palette (`color0`..`color15`).
/// Strictly simpler to parse than scraping `alacritty.toml`'s nested
/// `[colors.primary]`/`[colors.normal]`/etc for the same values.
#[derive(serde::Deserialize)]
struct OmarchyColors {
    background: String,
    foreground: String,
    cursor: String,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, String>,
}

impl OmarchyColors {
    fn ansi16(&self) -> anyhow::Result<Vec<String>> {
        (0..16)
            .map(|i| {
                self.extra
                    .get(&format!("color{i}"))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("colors.toml missing color{i}"))
            })
            .collect()
    }
}

fn state_theme_dir() -> PathBuf {
    expand_home("~/.local/state/omarchy/current")
}

async fn run(ctx: Context) -> anyhow::Result<()> {
    let watch_root = state_theme_dir();
    if !watch_root.exists() {
        anyhow::bail!(
            "{} does not exist — is this an Omarchy system?",
            watch_root.display()
        );
    }

    // Sync once immediately, so a theme switch that happened while
    // ApexDaemon was down (or simply starting up fresh) doesn't have to
    // wait for the next live file event.
    if let Err(e) = sync_once(&ctx).await {
        eprintln!("[theme-sync] initial sync failed: {e:#}");
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = tx.blocking_send(event);
        }
    })?;
    watcher.watch(&watch_root, RecursiveMode::Recursive)?;

    loop {
        rx.recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("theme watch channel closed unexpectedly"))?;

        // Omarchy's theme-set does `rm -rf` + `mv`, which fires several
        // events in quick succession for one logical theme change — wait
        // for a quiet period before re-syncing (same idea, same rough
        // duration, as the debounce added to OmNote's own ThemeWatcher for
        // the identical reason).
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
            // more events arrived within the quiet window — keep draining
        }

        if let Err(e) = sync_once(&ctx).await {
            eprintln!("[theme-sync] sync failed: {e:#}");
            ctx.notifier
                .send("ApexDaemon: theme sync failed", &format!("{e:#}"));
        }
    }
}

async fn sync_once(ctx: &Context) -> anyhow::Result<()> {
    let root = state_theme_dir();
    let colors_path = root.join("theme").join("colors.toml");
    let name_path = root.join("theme.name");

    if !colors_path.exists() {
        anyhow::bail!("{} not found — nothing to sync yet", colors_path.display());
    }

    let name = std::fs::read_to_string(&name_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "omarchy-live".to_string());

    let raw = std::fs::read_to_string(&colors_path)?;
    let colors: OmarchyColors = toml::from_str(&raw)?;
    let input = ThemeGenInput {
        name: name.clone(),
        background: colors.background.clone(),
        foreground: colors.foreground.clone(),
        cursor: colors.cursor.clone(),
        colors: colors.ansi16()?,
    };

    // Skip entirely if nothing changed since the last successful sync —
    // avoids re-running the converter (and firing a notification) on
    // every unrelated file event under the watched directory.
    let fingerprint = serde_json::to_string(&input)?;
    if ctx.state.get_str("theme_sync_last").as_deref() == Some(fingerprint.as_str()) {
        return Ok(());
    }

    if ctx.dry_run {
        println!(
            "[theme-sync] (dry-run) would sync theme '{name}' -> cybercore family '{}'",
            ctx.config.theme_sync.family
        );
        ctx.state
            .set("theme_sync_last", serde_json::Value::String(fingerprint));
        return Ok(());
    }

    let cybercore_root = expand_home(&ctx.config.theme_sync.cybercore_root);
    let script = cybercore_root.join("tools").join("ansi2cybergrid.py");
    if !script.exists() {
        anyhow::bail!("ansi2cybergrid.py not found at {}", script.display());
    }
    let target_dir = cybercore_root
        .join("schema")
        .join("themes")
        .join(&ctx.config.theme_sync.family);
    std::fs::create_dir_all(&target_dir)?;

    let tmp = std::env::temp_dir().join(format!("apexd-theme-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&input)?)?;

    let run_result = Command::new("python3")
        .arg(&script)
        .arg(&tmp)
        .arg("--slug")
        .arg(&name)
        .arg("--write")
        .arg(&target_dir)
        .arg("--force")
        .output();
    let _ = std::fs::remove_file(&tmp);
    let output = run_result?;

    if !output.status.success() {
        anyhow::bail!(
            "ansi2cybergrid.py failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for hook in &ctx.config.theme_sync.post_hooks {
        if let Err(e) = Command::new("sh").arg("-c").arg(hook).status() {
            eprintln!("[theme-sync] post_hook `{hook}` failed to run: {e}");
        }
    }

    ctx.state
        .set("theme_sync_last", serde_json::Value::String(fingerprint));
    ctx.notifier.send(
        "ApexDaemon: theme synced",
        &format!(
            "'{name}' -> cybercore/schema/themes/{}/{name}.json (rebuild consumers to pick it up)",
            ctx.config.theme_sync.family
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_omarchy_colors_toml_into_theme_gen_input() {
        let raw = r##"
            accent = "#BE3F50"
            cursor = "#ff7f41"
            foreground = "#14B9B5"
            background = "#0e091d"
            selection_foreground = "#0e091d"
            selection_background = "#14B9B5"
            color0 = "#000000"
            color1 = "#c8e967"
            color2 = "#111111"
            color3 = "#222222"
            color4 = "#333333"
            color5 = "#444444"
            color6 = "#555555"
            color7 = "#666666"
            color8 = "#777777"
            color9 = "#888888"
            color10 = "#999999"
            color11 = "#aaaaaa"
            color12 = "#bbbbbb"
            color13 = "#cccccc"
            color14 = "#dddddd"
            color15 = "#11AEB3"
        "##;
        let colors: OmarchyColors = toml::from_str(raw).unwrap();
        assert_eq!(colors.background, "#0e091d");
        assert_eq!(colors.foreground, "#14B9B5");
        let ansi = colors.ansi16().unwrap();
        assert_eq!(ansi.len(), 16);
        assert_eq!(ansi[0], "#000000");
        assert_eq!(ansi[15], "#11AEB3");
    }

    #[test]
    fn missing_ansi_slot_is_an_error_not_a_panic() {
        let raw = r##"
            cursor = "#ff7f41"
            foreground = "#14B9B5"
            background = "#0e091d"
            color0 = "#000000"
        "##;
        let colors: OmarchyColors = toml::from_str(raw).unwrap();
        assert!(colors.ansi16().is_err());
    }
}
