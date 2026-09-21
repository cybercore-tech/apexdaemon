//! Machine-profile reminders for the dotfiles fleet.
//!
//! A profile is a small TOML record under `~/.config/apexdaemon/profiles/`.
//! It does not apply configuration or switch repositories; it only confirms
//! that the daemon is running on the machine the operator intended.

use crate::config::{expand_home, ProfilesConfig};
use crate::plugin::{tick_forever, Context, Plugin};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct ProfilesPlugin;

impl Plugin for ProfilesPlugin {
    fn name(&self) -> &'static str {
        "profiles"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.profiles.interval_secs.max(30));
            tick_forever("profiles", period, || tick(&ctx)).await
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct ProfileFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    public_manifest: Option<String>,
    #[serde(default)]
    private_manifest: Option<String>,
}

async fn tick(ctx: &Context) -> Result<()> {
    let cfg = &ctx.config.profiles;
    let report = inspect_profile(cfg, &ctx.config.dotfiles)?;
    let key = "profiles_active_report";
    let previous = ctx.state.get_str(key);

    if previous.as_deref() != Some(report.as_str()) {
        if report == "profile: healthy" {
            if previous.is_some() && cfg.remind_on_mismatch {
                ctx.notifier.send("ApexDaemon: profile restored", &report);
            }
        } else if cfg.remind_on_mismatch {
            ctx.notifier
                .send("ApexDaemon: profile needs attention", &report);
        }

        if !ctx.dry_run {
            ctx.state.set(key, Value::String(report));
        }
    }

    Ok(())
}

fn inspect_profile(
    cfg: &ProfilesConfig,
    dotfiles: &crate::config::DotfilesConfig,
) -> Result<String> {
    if !is_safe_profile_name(&cfg.active) {
        return Ok(format!(
            "profile: invalid active profile name `{}`",
            cfg.active
        ));
    }

    let path = expand_home(&cfg.directory).join(format!("{}.toml", cfg.active));
    if !path.is_file() {
        return Ok(format!(
            "profile: active profile missing ({})",
            path.display()
        ));
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("failed to read active profile {}: {e}", path.display()))?;
    let profile: ProfileFile = toml::from_str(&raw)
        .map_err(|e| anyhow!("failed to parse active profile {}: {e}", path.display()))?;
    let mut issues = Vec::new();

    if let Some(name) = profile.name.as_deref() {
        if name != cfg.active {
            issues.push(format!("name is `{name}`, expected `{}`", cfg.active));
        }
    }

    if let Some(expected_host) = profile.hostname.as_deref() {
        let actual_host = hostname();
        if expected_host != actual_host {
            issues.push(format!(
                "hostname is `{actual_host}`, expected `{expected_host}`"
            ));
        }
    }

    if let Some(public_manifest) = profile.public_manifest.as_deref() {
        if public_manifest != dotfiles.public_manifest {
            issues.push("public manifest does not match active daemon config".to_string());
        }
    }
    if let Some(private_manifest) = profile.private_manifest.as_deref() {
        if private_manifest != dotfiles.private_manifest {
            issues.push("private manifest does not match active daemon config".to_string());
        }
    }

    if issues.is_empty() {
        Ok("profile: healthy".to_string())
    } else {
        Ok(format!("profile `{}`: {}", cfg.active, issues.join("; ")))
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".to_string())
}

fn is_safe_profile_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_cannot_escape_profile_directory() {
        assert!(is_safe_profile_name("darkbox"));
        assert!(!is_safe_profile_name("../private"));
        assert!(!is_safe_profile_name("darkbox/other"));
        assert!(!is_safe_profile_name(""));
    }
}
