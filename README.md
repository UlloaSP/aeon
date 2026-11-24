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
- **Every shell at a glance.** The sidebar lists your sessions by repository and shell (`aeon`, PowerShell; `WSL · kali-linux`), with what each is doing and the servers it started (`:5173`). A header above the terminal shows the shell, folder, git branch, and pending changes. No prompt framework needed.
- **Full color.** 16, 256, and 24-bit color, with Aeon's own theme or yours.
- **No account, no telemetry.** It is a terminal.
- **Open.** MIT licensed. If you don't like something, fork it.

> Aeon is early. Expect rough edges, and tell us about them.

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

One window holds as many shells as you want, listed in the sidebar in the order you opened them. A session is named after the repository it is in, or after its shell outside one; under its name the sidebar says which shell it is (PowerShell, Windows PowerShell, Command Prompt, Git Bash, `WSL · kali-linux`, `SSH · host`), or its folder when the name already does. The header and the window title say the same, and the header adds its folder and branch. A session serving a port shows it (`:5173`). New sessions start in the folder of the one you are in.

**New session** (or Ctrl+Shift+T) lists what it can start: your configured shell first (**Enter** opens it), then the other shells this PC has (PowerShell 7, Windows PowerShell, Command Prompt, Git Bash), each WSL distro, each host in `~/.ssh/config`, and Aeon as administrator in a separate window. Type to narrow the list.

| Keys | Action |
| --- | --- |
| Ctrl+Shift+T | New session: pick a shell, distro, or host (or click **New session**) |
| Ctrl+Shift+W | Close the pane, or the session when it has one |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous session |
| Ctrl+1 to Ctrl+9 | Go to a session (or click it) |
| Ctrl+Shift+D, Ctrl+Shift+E | Split the pane to the right, or down |
| Alt+Arrows | Move to the pane in that direction (or click it) |
| Ctrl+Shift+Z | Show only this pane, or all of them again |
| F2 | Rename the session; Enter keeps the name, Escape drops it, and an empty name goes back to the automatic one |
| Ctrl+Shift+K | The palette, over this window |
| Ctrl+Shift+B | Show or hide the sidebar |

These are the defaults; change any of them except Alt+Arrows in **Settings > Keyboard**. They use Ctrl+Shift because shells already use Ctrl+T, Ctrl+W, and Ctrl+B.

A split starts its shell in the folder of the pane you split. Typing `exit` closes the pane. Closing a pane, a session, or the window while a command is still running asks first; that needs shell integration, which is what reports commands.

When Aeon starts with no other window open, it reopens the sessions the last window had: their names, splits, and folders, each with a fresh shell. Started from a shell in a folder none of them is in, it adds a session there. Turn this off with `restore = false` under `[window]`. Sessions started with a command (the launcher's Ctrl+Enter) start fresh. What to reopen, and whether the sidebar is shown, is kept in `%APPDATA%\aeon\state`, apart from your `config.toml`.

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

### Projects

The project panel (**Ctrl+Shift+P**, or the button at the right of the header) shows what the active session's project can run and what it is running:

- **Tasks** found in `package.json` (run with pnpm, yarn, bun, or npm, whichever the project uses), a `justfile`, a `Makefile`, or `Cargo.toml`. **Run** opens a session named after the task at the project's top; **Stop** sends it Ctrl+C, and the session keeps its output.
- **Servers** the project's sessions started, with **Open** (in the browser) and **Stop**.
- **Git**: the branch, how far ahead or behind it is, and the changed files.

A session's project is its repository, or its folder outside one. **Open in VS Code** in the header opens it when VS Code is installed. Whether the panel is open is remembered in `%APPDATA%\aeon\state`.

### Commands and what they did

In every shell Aeon sets up, every command is a block: a line above it, and at the end of its prompt line how long it took (`3.4 s`), or `exit 1` in red for a failure, with a red bar down the block's left side and the block tinted. In PowerShell, a line of several statements (`npm run build; npm test`) becomes one block per statement, each run on its own after a quiet prompt line naming it, so each is formatted on its own; `&&` and `||` chains stay one block, and Ctrl+C stops the rest of the line. Hover a command for a copy button at its left, which copies the command, and **Copy output** and **Rerun** at its right. **Ctrl+Up** and **Ctrl+Down** jump between commands. cmd marks only its prompts, so there a block runs from Enter to the next prompt, and cmd cannot say how a command ended. When the window changes width, the shell redraws the screen rewrapped and the blocks follow their lines.

The sidebar shows what each session is doing: a blue dot while a command runs, its port while it serves one, the progress a program reports (`64%`), and, for a session you are not looking at, `exit 1` in red, `done` in green, or `bell`, in bold until you open it. When a command that ran for 10 seconds or more ends in a session you are not looking at, Windows shows a notification and the taskbar button flashes. The bell at the end of a session row (on hover) mutes its notifications.

### The palette

**Ctrl+Shift+K** in a window opens the same search as the launcher below, plus what that window has: its sessions (the empty box lists them; **Enter** goes to one), the active project's tasks, and new sessions. **Enter** on a command types it at the focused prompt, and **Ctrl+Enter** runs it in a new session.

In the palette, type `new`, `wsl`, `ssh`, or a name to find what **New session** lists. A split starts the same shell as the pane it splits, and restoring a session restarts each pane's own shell.

### The launcher

Aeon starts with Windows and waits in the background, using about 8 MB. Press **Ctrl+Alt+Space** anywhere to open the launcher, the palette without a window:

- Type a command, such as `code C:\code\app`, and press **Enter**. It starts in the background, the way Win+R would, and the launcher closes.
- **Ctrl+Enter** opens it in an Aeon terminal instead, so you can watch the output.
- Type part of an app's name, such as `disc` for Discord or `vsc` for Visual Studio Code, and press **Enter** to open it. Every app in the Start menu is there, Store apps included, and the ones you open most rise to the top.
- Type anything that is not a command or an app, and the first result searches the web for it in your default browser. Start with `?` to always search.
- Type `new`, `wsl`, or `ssh` to open a terminal with another shell, distro, or host.
- Type `theme` to switch Windows between light and dark, or `theme dark` and `theme light` to pick one (`tema oscuro` and `tema claro` work too).
- Type math, such as `5*3-2` or `sqrt(2)`, and the result appears as you type. **Enter** copies it.
- The list shows what you ran from the launcher, then your PowerShell history in Aeon, filtered as you type. **Up** and **Down** pick one, and **Tab** puts it in the box to edit.

While it runs, Aeon has an icon in the notification area (under the taskbar's hidden icons). Click it to open the launcher, or right-click for a new terminal, your settings, or to quit.

To stop it from starting with Windows, turn off **Start with Windows** in Settings > General.

### The drop-down terminal

Press **Ctrl+Alt+`** anywhere and a terminal drops down across the top of the screen you are on. Press it again, or click elsewhere, and it gets out of the way with its sessions still running; it comes back as you left it. The background launcher handles this key, so it needs **Start with Windows** (or an Aeon running in the background). Change the key in Settings > Launcher, or set `dropdown = ""` to turn it off.

### Your shell inside Aeon

PowerShell loads your profile (your PATH, modules such as Terminal-Icons, PSReadLine settings), then takes Aeon's prompt, colours for what you type (commands, parameters, strings, and variables, in bold, unless your profile sets PSReadLine's own), and its own command history in `%APPDATA%\aeon\history`, separate from other terminals. A prompt theme such as Oh My Posh is replaced: the session header shows the same information, for every shell. Your profile can make a shell take a second or more to start; `profile = false` skips it. cmd reports its directory to Aeon too.

Characters your terminal font lacks, such as Nerd Font icons, are drawn from Segoe UI Symbol or an installed Nerd Font (FiraCode, Caskaydia, JetBrains Mono, Hack, Meslo, or Symbols Nerd Font).

In WSL and Git Bash, Aeon starts your account's shell (bash or zsh) as a login would, with your own files (`.profile`, `.bashrc`, `.zshrc`), then gives it Aeon's prompt, command blocks, the folder in the header, and the same colours for what you type: zsh-syntax-highlighting takes them as you type, and otherwise the line takes them when you press Enter. Folders on Windows drives, which `ls` would paint with a background, show as plain folders. cmd gets Aeon's prompt too, but cannot colour what you type. Git shows for folders under `/mnt/c`; inside the Linux file system Windows' git cannot read the repository.

Aeon sets `TERM_PROGRAM=aeon` and `COLORTERM=truecolor` for everything it runs. To start a shell exactly as you configured it, prompt included, set `integration = false` or give it `args`.

```toml
[shell]
program = "pwsh.exe"          # default: pwsh, then powershell, then cmd
args = ["-NoLogo"]
cwd = "C:\\code"              # default: where you launched Aeon, or your home folder
integration = true            # Aeon's prompt, blocks, and history; off when args are set
profile = true                # PowerShell loads your profile first

[font]
path = "C:\\Windows\\Fonts\\CascadiaMono.ttf"   # default: Cascadia Mono, then Consolas
size = 11.0                   # points
line_height = 1.2             # rows as a multiple of the font's line height

[window]
columns = 120
rows = 32
padding = 8
restore = true                # reopen the last window's sessions

[launcher]
hotkey = "Ctrl+Alt+Space"     # Ctrl, Alt, Shift, Win + a letter, digit, F1-F24, Space, or `
dropdown = "Ctrl+Alt+`"       # the drop-down terminal; empty turns it off
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
project_panel = "Ctrl+Shift+P"
split_right = "Ctrl+Shift+D"
split_down = "Ctrl+Shift+E"
zoom_pane = "Ctrl+Shift+Z"
rename_session = "F2"
palette = "Ctrl+Shift+K"

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
