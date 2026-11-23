<h1 align="center">Aeon</h1>

<p align="center">A fast, lean terminal for Windows.</p>

<p align="center">
  <a href="https://github.com/UlloaSP/aeon/releases/latest"><strong>Download</strong></a> ·
  <a href="https://github.com/UlloaSP/aeon/releases">All releases</a> ·
  <a href="./docs/release.md">Release process</a> ·
  <a href="./CONTRIBUTING.md">Contributing</a>
</p>

Aeon opens instantly, stays small, and gets out of the way. It runs your shell through ConPTY, the same pseudo console Windows Terminal uses, and draws it without a GPU device, a browser engine, or a runtime it does not need.

It works with PowerShell 7, Windows PowerShell, cmd, WSL, Git Bash, and anything else that runs in a console.

## What you get

- **Fast.** Native Rust, built with full link-time optimization. Startup is instant, and a dropped frame is a bug.
- **Small.** One static `aeon.exe` under 1 MB. A window with a shell uses about 13 MB of private memory. Every release build reports both numbers, so regressions are caught before they ship.
- **Native to Windows.** ConPTY, per-monitor DPI, x64 and arm64, and an MSI installer that puts `aeon` on your PATH.
- **No account, no telemetry.** It is a terminal.
- **Open.** MIT licensed. If you don't like something, fork it.

> Aeon is early. It hosts a shell, prints text, and handles the keys you need to use it. Colors, scrollback, selection, and tabs are next.

## Install

Download the MSI for your machine from the [latest release](https://github.com/UlloaSP/aeon/releases/latest):

| Machine          | File                               |
| ---------------- | ---------------------------------- |
| Most PCs         | `aeon-<version>-windows-x64.msi`   |
| ARM (Snapdragon) | `aeon-<version>-windows-arm64.msi` |

The installer adds Aeon to the Start menu and to your PATH, so you can also start it with `aeon` from any shell. Prefer no installer? Each release also has a portable `.zip` with just `aeon.exe`. `SHA256SUMS` lists the checksum of every file.

Nightly builds of `main` are published as prereleases. They install over each other and over the stable release.

Builds are not code-signed yet, so Windows SmartScreen may warn the first time you run the installer.

## Configuration

Aeon reads `%APPDATA%\aeon\config.toml`. Every setting is optional, and an unknown key is an error, so typos don't fail silently. If the file is invalid, Aeon starts with the defaults and says so in the window title.

```toml
[shell]
program = "pwsh.exe"          # default: pwsh, then powershell, then cmd
args = ["-NoLogo"]
cwd = "C:\\code"              # default: where you launched Aeon, or your home folder

[font]
path = "C:\\Windows\\Fonts\\CascadiaMono.ttf"   # default: Cascadia Mono, then Consolas
size = 11.0                   # points

[window]
columns = 120
rows = 32
padding = 8

[colors]
foreground = "#d8dbe2"
background = "#101114"
cursor = "#f2b84b"
```

## Build from source

You need Windows, [rustup](https://rustup.rs), and the Visual Studio C++ build tools. The toolchain version is pinned in [rust-toolchain.toml](./rust-toolchain.toml); rustup installs it on first build.

```powershell
cargo run -p aeon                      # debug build, with a console for logs
cargo build --release -p aeon          # what ships: target\release\aeon.exe
cargo test --workspace
```

To build the MSI, see [docs/release.md](./docs/release.md#building-an-msi-locally).

## Contributing

Bug reports with clear reproduction steps are the most useful contribution. Small, focused fixes are the most likely to be merged.

- Read [CONTRIBUTING.md](./CONTRIBUTING.md) before you open a pull request.
- [AGENTS.md](./AGENTS.md) explains how the code is organized and what we will not compromise on. It is written for coding agents and works just as well for people.

## License

[MIT](./LICENSE)
