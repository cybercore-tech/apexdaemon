//! Read-only dotfiles doctor for the public and private bare repositories.
//!
//! Slice 1 deliberately stops at inspection and notification. It never
//! commits, pushes, checks out, or rewrites a user's home directory. The
//! manifests are the explicit allow-list that keeps `config add -A` from
//! becoming the daemon's policy.

use crate::config::{expand_home, DotfilesConfig};
use crate::plugin::{tick_forever, Context, Plugin};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tokio::process::Command;

pub struct DotfilesPlugin;

impl Plugin for DotfilesPlugin {
    fn name(&self) -> &'static str {
        "dotfiles"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.dotfiles.interval_secs.max(30));
            tick_forever("dotfiles", period, || tick(&ctx)).await
        })
    }
}

#[derive(Debug, Default)]
struct RepoReport {
    label: &'static str,
    issues: Vec<String>,
}

impl RepoReport {
    fn issue(&mut self, message: impl Into<String>) {
        self.issues.push(message.into());
    }

    fn render(&self) -> String {
        if self.issues.is_empty() {
            format!("{}: clean", self.label)
        } else {
            format!("{}: {}", self.label, self.issues.join("; "))
        }
    }
}

async fn tick(ctx: &Context) -> Result<()> {
    let cfg = &ctx.config.dotfiles;
    let mut reports = Vec::new();
    reports.push(inspect_repo("public", &cfg.public_git_dir, &cfg.public_manifest, cfg).await);
    reports.push(inspect_repo("private", &cfg.private_git_dir, &cfg.private_manifest, cfg).await);

    let report = reports
        .iter()
        .map(RepoReport::render)
        .collect::<Vec<_>>()
        .join("\n");
    let key = "dotfiles_slice1_report";
    let previous = ctx.state.get_str(key);

    if previous.as_deref() != Some(report.as_str()) {
        let has_issues = reports.iter().any(|r| !r.issues.is_empty());
        if has_issues {
            ctx.notifier
                .send("ApexDaemon: dotfiles need attention", &report);
        } else if previous.is_some() {
            ctx.notifier.send("ApexDaemon: dotfiles healthy", &report);
        }

        if !ctx.dry_run {
            ctx.state.set(key, Value::String(report.clone()));
        }
    }

    if cfg.auto_commit || cfg.auto_push {
        println!(
            "[dotfiles] auto_commit/auto_push are reserved for a later slice; no write action taken"
        );
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotMetadata {
    id: String,
    created_utc: String,
    profile: String,
    files: Vec<SnapshotFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    repo: String,
    path: String,
    #[serde(default = "default_snapshot_mode")]
    mode: u32,
}

fn default_snapshot_mode() -> u32 {
    0o600
}

/// Create a local checkpoint from the explicit public/private manifests.
/// This is deliberately a direct command action, not a periodic plugin write.
pub fn create_snapshot(cfg: &DotfilesConfig, profile: &str, dry_run: bool) -> Result<String> {
    let id = next_snapshot_id(&cfg.snapshot_dir)?;
    let snapshot_root = expand_home(&cfg.snapshot_dir).join(&id);
    let sources = snapshot_sources(cfg)?;
    let mut files = Vec::new();

    for (repo, manifest) in sources {
        for path in read_manifest(&manifest)? {
            let relative = Path::new(&path);
            if !is_safe_relative_path(relative) {
                return Err(anyhow!("manifest path is not relative and safe: {path}"));
            }
            let source = home_dir().join(relative);
            if !source.is_file() {
                return Err(anyhow!(
                    "cannot snapshot missing file {} ({path})",
                    source.display()
                ));
            }

            files.push(SnapshotFile {
                repo: repo.to_string(),
                path,
                mode: file_mode(&source),
            });
        }
    }

    if dry_run {
        return Ok(format!(
            "[dry-run] would create dotfiles snapshot {id} with {} file(s)",
            files.len()
        ));
    }

    secure_create_dir(&snapshot_root)?;
    for file in &files {
        let source = home_dir().join(&file.path);
        let destination = snapshot_root.join(&file.repo).join(&file.path);
        copy_file_secure(&source, &destination, file.mode)?;
    }

    let metadata = SnapshotMetadata {
        id: id.clone(),
        created_utc: Utc::now().to_rfc3339(),
        profile: profile.to_string(),
        files,
    };
    let metadata_path = snapshot_root.join("metadata.json");
    fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
    secure_file_mode(&metadata_path, 0o600)?;
    enforce_snapshot_retention(&expand_home(&cfg.snapshot_dir), cfg.max_snapshots)?;

    Ok(format!(
        "created dotfiles snapshot {id} ({} file(s))",
        metadata.files.len()
    ))
}

/// Restore files from one local checkpoint. Extra live files are never
/// deleted, and this function never invokes Git or touches a remote.
pub fn rollback_snapshot(cfg: &DotfilesConfig, id: &str, dry_run: bool) -> Result<String> {
    if !is_safe_snapshot_id(id) {
        return Err(anyhow!("invalid snapshot id: {id}"));
    }
    let root = expand_home(&cfg.snapshot_dir).join(id);
    let metadata_path = root.join("metadata.json");
    let metadata: SnapshotMetadata = serde_json::from_slice(
        &fs::read(&metadata_path)
            .map_err(|e| anyhow!("failed to read snapshot {}: {e}", metadata_path.display()))?,
    )?;

    for file in &metadata.files {
        if file.repo != "public" && file.repo != "private" {
            return Err(anyhow!("snapshot contains an unknown repository label"));
        }
        let relative = Path::new(&file.path);
        if !is_safe_relative_path(relative) {
            return Err(anyhow!("snapshot contains an unsafe path: {}", file.path));
        }
        let source = root.join(&file.repo).join(relative);
        if !source.is_file() {
            return Err(anyhow!("snapshot file is missing: {}", source.display()));
        }
        let destination = home_dir().join(relative);
        if destination
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(anyhow!(
                "refusing to overwrite symlink: {}",
                destination.display()
            ));
        }
        if dry_run {
            println!("[dry-run] would restore {}", destination.display());
        } else {
            copy_file_secure(&source, &destination, file.mode)?;
        }
    }

    Ok(if dry_run {
        format!("[dry-run] would restore snapshot {id}")
    } else {
        format!("restored snapshot {id} ({} file(s))", metadata.files.len())
    })
}

fn snapshot_sources(cfg: &DotfilesConfig) -> Result<Vec<(&'static str, PathBuf)>> {
    Ok(vec![
        ("public", expand_manifest(&cfg.public_manifest)?),
        ("private", expand_manifest(&cfg.private_manifest)?),
    ])
}

fn expand_manifest(value: &str) -> Result<PathBuf> {
    let path = expand_home(value);
    if !path.is_file() {
        return Err(anyhow!("manifest missing: {}", path.display()));
    }
    Ok(path)
}

fn next_snapshot_id(snapshot_dir: &str) -> Result<String> {
    let base = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let path = expand_home(snapshot_dir).join(&base);
    if !path.exists() {
        return Ok(base);
    }
    Ok(format!("{base}-{}", std::process::id()))
}

fn is_safe_snapshot_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains('/') && !id.contains('\\')
}

fn secure_create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    secure_file_mode(path, 0o700)?;
    Ok(())
}

fn copy_file_secure(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    secure_file_mode(destination, mode)?;
    Ok(())
}

fn enforce_snapshot_retention(snapshot_dir: &Path, max_snapshots: usize) -> Result<()> {
    let mut snapshots = fs::read_dir(snapshot_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    snapshots.sort_by_key(|entry| entry.file_name());
    let keep = max_snapshots.max(1);
    while snapshots.len() > keep {
        let oldest = snapshots.remove(0);
        fs::remove_dir_all(oldest.path())?;
    }
    Ok(())
}

fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(0o600)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o600
    }
}

fn secure_file_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

async fn inspect_repo(
    label: &'static str,
    git_dir_value: &str,
    manifest_value: &str,
    cfg: &DotfilesConfig,
) -> RepoReport {
    let mut report = RepoReport {
        label,
        issues: Vec::new(),
    };
    let git_dir = expand_home(git_dir_value);
    let manifest = expand_home(manifest_value);
    let home = home_dir();

    if !git_dir.is_dir() {
        report.issue(format!("git dir missing ({})", git_dir.display()));
        return report;
    }
    if !manifest.is_file() {
        report.issue(format!("manifest missing ({})", manifest.display()));
        return report;
    }

    let entries = match read_manifest(&manifest) {
        Ok(entries) => entries,
        Err(e) => {
            report.issue(format!("manifest error: {e}"));
            return report;
        }
    };

    // Run from the worktree root so Git does not silently limit a bare-repo
    // index query to ApexDaemon's source subdirectory. The index includes
    // staged files, which is exactly what a pre-commit doctor should inspect.
    let tracked = match git_lines(&git_dir, &["ls-files"]).await {
        Ok(lines) => lines.into_iter().collect::<BTreeSet<_>>(),
        Err(e) => {
            report.issue(format!("cannot read tracked files: {e}"));
            BTreeSet::new()
        }
    };

    match git_lines(
        &git_dir,
        &["status", "--porcelain=v1", "--untracked-files=no"],
    )
    .await
    {
        Ok(lines) if !lines.is_empty() => {
            let shown = lines
                .iter()
                .take(8)
                .map(|line| status_path(line))
                .collect::<Vec<_>>()
                .join(", ");
            let suffix = if lines.len() > 8 { ", …" } else { "" };
            report.issue(format!(
                "{} working-tree change(s): {shown}{suffix}",
                lines.len()
            ));
        }
        Ok(_) => {}
        Err(e) => report.issue(format!("status check failed: {e}")),
    }

    let mut listed = BTreeSet::new();
    for entry in entries {
        if !listed.insert(entry.clone()) {
            report.issue(format!("manifest duplicates {entry}"));
            continue;
        }

        let relative = Path::new(&entry);
        if !is_safe_relative_path(relative) {
            report.issue(format!("manifest path is not relative and safe: {entry}"));
            continue;
        }

        if !home.join(relative).exists() {
            report.issue(format!("manifest file missing from home: {entry}"));
        }
        if !tracked.contains(&entry) {
            report.issue(format!("manifest file is not tracked: {entry}"));
        }

        if cfg.security.scan_secrets {
            scan_file_for_secrets(&home.join(relative), &entry, &mut report);
        }
    }

    for extra in tracked.difference(&listed).take(8) {
        if !matches_exclude(extra, &cfg.security.exclude) {
            report.issue(format!("tracked file is outside manifest: {extra}"));
        }
    }

    report
}

async fn git_lines(git_dir: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .arg("--work-tree")
        .arg(home_dir())
        .current_dir(home_dir())
        .args(args)
        .output()
        .await
        .map_err(|e| anyhow!("failed to run git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "git {} failed{}",
            args.join(" "),
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn read_manifest(path: &Path) -> Result<Vec<String>> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect())
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn status_path(line: &str) -> String {
    line.get(3..).unwrap_or(line).trim().to_string()
}

fn matches_exclude(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix('*') {
            path.starts_with(prefix)
        } else if let Some(suffix) = pattern.strip_prefix('*') {
            path.ends_with(suffix)
        } else if let Some(prefix) = pattern.strip_suffix('/') {
            path == prefix || path.starts_with(&format!("{prefix}/"))
        } else {
            path == pattern || path.starts_with(&format!("{pattern}/"))
        }
    })
}

fn scan_file_for_secrets(path: &Path, display_path: &str, report: &mut RepoReport) {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };

    let mut signals = BTreeSet::new();
    for line in contents.lines() {
        if line.contains("sk-or-") || line.contains("OPENROUTER_API_KEY=") {
            signals.insert("OpenRouter key");
        }
        if line.contains("AIzaSy") || line.contains("GEMINI_API_KEY=") {
            signals.insert("Google/Gemini API key");
        }
        if line.contains("ghp_") || line.contains("github_pat_") {
            signals.insert("GitHub token");
        }
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY-----") {
            signals.insert("private key");
        }
        if line.contains("AWS_ACCESS_KEY_ID=") || line.contains("AWS_SECRET_ACCESS_KEY=") {
            signals.insert("AWS credential");
        }
        if line.contains("DISCORD_TOKEN=") || line.contains("DISCORD_WEBHOOK") {
            signals.insert("Discord credential");
        }
    }

    if !signals.is_empty() {
        report.issue(format!(
            "secret-like material in {display_path} ({})",
            signals.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_ignores_comments_and_blank_lines() {
        let path =
            std::env::temp_dir().join(format!("apexd-dotfiles-manifest-{}", std::process::id()));
        std::fs::write(&path, "# comment\n\n.config/hypr/hyprland.conf\n").unwrap();
        assert_eq!(
            read_manifest(&path).unwrap(),
            vec![".config/hypr/hyprland.conf"]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unsafe_manifest_paths_are_rejected() {
        assert!(is_safe_relative_path(Path::new(".config/foo")));
        assert!(!is_safe_relative_path(Path::new("/etc/passwd")));
        assert!(!is_safe_relative_path(Path::new("../outside")));
    }

    #[test]
    fn excludes_support_exact_prefix_and_suffix_patterns() {
        let patterns = vec!["Vaults/".into(), "*.pem".into(), ".bashrc".into()];
        assert!(matches_exclude("Vaults/darknotes/file", &patterns));
        assert!(matches_exclude("private.pem", &patterns));
        assert!(matches_exclude(".bashrc", &patterns));
        assert!(!matches_exclude(".config/hypr/hyprland.conf", &patterns));
    }

    #[test]
    fn secret_scan_reports_kind_without_secret_value() {
        let path =
            std::env::temp_dir().join(format!("apexd-dotfiles-secret-{}", std::process::id()));
        std::fs::write(&path, "OPENROUTER_API_KEY=sk-or-test-value\n").unwrap();
        let mut report = RepoReport {
            label: "test",
            issues: Vec::new(),
        };
        scan_file_for_secrets(&path, "test.env", &mut report);
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("OpenRouter key"));
        assert!(!report.issues[0].contains("sk-or-test-value"));
        let _ = std::fs::remove_file(path);
    }
}
