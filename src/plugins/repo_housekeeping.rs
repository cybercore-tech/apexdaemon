//! Passive background version of what cyberfleet's TUI does on demand:
//! periodic `git fetch` + local dirty/ahead-behind status across your
//! repos, plus polling a small watchlist of GitHub PRs/issues via `gh`
//! (already authenticated in this environment — no separate token/HTTP
//! client needed).
//!
//! Repo discovery deliberately reuses cyberfleet's own
//! `~/.config/cyberfleet/config.json` (`roots`/`ignore`/`max_depth`) when
//! it exists, rather than keeping a second, independently-maintained repo
//! list — one source of truth for "which repos do I track" shared with
//! the tool that already has a UI for editing it.
//!
//! Fetch is non-interactive on purpose: `GIT_TERMINAL_PROMPT=0` plus a
//! hard timeout means a repo whose remote needs an SSH passphrase is
//! skipped with a log line instead of hanging the whole scan — the
//! opposite tradeoff from cyberfleet's TUI, which can suspend itself and
//! let a human type the passphrase. A headless daemon has nobody to ask.

use crate::config::expand_home;
use crate::plugin::{tick_forever, Context, Plugin};
use git2::{BranchType, Repository, StatusOptions};
use serde::Deserialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tokio::process::Command;

pub struct RepoHousekeepingPlugin;

impl Plugin for RepoHousekeepingPlugin {
    fn name(&self) -> &'static str {
        "repo-housekeeping"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.repo_housekeeping.interval_secs.max(300));
            tick_forever("repo-housekeeping", period, || tick(&ctx)).await
        })
    }
}

#[derive(Deserialize, Default)]
struct CyberfleetConfig {
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default = "default_depth")]
    max_depth: usize,
}

fn default_depth() -> usize {
    4
}

fn discover_repos(ctx: &Context) -> Vec<PathBuf> {
    let cyberfleet_path = expand_home(&ctx.config.repo_housekeeping.cyberfleet_config);
    let (roots, ignore, max_depth) = match std::fs::read_to_string(&cyberfleet_path)
        .ok()
        .and_then(|s| serde_json::from_str::<CyberfleetConfig>(&s).ok())
    {
        Some(cfg) if !cfg.roots.is_empty() => {
            println!(
                "[repo-housekeeping] using repo roots from {}",
                cyberfleet_path.display()
            );
            (cfg.roots, cfg.ignore, cfg.max_depth)
        }
        _ => (
            ctx.config.repo_housekeeping.roots.clone(),
            vec![],
            default_depth(),
        ),
    };

    let mut repos = Vec::new();
    for root in roots {
        let root = expand_home(&root);
        walk(&root, max_depth, &ignore, &mut repos);
    }
    repos
}

fn walk(dir: &Path, depth_left: usize, ignore: &[String], out: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        out.push(dir.to_path_buf());
        return; // don't recurse into nested repos (submodules, vendored checkouts)
    }
    if depth_left == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || ignore.iter().any(|i| i == name.as_ref()) {
            continue;
        }
        walk(&path, depth_left - 1, ignore, out);
    }
}

/// (dirty file count, commits ahead, commits behind). Any of the git2
/// calls failing (detached HEAD, no upstream configured, empty repo)
/// degrades to "nothing to report" for that piece rather than failing the
/// whole repo — an unusual repo state shouldn't stop the scan of every
/// other repo.
fn local_status(path: &Path) -> anyhow::Result<(usize, usize, usize)> {
    let repo = Repository::open(path)?;

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false);
    let dirty = repo.statuses(Some(&mut opts))?.len();

    let (ahead, behind) = ahead_behind(&repo).unwrap_or((0, 0));
    Ok((dirty, ahead, behind))
}

fn ahead_behind(repo: &Repository) -> Option<(usize, usize)> {
    let head = repo.head().ok()?;
    let branch_name = head.shorthand()?;
    let local_oid = head.target()?;
    let branch = repo.find_branch(branch_name, BranchType::Local).ok()?;
    let upstream = branch.upstream().ok()?;
    let upstream_oid = upstream.get().target()?;
    repo.graph_ahead_behind(local_oid, upstream_oid).ok()
}

async fn fetch_noninteractive(path: &Path) -> bool {
    let fetch = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("fetch")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    matches!(
        tokio::time::timeout(Duration::from_secs(20), fetch).await,
        Ok(Ok(out)) if out.status.success()
    )
}

async fn tick(ctx: &Context) -> anyhow::Result<()> {
    let repos = discover_repos(ctx);
    if repos.is_empty() {
        return check_watched_prs(ctx).await;
    }

    let mut needs_attention = Vec::new();
    for repo in &repos {
        if !ctx.dry_run {
            fetch_noninteractive(repo).await;
        }
        match local_status(repo) {
            Ok((dirty, ahead, behind)) if dirty > 0 || ahead > 0 || behind > 0 => {
                let name = repo
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut parts = Vec::new();
                if dirty > 0 {
                    parts.push(format!("{dirty} dirty"));
                }
                if ahead > 0 {
                    parts.push(format!("{ahead} ahead"));
                }
                if behind > 0 {
                    parts.push(format!("{behind} behind"));
                }
                needs_attention.push(format!("{name}: {}", parts.join(", ")));
            }
            Ok(_) => {}
            Err(e) => eprintln!("[repo-housekeeping] {}: {e:#}", repo.display()),
        }
    }

    if !needs_attention.is_empty() {
        println!(
            "[repo-housekeeping] {} repo(s) need attention:",
            needs_attention.len()
        );
        for line in &needs_attention {
            println!("  {line}");
        }
        ctx.notifier.send(
            "ApexDaemon: repos need attention",
            &format!(
                "{} repo(s): {}",
                needs_attention.len(),
                needs_attention.join("; ")
            ),
        );
    }

    check_watched_prs(ctx).await
}

/// GitHub's issues API also returns pull requests (a PR is a superset of
/// an issue), so one endpoint covers both "owner/repo#N" cases without
/// needing to know ahead of time which kind N is. This can't distinguish
/// "closed" from "merged" — good enough for "something changed, go look",
/// not a replacement for actually opening the PR.
async fn check_watched_prs(ctx: &Context) -> anyhow::Result<()> {
    for spec in &ctx.config.repo_housekeeping.watch_prs {
        let Some((repo, number)) = parse_watch_spec(spec) else {
            eprintln!("[repo-housekeeping] skipping malformed watch_prs entry: {spec}");
            continue;
        };

        let output = Command::new("gh")
            .args([
                "api",
                &format!("repos/{repo}/issues/{number}"),
                "--jq",
                ".state",
            ])
            .output()
            .await;
        let Ok(output) = output else { continue };
        if !output.status.success() {
            continue;
        }
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if state.is_empty() {
            continue;
        }

        let key = format!("repo_watch_{repo}_{number}");
        let prior = ctx.state.get_str(&key);
        if prior.as_deref() != Some(state.as_str()) {
            if prior.is_some() {
                ctx.notifier.send(
                    "ApexDaemon: PR/issue update",
                    &format!("{spec} is now {state}"),
                );
            }
            ctx.state.set(&key, serde_json::Value::String(state));
        }
    }
    Ok(())
}

/// "owner/repo#123" -> ("owner/repo", 123).
fn parse_watch_spec(spec: &str) -> Option<(&str, &str)> {
    spec.split_once('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo_hash_number() {
        assert_eq!(
            parse_watch_spec("darkstardevx/OmNote#4"),
            Some(("darkstardevx/OmNote", "4"))
        );
    }

    #[test]
    fn rejects_spec_without_hash() {
        assert_eq!(parse_watch_spec("darkstardevx/OmNote"), None);
    }

    #[test]
    fn walk_finds_a_repo_and_does_not_recurse_into_it() {
        let base = std::env::temp_dir().join(format!("apexd-walk-test-{}", std::process::id()));
        let repo_dir = base.join("proj");
        let nested_git = repo_dir.join("vendor").join("sub");
        std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
        std::fs::create_dir_all(&nested_git).unwrap();
        std::fs::create_dir_all(nested_git.join(".git")).unwrap();

        let mut out = Vec::new();
        walk(&base, 4, &[], &mut out);

        assert_eq!(out, vec![repo_dir]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn walk_skips_ignored_directory_names() {
        let base =
            std::env::temp_dir().join(format!("apexd-walk-ignore-test-{}", std::process::id()));
        let ignored_repo = base.join("node_modules").join("proj");
        std::fs::create_dir_all(ignored_repo.join(".git")).unwrap();

        let mut out = Vec::new();
        walk(&base, 4, &["node_modules".to_string()], &mut out);

        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
