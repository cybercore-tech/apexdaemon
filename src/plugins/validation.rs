//! Read-only local validation and GitHub Actions status reminders.
//!
//! The local side validates files explicitly listed by the dotfiles
//! manifests. It never executes a shell script, changes a file, or invokes a
//! formatter. The GitHub side uses the authenticated `gh` CLI's read-only
//! `run list` command; it never dispatches, cancels, reruns, commits, or
//! publishes anything.

use crate::config::{expand_home, ValidationConfig};
use crate::plugin::{tick_forever, Context, Plugin};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub struct ValidationPlugin;

impl Plugin for ValidationPlugin {
    fn name(&self) -> &'static str {
        "validation"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.validation.interval_secs.max(60));
            tick_forever("validation", period, || tick(&ctx)).await
        })
    }
}

#[derive(Debug, Default)]
struct Report {
    issues: Vec<String>,
    notes: BTreeSet<String>,
}

impl Report {
    fn issue(&mut self, message: impl Into<String>) {
        self.issues.push(message.into());
    }

    fn note(&mut self, message: impl Into<String>) {
        self.notes.insert(message.into());
    }

    fn render(&self) -> String {
        let mut lines = self.issues.clone();
        lines.extend(self.notes.iter().map(|note| format!("note: {note}")));
        if lines.is_empty() {
            "validation: healthy".to_string()
        } else {
            format!("validation: {}", lines.join("; "))
        }
    }
}

async fn tick(ctx: &Context) -> Result<()> {
    let cfg = &ctx.config.validation;
    let mut report = Report::default();
    let files = manifest_files(
        &ctx.config.dotfiles.public_manifest,
        &ctx.config.dotfiles.private_manifest,
        &mut report,
    )?;

    for path in &files {
        validate_file(path, cfg, &mut report).await;
    }

    if cfg.watch_ci {
        if cfg.repositories.is_empty() {
            report.issue("CI watch is enabled but no repositories are configured");
        } else {
            for repository in &cfg.repositories {
                check_ci(repository, cfg.workflow.as_deref(), &mut report).await;
            }
        }
    }

    let rendered = report.render();
    let key = "validation_slice3_report";
    let previous = ctx.state.get_str(key);
    if previous.as_deref() != Some(rendered.as_str()) {
        if report.issues.is_empty() {
            if previous.is_some() && cfg.notify_success {
                ctx.notifier
                    .send("ApexDaemon: validation restored", &rendered);
            }
        } else {
            ctx.notifier
                .send("ApexDaemon: validation needs attention", &rendered);
        }

        if !ctx.dry_run {
            ctx.state.set(key, Value::String(rendered));
        }
    }

    Ok(())
}

fn manifest_files(
    public_value: &str,
    private_value: &str,
    report: &mut Report,
) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for manifest_value in [public_value, private_value] {
        let manifest = expand_home(manifest_value);
        if !manifest.is_file() {
            report.issue(format!("manifest missing ({})", manifest.display()));
            continue;
        }
        let contents = std::fs::read_to_string(&manifest)
            .map_err(|e| anyhow!("failed to read {}: {e}", manifest.display()))?;
        for entry in contents.lines().map(str::trim) {
            if entry.is_empty() || entry.starts_with('#') {
                continue;
            }
            let relative = Path::new(entry);
            if !is_safe_relative_path(relative) {
                report.issue(format!("unsafe manifest path: {entry}"));
                continue;
            }
            files.insert(home_dir().join(relative));
        }
    }
    Ok(files.into_iter().collect())
}

async fn validate_file(path: &Path, cfg: &ValidationConfig, report: &mut Report) {
    if !path.is_file() {
        report.issue(format!("missing file ({})", display_path(path)));
        return;
    }

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();

    if cfg.check_shell && (extension == "sh" || name == ".bashrc" || name == ".bash_profile") {
        run_syntax_check("bash", &["-n"], path, report).await;
    }
    if cfg.check_toml && extension == "toml" {
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|raw| toml::from_str::<toml::Value>(&raw).map_err(|e| e.to_string()))
        {
            Ok(_) => {}
            Err(error) => report.issue(format!("{} TOML: {}", display_path(path), compact(&error))),
        }
    }
    if cfg.check_lua && extension == "lua" {
        if command_available("luac") {
            run_syntax_check("luac", &["-p"], path, report).await;
        } else {
            report.note("luac not installed; Lua syntax checks skipped");
        }
    }
    if cfg.check_svg && extension == "svg" {
        if command_available("xmllint") {
            run_syntax_check("xmllint", &["--noout"], path, report).await;
        } else if let Ok(raw) = std::fs::read_to_string(path) {
            if !raw.contains("<svg") || !raw.contains("</svg>") {
                report.issue(format!(
                    "{} does not contain a complete SVG root",
                    display_path(path)
                ));
            }
            report.note("xmllint not installed; SVG checks used a root-element fallback");
        } else {
            report.issue(format!("cannot read SVG ({})", display_path(path)));
        }
    }
    if cfg.check_readme_links && (extension == "md" || extension == "markdown") {
        check_readme_links(path, report);
    }
}

async fn run_syntax_check(command: &str, prefix_args: &[&str], path: &Path, report: &mut Report) {
    let output = Command::new(command)
        .args(prefix_args)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr);
            report.issue(format!(
                "{} check failed: {}",
                display_path(path),
                compact(&error)
            ));
        }
        Err(error) => report.issue(format!(
            "{command} failed for {}: {error}",
            display_path(path)
        )),
    }
}

fn command_available(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }

    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(command))
        .any(|candidate| candidate.is_file())
}

fn check_readme_links(path: &Path, report: &mut Report) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        report.issue(format!("cannot read README ({})", display_path(path)));
        return;
    };
    let mut rest = raw.as_str();
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(')') else { break };
        let mut target = rest[..end].trim();
        if target.starts_with('<') && target.ends_with('>') {
            target = &target[1..target.len() - 1];
        } else if let Some((first, _)) = target.split_once(char::is_whitespace) {
            target = first;
        }
        if is_local_link(target) {
            let target_path_value = target.split(['#', '?']).next().unwrap_or(target);
            let target_path = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target_path_value);
            if !target_path.exists() {
                report.issue(format!(
                    "{} links to missing {}",
                    display_path(path),
                    target_path_value
                ));
            }
        }
        rest = &rest[end + 1..];
    }
}

async fn check_ci(repository: &str, workflow: Option<&str>, report: &mut Report) {
    if !is_safe_repository(repository) {
        report.issue(format!("invalid GitHub repository name: {repository}"));
        return;
    }

    let mut command = Command::new("gh");
    command.args([
        "run",
        "list",
        "--repo",
        repository,
        "--limit",
        "1",
        "--json",
        "status,conclusion,workflowName,headBranch,url",
    ]);
    if let Some(workflow) = workflow.filter(|workflow| !workflow.is_empty()) {
        command.args(["--workflow", workflow]);
    }
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    let output = match output {
        Ok(output) => output,
        Err(error) => {
            report.issue(format!("{repository} CI check failed: {error}"));
            return;
        }
    };
    if !output.status.success() {
        let error = compact(&String::from_utf8_lossy(&output.stderr));
        report.issue(format!("{repository} CI check failed: {error}"));
        return;
    }

    let runs: Vec<WorkflowRun> = match serde_json::from_slice(&output.stdout) {
        Ok(runs) => runs,
        Err(error) => {
            report.issue(format!("{repository} CI response was invalid: {error}"));
            return;
        }
    };
    let Some(run) = runs.first() else {
        report.issue(format!("{repository} has no GitHub Actions runs"));
        return;
    };

    match (run.status.as_str(), run.conclusion.as_deref()) {
        ("completed", Some("success")) => {}
        ("completed", Some(conclusion)) => report.issue(format!(
            "{repository} latest {} run is {conclusion} ({}) — {}",
            run.workflow_name, run.head_branch, run.url
        )),
        (status, _) => report.note(format!(
            "{repository} latest {} run is {status} ({}) — {}",
            run.workflow_name, run.head_branch, run.url
        )),
    }
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    status: String,
    conclusion: Option<String>,
    #[serde(rename = "workflowName")]
    workflow_name: String,
    #[serde(rename = "headBranch")]
    head_branch: String,
    #[allow(dead_code)]
    url: String,
}

fn is_local_link(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('#')
        && !target.starts_with('/')
        && !target.starts_with("http://")
        && !target.starts_with("https://")
        && !target.starts_with("mailto:")
}

fn is_safe_repository(repository: &str) -> bool {
    let mut parts = repository.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(name) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !owner.is_empty()
        && !name.is_empty()
        && owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn display_path(path: &Path) -> String {
    path.strip_prefix(home_dir())
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn compact(value: &str) -> String {
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.len() > 180 {
        value.truncate(180);
        value.push('…');
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_names_are_constrained() {
        assert!(is_safe_repository("darkstardevx/apexdaemon"));
        assert!(!is_safe_repository("darkstardevx"));
        assert!(!is_safe_repository("darkstardevx/apexdaemon/extra"));
        assert!(!is_safe_repository("darkstardevx/../../secret"));
    }

    #[test]
    fn external_and_anchor_links_are_not_local() {
        assert!(!is_local_link("https://example.com"));
        assert!(!is_local_link("#config"));
        assert!(is_local_link("assets/header.svg"));
    }

    #[test]
    fn compact_removes_newlines() {
        assert_eq!(compact("one\n two\tthree"), "one two three");
    }
}
