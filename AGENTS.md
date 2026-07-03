# AGENTS.md

This repo contains `chooser`, the AirTalk AppleTalk service browser for LINUX
Unplugged. It is a Rust CLI/TUI that uses TailTalk's userspace AppleTalk stack.

## Ground Rules

- Do not implement AppleTalk, NBP, DDP, packet parsing, or raw socket handling
  here. Use TailTalk.
- Keep TailTalk pinned in `Cargo.toml` unless the task explicitly asks for an
  upstream update.
- Do not use or modify `tailtalk-gui`; this project is a separate terminal app.
- Keep the app useful on an isolated demo LAN with no internet after the build
  inputs have been fetched.
- Prefer small Rust changes and verify with real commands.

## Setup

Use the Nix development shell on NixOS:

```sh
nix develop
cargo build --release
```

EtherTalk needs raw socket privileges. For local demo runs, build into
`target/release` and grant the local binary the capability:

```sh
sudo setcap cap_net_raw+eip target/release/chooser
./target/release/chooser --interface eth0
```

If the binary is rebuilt, run `setcap` again. TashTalk-only runs do not need
raw socket capability, but the user must be able to access the serial device.

`nix run` is useful for help output and packaging smoke tests. For EtherTalk,
prefer the local `target/release` plus `setcap` workflow because Nix store paths
are immutable.

## Validation

Run the narrow checks first:

```sh
nix develop --command cargo fmt -- --check
nix develop --command cargo build --release
nix build
./target/release/chooser --help
```

Useful runtime checks:

```sh
./target/release/chooser --plain --interface eth0
./target/release/chooser --interface eth0
```

On non-demo interfaces, stack setup or lookups may fail because TailTalk needs a
real EtherTalk-capable LAN and raw socket access. Handle those errors without
panics.

## UX Notes

The default UI is a modern terminal app inspired by the classic Mac OS Chooser:
service types on the left, discovered services on the right, and selected
service details at the bottom. Keep the UI readable on a projector and reliable
over pixel-perfect nostalgia.

Keyboard controls:

- `q` or Ctrl-C: quit
- `r`: refresh immediately
- Arrow keys: move selection
- `/`: edit filter
- Enter or Esc: finish editing filter
- Esc outside filter mode: clear filter
