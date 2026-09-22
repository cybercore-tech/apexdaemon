# Contributing to ApexDaemon

## Commit documentation policy

Every commit that changes ApexDaemon must carry enough information for a
future operator to understand the change without reconstructing it from a
diff alone. This applies to fixes, features, configuration changes, tests,
documentation, and release plumbing.

Use a concise subject followed by a body that answers:

1. **What changed?** Name the behavior, interface, or files affected.
2. **Why was it needed?** Describe the observed issue, user impact, or
   operational goal.
3. **How does it work?** Summarize the implementation and important safety
   boundaries, including fallback or failure behavior.
4. **How was it verified?** Record the tests, release gates, live checks, or
   other evidence run for this exact change.

For a bug fix, include the original symptom and root cause. For a new feature,
include its operator-visible behavior and limitations. For documentation-only
changes, identify the behavior or workflow being clarified. Do not rely on a
ticket number, a vague one-line subject, or the diff alone.

Recommended format:

```text
fix(theme-sync): tolerate transient Omarchy palette replacement

Problem: `omarchy theme set` briefly exposed a partial colors.toml, causing
false `missing field cursor` notifications; semantic-only themes also lacked
explicit ANSI slots.

Change: retry incomplete reads for 1.2 seconds and derive cursor/ANSI values
from semantic colors when explicit fields are absent. Truly incomplete themes
still fail after the bounded retry window.

Verification: `./scripts/release-gates full`; live switches between Cosmere
Investiture and Cybercore Iron Neural; 34 tests passing.
```

Before pushing, ensure the commit body, relevant documentation, and validation
evidence agree with the actual change. Existing historical commits are not
rewritten solely to apply this policy; the policy governs all commits from this
point forward.
