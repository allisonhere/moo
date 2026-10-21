# herdr-forge

<p align="center">
  <img src="assets/logo.png" alt="herdr-forge" width="100" />
</p>

A fork of [Herdr](https://github.com/herdrdev/herdr), the terminal runtime for coding agents. It keeps Herdr's detachable server, panes, and agent status, and adds agent-focused features. It installs and runs beside upstream Herdr without sharing state.

> **Status:** the Rust test suite has not yet been run for the fork-specific changes. Static checks pass; runtime behavior is not verified. Treat builds as experimental.

## what's different

- **separate identity** — the binary is `herdr-forge`. Config, state, logs, and sockets live under `herdr-forge` paths, so it never touches an upstream install. Debug builds use `herdr-forge-dev`.
- **agent-aware command palette** — open it with `prefix+space` (default prefix `ctrl+b`) and search actions by name.
- **agent names in the sidebar** — rename an agent from the sidebar or with `herdr-forge agent rename <target> <name>`; `--clear` resets it.
- **attention queue** — agents that need an answer are queued, and you can acknowledge them.
- **durable status history** — agent status changes are recorded and exposed through the API as `agent.history`.
- **fork-only remotes** — remote attach and saved SSH machines look for `herdr-forge` on the remote host and never fetch upstream `herdr`. Automated Windows install is refused for now.

Everything else follows upstream Herdr, with `herdr-forge` in place of `herdr`.

## install

Requires a Rust toolchain and Zig 0.16.0 (for the vendored libghostty-vt).

```bash
git clone https://github.com/allisonhere/herdr
cd herdr
./scripts/install-herdr-forge.sh
```

This builds the locked release binary and installs it to `~/.local/bin`; set `HERDR_FORGE_INSTALL_DIR` to change that. Then run:

```bash
herdr-forge
```

`ctrl+b q` detaches and `herdr-forge` reattaches. `herdr-forge update` does not download anything; rerun the install script to update.

## docs

The fork guide is [`docs/next/website/src/content/docs/herdr-forge.mdx`](docs/next/website/src/content/docs/herdr-forge.mdx): requirements, identity paths, troubleshooting, removal, and current limitations. Upstream docs at [herdr.dev/docs](https://herdr.dev/docs/) apply to everything the fork has not changed.

## development

```bash
cargo build --release --locked --bin herdr-forge
just test        # unit tests
just check       # formatting, tests, and maintenance checks
```

Read [`AGENTS.md`](./AGENTS.md) and [`CLAUDE.md`](./CLAUDE.md) before making changes.

## license

Apache License 2.0, the same as upstream Herdr. See [LICENSE](LICENSE). Herdr is by [herdrdev](https://github.com/herdrdev/herdr).
