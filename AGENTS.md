# Aeon

Aeon is a terminal for Windows. A native Rust binary hosts your shell through ConPTY, parses its output into a cell grid, and paints it to a window on the CPU. It ships as a single static `aeon.exe` inside an MSI.

It exists because the terminals people actually use on Windows are slow, heavy, or both. Aeon is the one that opens instantly and stays small.

## What makes Aeon special?

These are the things we can never compromise on. Every change is measured against them.

### 1. Fast

Startup is instant and typing never lags behind the shell. A dump of a large file scrolls at the speed of the PTY, not the speed of the renderer. Performance regressions are bugs, even when nothing is functionally broken. The usual suspects: work on the UI thread that belongs on the reader thread, per-cell allocations, repainting when nothing changed, and parsing the same bytes twice.

### 2. Lean

Memory is a feature. Aeon does not hold a GPU device, a browser engine, or a runtime it does not use. Today a window with a shell uses about 13 MB of private memory and the binary is under 1 MB; the release smoke test reports both in every build summary. A dependency or feature that moves those numbers needs to earn it, and the PR says by how much.

### 3. Native to Windows

Aeon is built for Windows, not ported to it. ConPTY, per-monitor DPI, Windows paths, the shells Windows users run (pwsh, PowerShell, cmd, WSL, Git Bash), x64 and arm64. Cross-platform abstractions that cost Windows anything are not worth it.

### 4. Open

MIT licensed, built in the open. The release pipeline is in this repo and anyone can reproduce a build from a tag.

## A note on approach

Prefer the smallest model that makes the correct behavior unsurprising. Do not preserve complexity just because it already exists, and do not introduce machinery because it looks architecturally impressive. Understand the real constraint, then fight for the simple version.

Channel both "measure twice, cut once" and "yagni". Fight scope creep. Honor the developer's intent in both a minimal and realistic fashion.

The rest of this document helps you navigate the codebase and make changes effectively. Treat these as good defaults, not hard rules. The developer's preferences override anything here.

## Terminology

- **you** = the agent reading this file. **we / us / maintainers** = the people building Aeon. **user** = person using Aeon as their terminal.
- **shell** = the child process Aeon hosts (pwsh, cmd, wsl, ...). **PTY** = the ConPTY pseudo console between Aeon and the shell. **grid** = the screen of cells the parser writes into. **frame** = one painted image of the grid.

## The three ways to hurt yourself

1. **Killing by name.** Never `Stop-Process -Name aeon`, `taskkill /im aeon.exe`, or kill anything you found by matching a name or path. The developer is very likely running Aeon right now, maybe hosting the shell you are in. Kill only a PID you captured when you started it (`Start-Process -PassThru`).
2. **Writing to the live config.** `%APPDATA%\aeon\config.toml` is the developer's real configuration. Read it if you need to, never write, move, or delete it. Test config handling through `Config::parse` or a file in a temp directory.
3. **Blocking the UI thread.** The event loop thread paints and reads keyboard input. PTY reads, file I/O, and anything that can wait belong on another thread. A UI thread that blocks for one frame is a visible stutter; one that blocks on the PTY is a hang.

## Hit every surface

The most common defect is a change that works on the path you tested and is missing everywhere else. Before calling work done, walk this list and say which entries applied:

- **Input.** Typed text, named keys, Ctrl/Alt combinations, paste, and (when it exists) IME and mouse. Fixing one is not fixing input.
- **Shells.** pwsh, Windows PowerShell, cmd, WSL, Git Bash. They differ in how they draw prompts, query the terminal, and handle resize.
- **Display.** 100%, 150%, and 200% scale, moving a window between monitors with different scales, and resizing to tiny sizes.
- **Architectures.** x64 and arm64. arm64 is cross-compiled in CI; a dependency with native code must build for both.
- **Launch paths.** From the Start menu, from another shell (`aeon` on PATH), from Explorer, and from the launcher's Ctrl+Enter (`aeon --command`). They start with different working directories.
- **Two processes.** A terminal (`aeon`) and the resident launcher (`aeon --background`, one per session, started at sign-in) are separate processes. The resident stays running for days: it must stay small and must not leak. To try a resident while the developer's installed one runs, use a debug build (it takes its own single-instance lock) with a different `[launcher] hotkey` in a temporary `APPDATA`.
- **Config.** A new option has a default, rejects typos (`deny_unknown_fields`), and appears in the README's config section.
- **Integration off.** With `shell.integration = false` or custom `args`, the shell reports no directory: the bar must still make sense with only the starting directory.
- **Reverse states.** Every way in needs a way out: what opens closes, what resizes up resizes down.
- **Docs.** Check whether the change makes existing guidance inaccurate. Follow [Documentation](#documentation).

## Verifying

- Before you push: `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked`. These are exactly what CI runs.
- Iterate with `cargo test -p <crate>` (add a test name filter for one test).
- `aeon-term` is pure: test parser behavior by feeding bytes to `Terminal::advance` and asserting on the grid. Most bugs in what the terminal shows are reproducible this way, and a fix needs a test that fails before it.
- `aeon-pty` tests spawn real processes through ConPTY. Wait for the output you expect, never for a fixed time.
- For changes to the hot path, measure before and after with a release build (`cargo build --release -p aeon`): private memory after startup, and the time to `type` a large file. Put the numbers in the PR.
- For visible changes, run the release build and look at it at more than one display scale.
- Packaging changes: build the MSI locally with [scripts/package-msi.ps1](scripts/package-msi.ps1) (needs WiX 5, see [docs/release.md](docs/release.md)). Do not install it on the developer's machine; CI installs and uninstalls it on a clean runner.

## Git workflow

Gitflow with two long-lived branches. Both are protected: nothing reaches them except through a pull request.

- **`main`** is what has shipped or is about to. Stable releases are tagged here.
- **`develop`** is the integration branch. Nightlies are built from it.

```
feature/x ──squash──▶ develop ──merge commit──▶ main ──merge commit──▶ develop
fix/y     ──squash──▶    ▲                       (tag vX.Y.Z)           (sync back)
```

1. **Branch from `develop`, always.** Never from `main` or from another work branch. Fetch first: `git fetch origin` and branch from `origin/develop`.
2. **Name it `<type>/<short-kebab-name>`.** Types: `feature/`, `fix/`, `chore/`, `docs/`, `refactor/`, `perf/`, `test/`, `ci/`, `build/`. CI rejects any other name (see `branch_rules` in [ci.yml](.github/workflows/ci.yml)).
3. **Work in a worktree, never in the main checkout.** Every new branch gets its own worktree next to the repo, in `../aeon.worktrees/<type>-<name>`:

   ```powershell
   git fetch origin
   git worktree add -b fix/erase-line ..\aeon.worktrees\fix-erase-line origin/develop
   ```

   The main checkout stays on `develop` and is not where you edit. Each worktree has its own `target/`, so the first build there is a full build.
4. **Work branch → `develop`: squash merge.** One commit per PR on `develop`, titled with the PR title.
5. **`develop` → `main`: merge commit.** Opened when `develop` is ready to release. Never squash it: `main` and `develop` must share history or the next sync conflicts.
6. **`main` → `develop`: merge commit, right after every merge into `main`.** Brings the merge commit (and any release tag) back so the branches do not diverge.
7. **Clean up once the PR is merged.** Confirm it with `gh pr view <n> --json state`, then remove the worktree and the local branch. Squash merges are not ancestors of `develop`, so the branch needs `-D`:

   ```powershell
   git worktree remove ..\aeon.worktrees\fix-erase-line
   git branch -D fix/erase-line
   git fetch --prune origin
   ```

Never commit or push directly to `main` or `develop`, never force-push them, and never merge locally into them. If a task seems to need any of that, stop and ask.

## Pull requests

- Never make a PR unless the developer explicitly asks you to.
- Work PRs target `develop`. Only `develop` targets `main`, and only `main` targets `develop` besides work branches.
- Conventional commit titles, plain language: `fix(term): erase-in-line no longer clears the cursor cell`. The title becomes the squash commit on `develop`, so it must stand on its own.
- Body: the problem in a sentence or two, then how you fixed it. End with the model and harness that did the work.
- Visual changes need before/after images. Upload PR evidence to GitHub, never commit it.
- One request is one PR. Split only when asked.

## Documentation

Most code changes do not need a documentation change. Agents can read the code.

- `docs/` is for decisions and their reasons, constraints that span crates, and traps that are hard to discover from the source. Before adding a paragraph, ask what a maintainer would get wrong without it.
- Do not enumerate fields, narrate control flow, or append PR summaries. Anything that can drift will drift.
- A local reason the code can't show goes in a short nearby comment. When a documented decision changes, rewrite or remove the affected text; do not append another account of the new behavior.
- [docs/release.md](docs/release.md) is the release runbook. Keep it true.

## Plans and work artifacts

- Do not commit implementation plans, research notes, or agent scratch files. Keep temporary material outside the repo and outside `../aeon.worktrees`.

## Where code lives

- `crates/aeon` - the binary. The terminal window and its sessions (`app.rs`), the settings panel (`settings.rs`), the resident launcher (`launcher.rs`), CPU rendering including the sidebar (`render.rs`), and fonts (`font.rs`). Thin: it wires the other crates together.
- `crates/aeon-term` - VT parsing into a grid. Pure and synchronous: no I/O, no clock, no window. Most terminal behavior lives and is tested here.
- `crates/aeon-pty` - ConPTY process spawning, command-line quoting, and shell integration (`shell.rs`, with the PowerShell script in `aeon.ps1`). Its Win32 code is in `conpty.rs`.
- `crates/aeon-context` - what the context bar shows: git state and the servers under the shell, gathered on a background thread. Its Win32 code is in `sys.rs`.
- `crates/aeon-launcher` - the launcher's state (query, results, calculator, history, installed apps, built-in commands such as `theme`) and update decisions (`update.rs`) as pure, tested code; the resident's update thread (`updater.rs`); and `sys.rs` for the global hotkey, notification-area icon, clipboard, single-instance lock, Windows theme, HTTPS downloads, and SHA-256. The window itself is `crates/aeon/src/launcher.rs`.
- `crates/aeon-config` - `config.toml` schema and loading, and `edit.rs`, which changes single keys while keeping the user's comments and layout. Aeon writes the file only through it.
- `packaging/windows` - the WiX source for the MSI. Its `UpgradeCode` must never change.
- `scripts/` - developer and CI scripts that are not workflow-specific.
- `.github/workflows` - `ci.yml` (every PR, plus pushes to `main` and `develop`), `release.yml` (stable tags on `main`, nightlies from `develop`), and the reusable `rust-checks.yml` and `release-windows.yml` both call. `.github/actions` holds the shared setup steps.

## Taste

- Complexity belongs at the boundary. `aeon-term` stays pure, `aeon` stays thin, and Win32 details stay in one `sys`-style module per crate (`aeon-pty/src/conpty.rs`, `aeon-context/src/sys.rs`, `aeon-launcher/src/sys.rs`). `unsafe` lives nowhere else.
- No allocation per cell or per byte on the hot path. Reuse buffers; allocate per chunk at most.
- Redraw only when something changed (`Terminal::take_dirty`). No timers or animations that repaint while idle.
- Every `unsafe` block has a `// SAFETY:` comment saying why it is sound (`clippy::undocumented_unsafe_blocks` enforces this).
- Inferred types over annotations. No `unwrap()` outside tests and truly impossible states; those use `expect("why")`.
- Default to no comment. Write one only for a constraint, platform quirk, or invariant the code can't show.
- New dependencies need a reason in the PR. Prefer `default-features = false` and enable only what is used.
- If a rule here fights the task in front of you, say so loudly and get a human sign-off before breaking it.
