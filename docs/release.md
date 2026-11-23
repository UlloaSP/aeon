# Releases

Releases are built and published by [release.yml](../.github/workflows/release.yml). Nothing is built on a developer machine.

## Channels

| Channel | Trigger                                           | Version                          | GitHub release                   |
| ------- | ------------------------------------------------- | -------------------------------- | -------------------------------- |
| Stable  | Push tag `vX.Y.Z`, or run Release with `stable`   | `version` in `Cargo.toml`        | Latest (prerelease if `-suffix`) |
| Nightly | Daily at 04:17 UTC, or run Release with `nightly` | `X.Y.Z-nightly.<yyyymmdd>.<run>` | Prerelease, newest 10 kept       |

Stable builds `main`; nightly builds `develop` (see the branch flow in [AGENTS.md](../AGENTS.md#git-workflow)). Manual runs pick the right branch themselves, whatever branch you start them from. A scheduled nightly is skipped when `develop` has not moved since the last one. Set the repository variable `PAUSE_NIGHTLY` to `true` to stop scheduled nightlies without editing the workflow.

## Cutting a stable release

`Cargo.toml` is the single source of truth for the version, so a release starts with a reviewed version bump on `develop`:

1. From `develop`, in a worktree, create `chore/release-v0.2.0`. Change `version` under `[workspace.package]` in `Cargo.toml`, refresh `Cargo.lock` (`cargo check`), and open a PR into `develop` titled `chore(release): v0.2.0`. Squash merge it.
2. Open a PR from `develop` into `main` titled `release: v0.2.0`. Merge it with a merge commit.
3. Tag the merge commit on `main`, either by pushing the tag:

   ```powershell
   git fetch origin
   git tag v0.2.0 origin/main
   git push origin v0.2.0
   ```

   or by running **Actions → Release → Run workflow** with channel `stable`, which tags `main` itself.
4. Open a PR from `main` into `develop` and merge it with a merge commit, so `develop` has the release merge.

Preflight refuses a tag that does not match `Cargo.toml`, a version whose tag already exists, and any stable commit that is not on `main`.

After a stable release, nightlies automatically move to the next patch version (`0.2.1-nightly...`) so they sort above it. Bump `Cargo.toml` past that whenever the next release is planned to be a minor or major.

## What a release runs

1. **Preflight** resolves the channel, version, tag, previous tag, and commit ([resolve-release.sh](../.github/scripts/resolve-release.sh)).
2. **Quality** runs clippy and the full test suite on that commit ([rust-checks.yml](../.github/workflows/rust-checks.yml)), the same workflow CI uses.
3. **Windows x64 / arm64** build the release binary, package the MSI and portable zip, and on x64 install the MSI, launch Aeon, record its startup memory in the job summary, and uninstall ([release-windows.yml](../.github/workflows/release-windows.yml)). CI runs this same workflow on packaging changes, so the release path is exercised before release day.
4. **Publish** writes `SHA256SUMS`, creates a draft release with generated notes, uploads every asset, and only then publishes it. A nightly's tag is created at this point.
5. **Prune** deletes all but the newest 10 nightlies (releases and tags). Stable releases are never touched.

## MSI details

- Built with WiX **5.0.2** from [packaging/windows/aeon.wxs](../packaging/windows/aeon.wxs). WiX 6 and later require accepting the Open Source Maintenance Fee EULA on every build, so upgrading is a deliberate decision, not a Dependabot bump.
- Per-machine install to `Program Files\Aeon`, a Start menu shortcut, and `Program Files\Aeon` appended to the machine `PATH`. Uninstall removes all three.
- MSI versions are numeric `major.minor.patch` (at most 255.255.65535). Prerelease suffixes are stripped, so a nightly installs as its base version. `AllowSameVersionUpgrades` lets each nightly replace the previous one.
- The `UpgradeCode` identifies Aeon across every version. Changing it makes new installs sit next to old ones instead of replacing them. Never change it.
- `aeon.exe` links the C runtime statically (`.cargo/config.toml`), so the MSI needs no Visual C++ Redistributable.
- Builds are not code-signed. Signing (for example with Azure Trusted Signing) slots into `release-windows.yml` between Build and Package.

## Building an MSI locally

Needs the .NET SDK for the WiX CLI:

```powershell
dotnet tool install --global wix --version 5.0.2
cargo build --release --locked --target x86_64-pc-windows-msvc -p aeon
./scripts/package-msi.ps1 -Version 0.1.0 -Arch x64
```

The MSI and zip land in `dist/`. For arm64, add the target first (`rustup target add aarch64-pc-windows-msvc`) and install the "MSVC ARM64 build tools" component of Visual Studio.

## Branch protection

`main` and `develop` each have a repository ruleset: no deletion, no force push, changes only through a pull request, and the `Check` status from [ci.yml](../.github/workflows/ci.yml) required. `Check` passes only when every other CI job passed or was skipped because the change could not affect it, so adding or splitting jobs never needs a ruleset change.

The rulesets also fix each branch's merge method: squash only into `develop` would block the `main` → `develop` sync, so `develop` allows squash and merge commits, and `main` allows merge commits only. Which branch may open a PR into which, and how work branches are named, is checked by the `Branch Rules` job in CI.
