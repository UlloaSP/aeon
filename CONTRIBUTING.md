# Contributing

Aeon is small on purpose. Review time is the bottleneck, not ideas, so the contributions most likely to land are the ones that are easy to review.

## The best way to help

- **Report bugs well.** One bug per issue, with the shell, Windows version, display scale, and the steps that reproduce it. A bug we can reproduce is a bug we can fix.
- **Close what's done.** If an open issue no longer happens on the latest release, say so and give the version. If an issue duplicates another, link the original.
- **Test open pull requests.** A comment saying what you tried and what you saw is worth more than a second PR for the same thing.

Search the open issues and pull requests before you start.

## Rules for a pull request

### Branch from develop

Create your branch from `develop` and open the pull request against `develop`. Name it `<type>/<short-kebab-name>`, where type is one of `feature`, `fix`, `chore`, `docs`, `refactor`, `perf`, `test`, `ci`, or `build`; for example `fix/erase-line-cursor`. CI rejects other names and other targets. Pull requests into `develop` are squash merged. `main` only receives `develop`, at release time. The full flow is in [AGENTS.md](./AGENTS.md#git-workflow).

### Fix one problem

A pull request fixes one bug or does the one thing a maintainer asked for. A crash, a regression, a rendering error, and a real performance or memory problem are bugs. A change to how Aeon behaves is a feature, even when the PR calls it a fix.

Features need an issue that a maintainer has agreed to first. That includes new config options, new dependencies, and changes to the installer.

### Keep it fast and small

Aeon's startup time, memory, and binary size are features. If your change touches the PTY reader, the parser, or the renderer, include before/after numbers from a release build. If it adds a dependency, say why and what it costs.

### Show that it works

Describe the tests you added or the checks you ran and what you observed. A bug fix needs a test that fails before the fix, unless the PR explains why one is impractical. Visual changes need before/after screenshots, uploaded to GitHub rather than committed.

Before pushing, run what CI runs:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

### Title and description

Use a conventional commit title in plain language, such as `fix(term): wide characters no longer overlap the next cell`. Fill in the [pull request template](.github/pull_request_template.md). If an agent did the work, say which model and harness.

A pull request that follows these rules can still be closed. We do not owe a review or a merge, but we will try to explain why.
