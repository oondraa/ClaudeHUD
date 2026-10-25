# ClaudeHUD

Web: https://oondraa.github.io/ClaudeHUD/

A companion for Claude Code on Windows. A small glass island sits in the
bottom-right corner, just above the taskbar, summarizing every session: how
many are working and for how long, which one finished, which one waits for you.
When a session finishes, fails or needs permission, a card rises above the
island so you can **Povolit / Otevřít / Zamítnout** right there, or jump to the
session's terminal.

Download `ClaudeHUD.exe` from the [latest release](https://github.com/oondraa/ClaudeHUD/releases/latest)
and run it. On every start it points Claude Code's hooks at itself
(`ClaudeHUD.exe hook`, in `~/.claude/settings.json`, with a backup) and checks
GitHub for a newer release; when there is one, a card offers **Aktualizovat**:
it downloads the exe, verifies it against `SHA256SUMS.txt`, swaps itself and
restarts. Restart running Claude Code sessions after the first start. To remove
the hooks, use **Odebrat hooky a ukončit** in the tray menu.

From source:

```
cargo build --release
target\release\claudehud.exe
```

To publish a release, bump `version` in `Cargo.toml` and push a matching tag
(`v0.1.1`); the Release workflow builds and uploads `ClaudeHUD.exe` and
`SHA256SUMS.txt`.

## How it works

- `ClaudeHUD.exe hook` is registered as a Claude Code command hook for
  `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`,
  `Notification`, `Stop`, `StopFailure` and `SessionEnd`. It forwards the event JSON, plus the
  terminal window it runs under (found by walking up the parent processes), to
  the HUD over `127.0.0.1:47821` (override with `CLAUDEHUD_PORT`).
- Every hook except `PermissionRequest` is `async`, so Claude Code never waits on it.
  `PermissionRequest` blocks until you click; **Otevřít** falls back to the normal
  prompt in the terminal.
- Inter (SIL OFL) is bundled from `assety/fonts/`.
- If the HUD isn't running, the hook exits immediately and Claude Code behaves as usual.

## Using it

- **Island** (bottom-right): resting it is three faint dots; otherwise it shows
  the most urgent state — waiting (pulsing amber, with how long), finished
  (green, with **Přepnout**), failed (red), or working (spinner, longest run,
  avatars). By default it lets clicks through to the window below; hold **Alt**
  to click it. Click it to open the panel.
- **Cards**: finished / waiting / failed cards fold away after 8 s (pauses
  while hovered; configurable), older ones stack behind. Permission cards stay
  until answered; **Otevřít** falls back to the prompt in the terminal.
- **Panel**: every session with its state and timer; click a row to focus its
  terminal. The gear opens the settings.
- **Shortcuts**: `Ctrl+Alt+Space` jumps to the last finished session,
  `Ctrl+Alt+H` toggles the panel.
- **Sound**: a quiet synthesized chime instead of the Windows sound. It stays
  silent when you're already looking at that session's terminal, while a
  full-screen app runs, and a "done" chime never follows another sound within
  30 s, so agents finishing in a burst give one chime. Switch to the system
  sound or off in the settings.
- **Settings** (gear in the panel, tray menu, or `claudehud.exe --settings`):
  click-through, dim while typing, hide during full-screen apps, collapse time,
  which events to report, sound. Stored in `%APPDATA%\ClaudeHUD\settings.json`.
- **Tray icon**: the Claude mascot from `assety/`, colored by the most urgent
  state; right-click for the panel, settings and Quit.

`claudehud-hook.exe probe` prints which window the hook would jump to from the
current shell.
