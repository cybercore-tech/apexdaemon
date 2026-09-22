# Omarchy theme-sync: incident and operator guide

This document explains the theme-sync failure that appeared while dogfooding
the custom Omarchy themes, what caused it, and how the corrected behavior is
validated.

## The original symptom

During a theme switch, the desktop notification was:

```text
ApexDaemon: theme sync failed
TOML parse error at line 1, column 1
1 | mode = "dark"
missing field `cursor`
```

The displayed line was misleading. `mode = "dark"` is valid TOML; it was the
only complete line visible because ApexDaemon had read the active theme while
Omarchy was still replacing the theme directory. The parser then reported the
first required field that had not appeared yet.

## What happens during a switch

Omarchy's `omarchy theme set <name>` operation does not update one file in
place. It replaces the active-theme directory and emits several filesystem
events as files become available. A watcher can observe any of these states:

1. the old theme directory;
2. a new directory with only a partial `colors.toml`;
3. a complete new palette;
4. follow-up events for wallpapers and generated consumer files.

The old implementation attempted one parse for every debounced event. A
partial but syntactically valid file therefore generated a false failure.

## The second compatibility issue

Omarchy themes can use either of these valid palette shapes:

- generated palettes containing `cursor` and explicit `color0` through
  `color15` keys;
- semantic-only palettes containing `background`, `foreground`, the named
  hues (`red`, `green`, `yellow`, `blue`, `magenta`, `cyan`), and optional
  bright variants.

The custom Cosmere Investiture theme uses the second shape. Treating
`cursor` and every ANSI slot as mandatory made that valid theme fail even
after the filesystem replacement had completed.

## The fix

The theme-sync adapter now performs two checks before conversion:

1. It retries file reads and TOML deserialization every 100 ms for at most
   12 attempts (1.2 seconds total). Missing fields and missing files during
   that window are treated as transient replacement state.
2. It validates the complete palette shape. Explicit ANSI slots are preferred;
   semantic-only themes use these deterministic fallbacks:

   | Slot | Fallback |
   | --- | --- |
   | `color0` | `background` |
   | `color1..6` | `red`, `green`, `yellow`, `blue`, `magenta`, `cyan` |
   | `color7` | `foreground` |
   | `color8` | `dark_foreground`, then `muted`, then `background` |
   | `color9..14` | bright red/green/yellow/blue/magenta/cyan, then normal hue |
   | `color15` | `bright_foreground`, then `foreground` |

   A missing `cursor` falls back to `foreground`. If a theme is missing both
   the generated and semantic values needed to produce a complete palette,
   ApexDaemon still reports the error after the retry window; this prevents a
   genuinely broken theme from being silently published.

## Output and safety boundaries

Successful conversion writes to:

```text
<cybercore_root>/schema/themes/omarchy-live/<theme-slug>.json
```

The `omarchy-live` family is intentionally separate from hand-curated
Cybercore themes. ApexDaemon does not edit `/usr/share/omarchy`, does not
rewrite the active theme, and does not delete existing Cybercore themes.
Cybercore consumers embed theme data at build time, so they must be rebuilt
after a successful sync.

## Verification

Check the service and follow theme events:

```bash
systemctl --user is-active apexdaemon.service
journalctl --user -u apexdaemon.service -f
```

A successful event looks like:

```text
ApexDaemon: theme synced: 'cosmere-investiture' -> cybercore/schema/themes/omarchy-live/cosmere-investiture.json
```

To reproduce the supported paths safely, switch between a semantic-only theme
and a generated ANSI theme, then return to the original theme:

```bash
omarchy theme set 'Cybercore Iron Neural'
omarchy theme set 'Cosmere Investiture'
```

Both switches should produce successful sync records. Historical failures may
remain in the journal; only entries after the fixed daemon was restarted are
relevant when validating the repair.
