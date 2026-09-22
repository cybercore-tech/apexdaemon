# Changelog

All notable changes to ApexDaemon are recorded here.

## Unreleased

- Fixed Omarchy theme-sync notifications caused by reading a theme while
  `omarchy theme set` was still replacing its files.
- Added support for semantic-only Omarchy palettes, including safe fallbacks
  for the cursor and ANSI color slots.
- Added regression coverage for both generated and semantic-only theme files.
