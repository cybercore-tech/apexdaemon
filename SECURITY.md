# Security Policy

## Threat model

What ApexDaemon's design actually defends against, and what it
doesn't — read this before pointing it at anything that matters.

### What's defended

- **No network listener at all.** Every plugin only dials out
  (`fleet_health`'s TCP/HTTP checks, `repo_housekeeping`'s `git fetch`/
  `gh api`) or spawns a subprocess — nothing in this daemon ever binds
  a socket. There's no remote attack surface to exploit in the first
  place; the whole class of "an attacker reaches the daemon over the
  network" doesn't apply.
- **State file is user-owned only.** `~/.local/state/apexdaemon/
  state.json` (`src/main.rs`) is written under the invoking user's own
  `$HOME` with no special permissions widening — it holds only
  bookkeeping (last-seen PR state, last restart-attempt timestamp), never
  secrets.
- **Vault push never handles credentials itself.** `vault_backup`
  relies on an already-unlocked `ssh-agent` in the desktop session for
  push authentication — the daemon never reads, stores, or prompts for
  a key or passphrase. `GIT_TERMINAL_PROMPT=0` (`vault_backup.rs`,
  `repo_housekeeping.rs`) means a repo whose remote *would* need
  interactive auth fails fast with a log line instead of hanging the
  daemon waiting for a prompt that can never arrive headlessly.

### What's NOT defended (by design)

- **Arbitrary shell execution via `start_cmd`/`post_hooks`.**
  `fleet_health`'s per-service `start_cmd` and `vault_backup`'s
  `post_hooks` run whatever command string the config names, with no
  sandboxing beyond what the unit file itself applies (see below).
  This is a **trusted-config-input assumption, not a vulnerability**:
  the config is written by the same person who runs the daemon, not by
  an untrusted or remote party. Treat `config.toml` like a shell
  script you'd run yourself — don't point ApexDaemon at a config you
  didn't write or fully trust.
- **The host it runs on.** Like every other daemon in this workspace,
  ApexDaemon assumes the machine it runs on isn't already compromised.
  It doesn't defend against a local attacker with access to the
  running process, its config, or its state file.
- **`start_cmd`/`post_hooks` are not sandboxed by the unit file at
  all.** `apexdaemon.service` carries no systemd hardening directives,
  unlike GhostPort's units — not an oversight, a verified
  incompatibility with this specific machine. Every directive that
  would otherwise be a safe default for a `--user` unit
  (`NoNewPrivileges`, `LockPersonality`, `RestrictRealtime`,
  `ProtectKernelTunables`, `ProtectKernelModules`,
  `ProtectControlGroups`) was tried individually and broke this host's
  `ssh` outright: this machine routes every `ssh` invocation through a
  setuid-root `firejail` wrapper (`/usr/local/bin/ssh -> firejail`),
  which `repo_housekeeping`'s `git fetch` and `vault_backup`'s `git
  push` both depend on, and firejail cannot run once the kernel's
  `NoNewPrivs` bit is set (forced by any unprivileged seccomp filter,
  confirmed via `/proc/self/status`, regardless of what
  `NoNewPrivileges=` itself is set to) or once the unit has its own
  private mount namespace (forced by the `Protect*` directives, which
  breaks firejail's internal root-ownership checks a different way).
  Breaking every git operation this daemon exists to do would be a
  worse outcome than the hardening these directives buy on a
  single-user desktop. See `apexdaemon.service`'s own comment for the
  exact directive-by-directive test results. Untested: whether a
  `--system` unit (with real `CAP_SYS_ADMIN`) would avoid this —
  ApexDaemon runs as a `--user` unit by design, and switching that to
  work around this host's `ssh` wrapper wasn't judged worth the
  architecture change.

## Supported deployment model

A single user's own machine, running their own trusted config — the
same "a handful of things you personally administer" model as the rest
of this workspace. Not designed for a config authored by, or shared
with, anyone else.

## Reporting a vulnerability

Email **darkstardevx@gmail.com** (primary) or, as a backup,
**cybercore.sh@gmail.com**. Include:

- the affected file/commit and a minimal repro or PoC
- what you'd expect to happen instead
- how you'd rate the impact (your best guess is fine)

Expect an acknowledgement within a few days. Please don't include
exploit details in a public GitHub issue or PR until a fix has
shipped.

## Supported versions

Only the latest commit on `main` is supported — there's no tagged
release yet.
