<p align="center">
  <img src="./packaging/windows/aeon.svg" width="80" height="80" alt="Aeon logo: a ring with an amber block cursor" />
</p>

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
- **A launcher on Ctrl+Alt+Space.** From anywhere, open any app by name, run a command, search the web, switch Windows between light and dark, or do quick math. Your history comes with you.
- **Projects, not tabs.** The sidebar groups your shells by the repository they run in, and each one shows the servers it started (`:5173`). A header above the terminal shows the project, the git branch and pending changes. No prompt framework needed.
- **Full color.** 16, 256, and 24-bit color, with Aeon's own theme or yours.
- **No account, no telemetry.** It is a terminal.
- **Open.** MIT licensed. If you don't like something, fork it.

> Aeon is early. Command blocks, project tasks, split panes, and one palette for everything are on the way.

## Install

Download the MSI for your machine from the [latest release](https://github.com/UlloaSP/aeon/releases/latest):

| Machine          | File                               |
| ---------------- | ---------------------------------- |
| Most PCs         | `aeon-<version>-windows-x64.msi`   |
| ARM (Snapdragon) | `aeon-<version>-windows-arm64.msi` |

The installer adds Aeon to the Start menu and to your PATH, so you can also start it with `aeon` from any shell. Prefer no installer? Each release also has a portable `.zip` with just `aeon.exe`. `SHA256SUMS` lists the checksum of every file.

Nightly builds of `develop` are published as prereleases. They install over each other and over the stable release.

Aeon looks for new versions in the background, shortly after sign-in and every 4 hours. When one is out, the bottom of the sidebar says so: click it to download, then **Restart to update**. Nothing downloads or installs on its own, and the installer is checked against the release's SHA-256 before it runs. **Settings > About** checks on demand and switches between the Stable and Nightly channels.

Builds are not code-signed yet, so Windows SmartScreen may warn the first time you run the installer.

## Configuration

Press **Ctrl+,** (or click **Settings** at the bottom of the sidebar) to change the shell, font, size, padding, session header, launcher hotkey, and web search from inside Aeon. Changes apply at once and are saved to `config.toml` without touching anything else in it, comments included.

Aeon reads `%APPDATA%\aeon\config.toml`. Every setting is optional, and an unknown key is an error, so typos don't fail silently. If the file is invalid, Aeon starts with the defaults and says so in the window title.

### Sessions

One window holds as many shells as you want. The sidebar groups them by project: the git repository they run in, or their folder outside one, with your home folder under **Home**. Inside a project, a session is named after its shell, or after its folder when it is deeper in the repository; a session serving a port shows it (`:5173`). A new session starts in the folder of the one you are in, and Ctrl+Tab and Ctrl+1 to Ctrl+9 follow the sidebar's order.

| Keys | Action |
| --- | --- |
| Ctrl+Shift+T | New session (or click **New session**) |
| Ctrl+Shift+W | Close the session |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous session |
| Ctrl+1 to Ctrl+9 | Go to a session (or click it) |
| Ctrl+Shift+B | Show or hide the sidebar |

These are the defaults; change any of them in **Settings > Keyboard**. They use Ctrl+Shift because shells already use Ctrl+T, Ctrl+W, and Ctrl+B. Whether the sidebar is shown is remembered in `%APPDATA%\aeon\state`, apart from your `config.toml`.

### In the terminal

| Keys | Action |
| --- | --- |
| Mouse wheel, Shift+PageUp, Shift+PageDown | Scroll back through the last 5000 lines |
| Drag, double-click, triple-click | Select text, a word, or a line |
| Ctrl+C | Copy the selection (without one, it goes to the shell as usual) |
| Ctrl+V, Ctrl+Shift+V, right-click | Paste (right-click copies instead when text is selected) |
| Ctrl+Shift+C | Copy |
| Ctrl+Shift+F | Find in the output: Enter goes to the previous match, Shift+Enter to the next, Esc closes |
| Ctrl+click | Open a link: URLs in the browser, `file.rs:42:7` in VS Code at that line |
| Ctrl+=, Ctrl+-, Ctrl+0, Ctrl+wheel | Text size for this window |

Programs that use the mouse, such as vim or htop, get it; hold Shift to select text instead. Typing returns to the bottom.

### The launcher

Aeon starts with Windows and waits in the background, using about 8 MB. Press **Ctrl+Alt+Space** anywhere to open the launcher:

- Type a command, such as `code C:\code\app`, and press **Enter**. It starts in the background, the way Win+R would, and the launcher closes.
- **Ctrl+Enter** opens it in an Aeon terminal instead, so you can watch the output.
- Type part of an app's name, such as `disc` for Discord or `vsc` for Visual Studio Code, and press **Enter** to open it. Every app in the Start menu is there, Store apps included, and the ones you open most rise to the top.
- Type anything that is not a command or an app, and the first result searches the web for it in your default browser. Start with `?` to always search.
- Type `theme` to switch Windows between light and dark, or `theme dark` and `theme light` to pick one (`tema oscuro` and `tema claro` work too).
- Type math, such as `5*3-2` or `sqrt(2)`, and the result appears as you type. **Enter** copies it.
- The list shows what you ran from the launcher, then your PowerShell history in Aeon, filtered as you type. **Up** and **Down** pick one, and **Tab** puts it in the box to edit.

While it runs, Aeon has an icon in the notification area (under the taskbar's hidden icons). Click it to open the launcher, or right-click for a new terminal, your settings, or to quit.

To stop it from starting with Windows, turn off **Start with Windows** in Settings > General.

### Your shell inside Aeon

Aeon starts PowerShell **without your profile**, with its own prompt and its own command history in `%APPDATA%\aeon\history`, separate from other terminals. Prompt themes like Oh My Posh are not needed: the session header shows the same information, for every shell. cmd keeps its usual prompt and reports its directory to Aeon.

Aeon sets `TERM_PROGRAM=aeon` and `COLORTERM=truecolor` for everything it runs. To start a shell exactly as you configured it, profile included, set `integration = false` or give it `args`.

```toml
[shell]
program = "pwsh.exe"          # default: pwsh, then powershell, then cmd
args = ["-NoLogo"]
cwd = "C:\\code"              # default: where you launched Aeon, or your home folder
integration = true            # no profile, Aeon's prompt and history; off when args are set

[font]
path = "C:\\Windows\\Fonts\\CascadiaMono.ttf"   # default: Cascadia Mono, then Consolas
size = 11.0                   # points

[window]
columns = 120
rows = 32
padding = 8

[launcher]
hotkey = "Ctrl+Alt+Space"     # Ctrl, Alt, Shift, Win + a letter, digit, F1-F24, Space, or `
search_url = "https://www.google.com/search?q={query}"   # empty turns web search off
start_with_windows = true

[keys]                        # Aeon's shortcuts in the terminal window
new_session = "Ctrl+Shift+T"
close_session = "Ctrl+Shift+W"
next_session = "Ctrl+Tab"
previous_session = "Ctrl+Shift+Tab"
toggle_sidebar = "Ctrl+Shift+B"
settings = "Ctrl+,"
find = "Ctrl+Shift+F"
copy = "Ctrl+Shift+C"
paste = "Ctrl+Shift+V"

[terminal]
scrollback = 5000             # lines kept for scrolling back; 0 keeps none

[bar]
enabled = true                # the session header: project, git, and servers above the terminal

[cursor]
style = "block"               # block, bar, or underline

[updates]
channel = "stable"            # or "nightly": daily builds, and stable releases when newer
check = true                  # look for updates at sign-in and every 4 hours

[colors]
theme = "aeon"                # aeon, aeon-light, dracula, nord, gruvbox-dark, solarized-dark,
                              # solarized-light, catppuccin-mocha, one-dark, tokyo-night
cursor = "#ffb547"            # any color set here overrides the theme's:
                              # foreground, background, cursor, bar_background

[colors.normal]               # ANSI 0-7; any color left out keeps the theme's
red = "#e06c75"

[colors.bright]               # ANSI 8-15
red = "#ff7a85"
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
