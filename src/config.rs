//! Config shape mirrors wraithflow's convention: `#[serde(default = "fn")]`
//! on every optional field (not a `Default` impl) so each default lives
//! right next to the field it belongs to, documented in place. One
//! top-level table per plugin, plus a `[plugins]` table that's just
//! enable/disable switches — keeps "is this on" separate from "how is it
//! configured" so toggling a plugin off doesn't require deleting its config.

use serde::Deserialize;
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
pub struct PluginToggles {
    #[serde(default = "default_true")]
    pub theme_sync: bool,
    #[serde(default = "default_true")]
    pub fleet_health: bool,
    #[serde(default = "default_true")]
    pub security: bool,
    #[serde(default = "default_true")]
    pub vault_backup: bool,
    #[serde(default = "default_true")]
    pub repo_housekeeping: bool,
}

// Deliberately NOT `#[derive(Default)]`: that would give every field
// `bool::default()` (false), silently disabling every plugin for anyone
// who constructs `AppConfig::default()` directly (the "no config file
// found" fallback in main.rs) — a materially different result from
// parsing an empty/absent TOML file, which is supposed to mean "on for
// everything" per the `#[serde(default = "default_true")]` above. This
// impl keeps both paths in agreement.
impl Default for PluginToggles {
    fn default() -> Self {
        Self {
            theme_sync: true,
            fleet_health: true,
            security: true,
            vault_backup: true,
            repo_housekeeping: true,
        }
    }
}

fn default_cybercore_root() -> String {
    "~/.sysops/cybercore".to_string()
}

fn default_family() -> String {
    "omarchy-live".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct ThemeSyncConfig {
    /// Where cybercore lives — needed to find its ansi2cybergrid.py
    /// converter and schema/themes/ target directory.
    #[serde(default = "default_cybercore_root")]
    pub cybercore_root: String,
    /// The family folder auto-synced themes get written into
    /// (schema/themes/<family>/<slug>.json), kept separate from the 72
    /// hand-curated themes so this never clobbers one of those.
    #[serde(default = "default_family")]
    pub family: String,
    /// Extra shell commands to run after a successful regen (e.g. to
    /// eventually poke a running app to reload). Empty by default since
    /// nothing in the fleet supports live-reload of cybercore data yet —
    /// consumers still need a rebuild to pick up new theme JSON.
    #[serde(default)]
    pub post_hooks: Vec<String>,
}

impl Default for ThemeSyncConfig {
    fn default() -> Self {
        Self {
            cybercore_root: default_cybercore_root(),
            family: default_family(),
            post_hooks: Vec::new(),
        }
    }
}

fn default_fleet_interval() -> u64 {
    60
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "check", rename_all = "kebab-case")]
pub enum HealthCheck {
    Tcp { addr: String },
    Http { url: String },
    SystemdUnit { unit: String },
}

#[derive(Deserialize, Debug, Clone)]
pub struct WatchedService {
    pub name: String,
    #[serde(flatten)]
    pub check: HealthCheck,
    /// Shell command that (re)starts the service if it's found down.
    /// Absent means "alert only, never touch it" — deliberately opt-in.
    pub start_cmd: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FleetHealthConfig {
    #[serde(default = "default_fleet_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub services: Vec<WatchedService>,
    /// Minimum gap between two restart attempts for the *same* service,
    /// so a service that's down for a real reason (not just slow to
    /// start) doesn't get hammered with restart attempts every tick.
    #[serde(default = "default_restart_backoff")]
    pub restart_backoff_secs: u64,
}

fn default_restart_backoff() -> u64 {
    300
}

impl Default for FleetHealthConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_fleet_interval(),
            services: Vec::new(),
            restart_backoff_secs: default_restart_backoff(),
        }
    }
}

fn default_security_interval() -> u64 {
    120
}

fn default_security_unit() -> String {
    "vortexwall".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct SecurityConfig {
    #[serde(default = "default_security_interval")]
    pub interval_secs: u64,
    /// The unit to watch. Named generically in config (not hardcoded to
    /// vortexwall in code) since vortexwall is an explicitly temporary
    /// learning project ahead of fail2ban — swapping the watched unit
    /// later should be a one-line config change, not a code change.
    #[serde(default = "default_security_unit")]
    pub unit: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_security_interval(),
            unit: default_security_unit(),
        }
    }
}

fn default_vault_interval() -> u64 {
    1800
}

fn default_vault_path() -> String {
    "~/Vaults/darknotes".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct VaultBackupConfig {
    #[serde(default = "default_vault_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_vault_path")]
    pub path: String,
    /// Off by default: pushing is the one operation here that can hit the
    /// network/a remote, so it's an explicit opt-in even though commits
    /// (purely local) happen automatically once this plugin is enabled.
    #[serde(default)]
    pub push: bool,
}

impl Default for VaultBackupConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_vault_interval(),
            path: default_vault_path(),
            push: false,
        }
    }
}

fn default_repo_interval() -> u64 {
    3600
}

fn default_cyberfleet_config() -> String {
    "~/.config/cyberfleet/config.json".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct RepoHousekeepingConfig {
    #[serde(default = "default_repo_interval")]
    pub interval_secs: u64,
    /// If this file exists, its `roots`/`ignore`/`max_depth` are reused
    /// instead of duplicating a second repo list in this config — one
    /// source of truth for "which repos do I track" shared with cyberfleet.
    #[serde(default = "default_cyberfleet_config")]
    pub cyberfleet_config: String,
    /// Fallback roots used only if `cyberfleet_config` doesn't exist.
    #[serde(default)]
    pub roots: Vec<String>,
    /// "owner/repo#123" entries to poll for state changes via `gh`.
    #[serde(default)]
    pub watch_prs: Vec<String>,
}

impl Default for RepoHousekeepingConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_repo_interval(),
            cyberfleet_config: default_cyberfleet_config(),
            roots: Vec::new(),
            watch_prs: Vec::new(),
        }
    }
}

fn default_true_notify() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
pub struct GeneralConfig {
    #[serde(default = "default_true_notify")]
    pub notify: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self { notify: true }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub plugins: PluginToggles,
    #[serde(default)]
    pub theme_sync: ThemeSyncConfig,
    #[serde(default)]
    pub fleet_health: FleetHealthConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub vault_backup: VaultBackupConfig,
    #[serde(default)]
    pub repo_housekeeping: RepoHousekeepingConfig,
}

/// `--config`, then `$XDG_CONFIG_HOME/apexdaemon/config.toml`, then
/// `~/.config/apexdaemon/config.toml`, then `./config.toml` — same
/// resolution order as wraithflow's `default_config_path()`.
pub fn default_config_path() -> Option<PathBuf> {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok()?;
    let candidates = [
        config_home.join("apexdaemon").join("config.toml"),
        PathBuf::from("config.toml"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

pub fn load(path: &Path) -> anyhow::Result<AppConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    toml::from_str(&raw).map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

/// `~` expansion — every path-shaped config value goes through this rather
/// than assuming the shell already expanded it, since these strings come
/// from a TOML file, not argv.
pub fn expand_home(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_default_matches_empty_toml_for_plugin_toggles() {
        // Regression test: `AppConfig::default()` (used by main.rs when no
        // config file exists at all) must agree with what parsing an
        // empty TOML file produces — these used to diverge because
        // `PluginToggles` derived `Default` (false for every bool)
        // instead of matching its `#[serde(default = "default_true")]`.
        let from_empty_toml: AppConfig = toml::from_str("").unwrap();
        let plain_default = AppConfig::default();
        assert!(plain_default.plugins.theme_sync);
        assert!(plain_default.plugins.fleet_health);
        assert!(plain_default.plugins.security);
        assert!(plain_default.plugins.vault_backup);
        assert!(plain_default.plugins.repo_housekeeping);
        assert_eq!(
            from_empty_toml.plugins.theme_sync,
            plain_default.plugins.theme_sync
        );
        assert_eq!(
            from_empty_toml.plugins.security,
            plain_default.plugins.security
        );
    }

    #[test]
    fn defaults_apply_to_empty_config() {
        let cfg: AppConfig = toml::from_str("").unwrap();
        assert!(cfg.plugins.theme_sync);
        assert!(cfg.plugins.fleet_health);
        assert_eq!(cfg.fleet_health.interval_secs, 60);
        assert_eq!(cfg.security.unit, "vortexwall");
        assert!(!cfg.vault_backup.push);
        assert_eq!(cfg.vault_backup.path, "~/Vaults/darknotes");
    }

    #[test]
    fn plugin_can_be_disabled_without_touching_its_config() {
        let toml_str = r#"
            [plugins]
            security = false
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.plugins.security);
        assert!(cfg.plugins.theme_sync); // untouched siblings stay enabled
        assert_eq!(cfg.security.unit, "vortexwall"); // config itself still defaults fine
    }

    #[test]
    fn watched_service_check_variants_parse() {
        let toml_str = r#"
            [[fleet_health.services]]
            name = "cyberdeck"
            check = "tcp"
            addr = "127.0.0.1:8080"
            start_cmd = "cyberdeck"

            [[fleet_health.services]]
            name = "cyberdesk"
            check = "http"
            url = "http://127.0.0.1:8765/healthz"

            [[fleet_health.services]]
            name = "vortexwall"
            check = "systemd-unit"
            unit = "vortexwall"
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.fleet_health.services.len(), 3);
        assert!(matches!(
            cfg.fleet_health.services[0].check,
            HealthCheck::Tcp { .. }
        ));
        assert!(matches!(
            cfg.fleet_health.services[1].check,
            HealthCheck::Http { .. }
        ));
        assert!(matches!(
            cfg.fleet_health.services[2].check,
            HealthCheck::SystemdUnit { .. }
        ));
        assert_eq!(
            cfg.fleet_health.services[0].start_cmd.as_deref(),
            Some("cyberdeck")
        );
        assert_eq!(cfg.fleet_health.services[1].start_cmd, None);
    }

    #[test]
    fn expand_home_handles_tilde_and_plain_paths() {
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(
            expand_home("~/Vaults/darknotes"),
            PathBuf::from("/home/testuser/Vaults/darknotes")
        );
        assert_eq!(expand_home("/etc/passwd"), PathBuf::from("/etc/passwd"));
    }
}
