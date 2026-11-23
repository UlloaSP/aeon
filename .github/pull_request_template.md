<!--
Read CONTRIBUTING.md before opening a PR. Solve one underlying problem.
Use a conventional commit title in plain language, such as
"fix(pty): shells launched from Explorer start in the home directory".
-->

## Problem

<!-- Explain the problem in a sentence or two. Include expected behavior and
reproduction steps (shell, Windows version, display scale) where relevant. -->

## Change

<!-- Explain how this fixes the problem. If it spans crates, explain why those
changes are needed for the same fix. Split independent fixes. -->

## Verification

<!-- Describe the focused tests or manual checks you ran and what you observed.
State anything you could not check. "Tests pass" alone is insufficient.

For changes on the hot path (PTY reads, parsing, rendering), include before/after
numbers: startup memory, idle memory, or time to dump a large file.
For visual changes, include before/after screenshots. Upload evidence to GitHub
and embed or link it here. Never commit PR-only assets. -->

<!-- If you used an agent, end with the model and harness that did the work. -->
