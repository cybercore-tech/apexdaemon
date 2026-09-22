# Changelog

All notable changes to ApexDaemon are recorded here.

## Unreleased

### Fixed: Omarchy theme-sync false failures during theme changes

- **Observed symptom:** switching a custom Omarchy theme displayed an
  `ApexDaemon: theme sync failed` notification with a TOML error pointing at
  `mode = "dark"` and reporting `missing field cursor`.
- **Root cause:** `omarchy theme set` replaces the active theme directory in
  multiple filesystem operations. The watcher could read the new
  `colors.toml` between operations, while it contained only a valid header or
  while the file was still being written. Separately, several valid
  hand-authored themes provide semantic colors but omit `cursor` and
  `color0..color15`.
- **Fix:** theme-sync now retries incomplete reads for a bounded 1.2-second
  window, accepts both generated ANSI palettes and semantic-only palettes,
  derives a missing cursor from `foreground`, and derives missing ANSI slots
  from the corresponding semantic/bright colors. A genuinely incomplete file
  still reports an error after retries.
- **Verification:** the full release gates pass (34 tests, formatting, check,
  Clippy, and cargo-deny), including regression coverage for both palette
  shapes. Live switches between Cosmere Investiture and Cybercore Iron Neural
  produced successful sync records with no new parse failures.
