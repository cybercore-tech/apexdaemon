//! Auto-commits (and optionally pushes) the darknotes vault on an
//! interval. cyberdesk already auto-commits on every save from inside its
//! own editor, but nothing catches edits made outside cyberdesk (a plain
//! text editor, a script, syncing from another machine) — this plugin is
//! the safety net for that gap, not a replacement for cyberdesk's own
//! commit-on-save.
//!
//! Push is the one operation here that touches the network, and the
//! vault's remote is SSH with a passphrase-protected key (by design, kept
//! that way — see the user's own stated preference to keep SSH+passphrase
//! remotes rather than switch to HTTPS+token). An unattended daemon must
//! never let `ssh` prompt on a TTY it doesn't have: that's exactly the
//! hang cyberfleet had to work around for interactive fetches by
//! suspending its TUI. A headless daemon has no TTY to suspend *to*, so
//! the correct fix here is the opposite one — `GIT_TERMINAL_PROMPT=0`
//! makes git refuse to prompt at all, so a push fails fast with a clear
//! error (agent not loaded / key not available) instead of hanging
//! forever. If an ssh-agent is already loaded in the desktop session
//! (the common case), the push just succeeds silently with no prompt
//! needed either way.

use crate::config::expand_home;
use crate::plugin::{tick_forever, Context, Plugin};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::process::Command;

pub struct VaultBackupPlugin;

impl Plugin for VaultBackupPlugin {
    fn name(&self) -> &'static str {
        "vault-backup"
    }

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let period = Duration::from_secs(ctx.config.vault_backup.interval_secs.max(60));
            tick_forever("vault-backup", period, || backup_once(&ctx)).await
        })
    }
}

/// Pure logic split out from the git call for unit testing: `git status
/// --porcelain` prints one line per changed path, nothing at all when
/// clean.
fn is_dirty(porcelain_output: &str) -> bool {
    !porcelain_output.trim().is_empty()
}

async fn backup_once(ctx: &Context) -> anyhow::Result<()> {
    let path = expand_home(&ctx.config.vault_backup.path);
    if !path.join(".git").exists() {
        anyhow::bail!("{} is not a git repo", path.display());
    }

    let status = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["status", "--porcelain"])
        .output()
        .await?;
    if !status.status.success() {
        anyhow::bail!("git status failed: {}", String::from_utf8_lossy(&status.stderr));
    }
    let porcelain = String::from_utf8_lossy(&status.stdout);
    if !is_dirty(&porcelain) {
        return Ok(());
    }

    let file_count = porcelain.lines().count();
    if ctx.dry_run {
        println!("[vault-backup] (dry-run) would commit {file_count} changed path(s) in {}", path.display());
        return Ok(());
    }

    Command::new("git").arg("-C").arg(&path).args(["add", "-A"]).status().await?;

    let message = format!("auto: backup {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"));
    let commit = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["commit", "-m", &message])
        .output()
        .await?;
    if !commit.status.success() {
        anyhow::bail!("git commit failed: {}", String::from_utf8_lossy(&commit.stderr));
    }
    println!("[vault-backup] committed {file_count} changed path(s): {message}");
    ctx.notifier.send("ApexDaemon: vault backed up", &format!("{file_count} path(s) committed"));

    if !ctx.config.vault_backup.push {
        return Ok(());
    }

    let push = Command::new("git")
        .arg("-C")
        .arg(&path)
        .arg("push")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if push.status.success() {
        println!("[vault-backup] pushed");
    } else {
        let err = String::from_utf8_lossy(&push.stderr);
        eprintln!("[vault-backup] push failed (commit is still safe locally): {err}");
        ctx.notifier.send(
            "ApexDaemon: vault push failed",
            "commit is safe locally — push manually (likely needs an unlocked ssh-agent)",
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_porcelain_output_is_clean() {
        assert!(!is_dirty(""));
        assert!(!is_dirty("\n"));
    }

    #[test]
    fn nonempty_porcelain_output_is_dirty() {
        assert!(is_dirty(" M src/main.rs\n"));
        assert!(is_dirty("?? new-note.md\n"));
    }
}
