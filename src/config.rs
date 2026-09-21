//! Config shape mirrors wraithflow's convention: `#[serde(default = "fn")]`
//! on every optional field (not a `Default` impl) so each default lives
//! right next to the field it belongs to, documented in place. One
//! top-level table per plugin, plus a `[plugins]` table that's just
//! enable/disable switches — keeps "is this on" separate from "how is it
//! configured" so toggling a plugin off doesn't require deleting its config.
//! The dotfiles doctor is intentionally opt-in because it needs explicit
//! manifests before it can report a useful policy.

use serde::Deserialize;
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
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
    /// Opt-in until manifests have been created for the user's public and
    /// private dotfiles repositories.
    #[serde(default = "default_false")]
    pub dotfiles: bool,
    /// Opt-in machine-profile validation and reminders.
    #[serde(default = "default_false")]
    pub profiles: bool,
    /// Opt-in local asset validation and read-only GitHub Actions reminders.
    #[serde(default = "default_false")]
    pub validation: bool,
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
            dotfiles: false,
            profiles: false,
            validation: false,
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

#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UnitScope {
    #[default]
    System,
    User,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "check", rename_all = "kebab-case")]
pub enum HealthCheck {
    Tcp {
        addr: String,
    },
    Http {
        url: String,
    },
    SystemdUnit {
        unit: String,
        /// `systemctl --user` vs. plain `systemctl`. Defaults to `system`
        /// since that's the historical/common case — a `--user` unit only
        /// resolves under the invoking user's own session bus, so this
        /// must match how the unit is actually installed or the check
        /// silently always reads "not active", regardless of real health.
        #[serde(default)]
        scope: UnitScope,
    },
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

fn default_dotfiles_interval() -> u64 {
    300
}

fn default_public_git_dir() -> String {
    "~/.dotfiles".to_string()
}

fn default_private_git_dir() -> String {
    "~/.dotfiles-private".to_string()
}

fn default_public_manifest() -> String {
    "~/.config/apexdaemon/public-files.list".to_string()
}

fn default_private_manifest() -> String {
    "~/.config/apexdaemon/private-files.list".to_string()
}

fn default_snapshot_dir() -> String {
    "~/.local/state/apexdaemon/dotfiles-snapshots".to_string()
}

fn default_max_snapshots() -> usize {
    10
}

#[derive(Deserialize, Debug, Clone)]
pub struct DotfilesSecurityConfig {
    /// Scan every manifest-listed file, including private files. Exclusions
    /// only affect future manifest policy checks; they never disable secret
    /// scanning for an explicitly listed file.
    #[serde(default = "default_true")]
    pub scan_secrets: bool,
    /// Paths intentionally outside automatic manifest policy. This is not a
    /// secret-scan bypass; it is reserved for later manifest/reporting work.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for DotfilesSecurityConfig {
    fn default() -> Self {
        Self {
            scan_secrets: true,
            exclude: Vec::new(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct DotfilesConfig {
    #[serde(default = "default_dotfiles_interval")]
    pub interval_secs: u64,
    /// Reserved for the next slice. Slice 1 is read-only and reminder-only.
    #[serde(default)]
    pub auto_commit: bool,
    /// Reserved for the next slice. Slice 1 never pushes automatically.
    #[serde(default)]
    pub auto_push: bool,
    #[serde(default = "default_public_git_dir")]
    pub public_git_dir: String,
    #[serde(default = "default_private_git_dir")]
    pub private_git_dir: String,
    #[serde(default = "default_public_manifest")]
    pub public_manifest: String,
    #[serde(default = "default_private_manifest")]
    pub private_manifest: String,
    #[serde(default = "default_snapshot_dir")]
    pub snapshot_dir: String,
    #[serde(default = "default_max_snapshots")]
    pub max_snapshots: usize,
    #[serde(default)]
    pub security: DotfilesSecurityConfig,
}

impl Default for DotfilesConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_dotfiles_interval(),
            auto_commit: false,
            auto_push: false,
            public_git_dir: default_public_git_dir(),
            private_git_dir: default_private_git_dir(),
            public_manifest: default_public_manifest(),
            private_manifest: default_private_manifest(),
            snapshot_dir: default_snapshot_dir(),
            max_snapshots: default_max_snapshots(),
            security: DotfilesSecurityConfig::default(),
        }
    }
}

fn default_profile_interval() -> u64 {
    300
}

fn default_active_profile() -> String {
    "darkbox".to_string()
}

fn default_profile_directory() -> String {
    "~/.config/apexdaemon/profiles".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct ProfilesConfig {
    #[serde(default = "default_profile_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_active_profile")]
    pub active: String,
    #[serde(default = "default_profile_directory")]
    pub directory: String,
    #[serde(default = "default_true")]
    pub remind_on_mismatch: bool,
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_profile_interval(),
            active: default_active_profile(),
            directory: default_profile_directory(),
            remind_on_mismatch: true,
        }
    }
}

fn default_validation_interval() -> u64 {
    3600
}

fn default_validation_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
pub struct ValidationConfig {
    #[serde(default = "default_validation_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_validation_true")]
    pub check_shell: bool,
    #[serde(default = "default_validation_true")]
    pub check_toml: bool,
    #[serde(default = "default_validation_true")]
    pub check_lua: bool,
    #[serde(default = "default_validation_true")]
    pub check_svg: bool,
    #[serde(default = "default_validation_true")]
    pub check_readme_links: bool,
    #[serde(default)]
    pub watch_ci: bool,
    #[serde(default)]
    pub notify_success: bool,
    /// Optional workflow filename/name passed to `gh run list`.
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub repositories: Vec<String>,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_validation_interval(),
            check_shell: true,
            check_toml: true,
            check_lua: true,
            check_svg: true,
            check_readme_links: true,
            watch_ci: false,
            notify_success: false,
            workflow: None,
            repositories: Vec::new(),
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
    #[serde(default)]
    pub dotfiles: DotfilesConfig,
    #[serde(default)]
    pub profiles: ProfilesConfig,
    #[serde(default)]
    pub validation: ValidationConfig,
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
        assert!(!plain_default.plugins.dotfiles);
        assert!(!plain_default.plugins.profiles);
        assert!(!plain_default.plugins.validation);
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
        assert!(!cfg.plugins.validation);
        assert_eq!(cfg.validation.interval_secs, 3600);
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
        assert!(matches!(
            &cfg.fleet_health.services[2].check,
            HealthCheck::SystemdUnit { scope, .. } if *scope == UnitScope::System
        ));
    }

    #[test]
    fn systemd_unit_scope_defaults_to_system_but_user_is_explicit() {
        let toml_str = r#"
            [[fleet_health.services]]
            name = "cyberdeck"
            check = "systemd-unit"
            unit = "cyberdeck"
            scope = "user"

            [[fleet_health.services]]
            name = "vortexwall"
            check = "systemd-unit"
            unit = "vortexwall"
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            &cfg.fleet_health.services[0].check,
            HealthCheck::SystemdUnit { scope, .. } if *scope == UnitScope::User
        ));
        assert!(matches!(
            &cfg.fleet_health.services[1].check,
            HealthCheck::SystemdUnit { scope, .. } if *scope == UnitScope::System
        ));
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
