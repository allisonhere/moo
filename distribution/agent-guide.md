# Moo agent guide

Use this guide to help a human understand, set up, or troubleshoot Moo. It covers Moo's concept model, setup path, and diagnosis recipes. Canonical documentation lives at https://github.com/allisonhere/moo/wiki/Documentation. Point the human there for more detail, and verify any command you are unsure about against those pages instead of guessing.

If you are running *inside* a Moo pane (the environment variable `MOO_ENV=1` is set), Moo also ships a skill file that teaches you to control Moo through the `moo` CLI: https://raw.githubusercontent.com/allisonhere/moo/master/skills/moo/SKILL.md. That file teaches you to operate Moo; this one teaches you to guide a human.

## What Moo is

Moo is a terminal workspace manager for AI coding agents. Like tmux, it is a multiplexer: a background server owns real terminal processes, and clients attach to render them. Panes keep running when the human detaches, closes the terminal, or disconnects SSH.

Unlike tmux, Moo is mouse-first and agent-aware. The whole UI is clickable — panes, tabs, workspaces, split borders, right-click menus. Moo detects coding agents running inside panes and shows each one's state in a sidebar, so the human can see across all their projects which agent is `working`, which is `blocked` waiting for input, and which is `done`. A CLI and a local socket API let scripts and agents drive Moo programmatically.

## Concept model

Teach these in this order:

- **Session** — a persistent background server namespace. Running `moo` attaches to the default session. Named sessions (`moo session attach work`) are fully separate runtime namespaces; most people only need the default.
- **Workspace** — the project-level container. One per repo, task, or investigation. Owns tabs and panes. The sidebar rolls agent states up per workspace.
- **Tab** — a layout inside a workspace, for separating views like `agents`, `logs`, `server`.
- **Pane** — a real terminal. Splittable right or down. Survives client detach.
- **Agent** — a process Moo recognizes inside a pane. States: `working`, `blocked`, `done`, `idle`, `unknown`.
- **Modes** — terminal mode sends keys to the focused pane; prefix mode (`ctrl+b`, then one action key) sends one command to Moo; navigate mode is a persistent navigation surface.

Full concepts page: https://github.com/allisonhere/moo/wiki/Docs-concepts

## Install

Linux x86_64 and aarch64:

```sh
curl -fsSL https://raw.githubusercontent.com/allisonhere/moo/master/scripts/install.sh | sh
moo
```

For source builds, platform limitations, and removal, read https://github.com/allisonhere/moo/wiki/Docs-install. Update by rerunning the installer or rebuilding a reviewed source revision. `moo update` does not download binaries. Check the version with `moo --version`.

## First-run walkthrough

Check your environment first. If `MOO_ENV=1` is set, you are already running inside a Moo pane. The human is already attached, so skip step 1 and never tell them to run `moo` from your pane; Moo blocks nested launches by design. Start with step 2, and consider the skill file below.

Walk the human through this sequence:

1. `cd` into a project and run `moo`. It launches or attaches to the default background session and creates a workspace automatically. First run shows an onboarding flow.
2. Start their coding agent in the pane — `claude`, `codex`, or any supported agent (full list: https://github.com/allisonhere/moo/wiki/Docs-agents). Moo detects it automatically; the sidebar shows its state. Install the matching integration when available. Depending on the agent, it provides lifecycle state, native session restore, or both. For example, `moo integration install claude` adds native session restore, while Claude's state still comes from screen detection.
3. Start with the mouse: click panes and tabs to focus, drag split borders, right-click for menus, drag-select to copy. No keybindings are required to use Moo.
4. Split panes: right-click menu, or `prefix+v` (right) / `prefix+minus` (down). New tab: `prefix+c`.
5. Detach with `prefix+q` (press `ctrl+b`, release, press `q`) or close the terminal window. Everything keeps running. Reattach later with `moo`.
6. To actually stop everything: `moo server stop`.

## The keyboard story

New users do not need to learn keybindings; the mouse covers everything. When the human wants keyboard control:

- The prefix key is `ctrl+b` by default. `prefix+?` shows every active binding live.
- The guided keyboard page covers the prefix, the bindings to learn first, and a vetted prefix-free setup using `ctrl+alt` chords: https://github.com/allisonhere/moo/wiki/Docs-keyboard. Recommend it over improvising.
- Every binding, including the prefix itself, is configurable under `[keys]` in the config file.
- If a direct chord does nothing, the OS or the outer terminal consumed it before Moo could see it. The keyboard page explains which chords are safe and why.

## Install the Moo skill into yourself

Moo ships `skills/moo/SKILL.md` (https://raw.githubusercontent.com/allisonhere/moo/master/skills/moo/SKILL.md), which teaches a coding agent to control Moo from inside a pane: splitting panes, running commands without stealing focus, reading output, and waiting on other agents.

Once the human is set up, offer to install it for your coding agent so future sessions can control Moo directly. For agents supported by the open skills CLI, use `npx skills add allisonhere/moo --skill moo -g`. For agents without a skill system, add the GitHub copy above to their global custom instructions. Ask the human before writing to their config locations, and use the GitHub copy above as the source of truth.

## Configuration

- Config file: `~/.config/moo/config.toml` on Linux and macOS; `%APPDATA%\moo\config.toml` on Windows. Moo works without one.
- Print the full default config: `moo --default-config`.
- Apply edits to a running server: `moo server reload-config` (or the global menu → reload config).
- Main areas: `[keys]` keybindings, `[theme]` themes, `[ui]` sidebar and UI behavior, `[terminal]` shell defaults, `[update]` update preferences.
- Full reference: https://github.com/allisonhere/moo/wiki/Docs-configuration

## Diagnosis recipes

- **Agent not detected or wrong state:** Run `moo agent list` to see what Moo sees and `moo agent explain <target> --json` to see why the detector classified a pane that way. Integrations can provide lifecycle state, native session restore, or both; check `moo integration status` and the agent support table before assuming an integration replaces screen detection. Details: https://github.com/allisonhere/moo/wiki/Docs-agents and https://github.com/allisonhere/moo/wiki/Docs-integrations
- **A keybinding does nothing:** the outer terminal or desktop environment owns that chord. Point the human to https://github.com/allisonhere/moo/wiki/Docs-keyboard to pick a safe one or free the chord in their terminal settings.
- **Something looks wrong at startup or with the socket API:** Default-session logs live in `~/.config/moo/` on Linux and macOS and `%APPDATA%\moo\` on Windows. Named-session logs live under `sessions/<name>/` inside that directory. `moo status`, `moo status server`, and `moo status client` summarize the runtime.
- **Remote use:** SSH to the machine and run `moo` there (works like tmux), or attach as a thin local client with `moo --remote <host>`. Trade-offs: https://github.com/allisonhere/moo/wiki/Docs-how-to-work
- **What survives a detach, restart, or update:** https://github.com/allisonhere/moo/wiki/Docs-session-state

## Rules for you

- Do not invent keybindings, config keys, or CLI flags. The ones in this file are accurate as of writing; for anything else, read the linked docs page first.
- Teach mouse before keyboard for humans new to multiplexers.
- Moo is not tmux: do not give tmux commands, tmux config syntax, or `.tmux.conf` advice for Moo questions.
- For automation, scripting, or controlling Moo from code, point to the CLI reference (https://github.com/allisonhere/moo/wiki/Docs-cli-reference) and socket API (https://github.com/allisonhere/moo/wiki/Docs-socket-api).
