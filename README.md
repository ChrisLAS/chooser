# chooser

`chooser` is the AirTalk AppleTalk service browser for LINUX Unplugged. It uses
TailTalk's userspace AppleTalk stack to run NBP lookups and displays discovered
services in a terminal UI inspired by the classic Mac OS Chooser.

It is intentionally a wrapper around TailTalk. It does not implement AppleTalk
protocols or packet parsing itself.

## Build on NixOS

```sh
nix develop
cargo build --release
```

The flake also exposes a package and app:

```sh
nix build
nix run . -- --help
```

For EtherTalk demo runs, prefer the local Cargo-built binary so you can grant
raw socket capability to that file.

## EtherTalk Permissions

EtherTalk uses a raw socket. Run as root, or grant the release binary raw socket
capability after building:

```sh
sudo setcap cap_net_raw+eip target/release/chooser
```

If you rebuild the binary, run `setcap` again.

TashTalk-only use through a serial device does not need raw socket capability,
but your user still needs permission to access the serial device.

## Run

Browse all visible AppleTalk services on an Ethernet interface:

```sh
./target/release/chooser --interface eth0
```

Use a TashTalk USB serial adapter:

```sh
./target/release/chooser --tashtalk /dev/ttyUSB0
```

Use a different refresh interval:

```sh
./target/release/chooser --interface eth0 --refresh 5
```

Query a specific NBP entity instead of the default wildcard `=:=@*`:

```sh
./target/release/chooser --interface eth0 --entity '=:LaserWriter@*'
```

Use the plain table fallback:

```sh
./target/release/chooser --plain --interface eth0
```

## TUI Controls

- `q` or Ctrl-C: quit
- `r`: refresh immediately
- Arrow keys: move selection
- `/`: edit filter
- Enter or Esc: finish editing filter
- Esc outside filter mode: clear filter

## TailTalk Dependency

`chooser` depends on TailTalk and `tailtalk-packets` from
`https://github.com/FeralFirmware/TailTalk.git`, pinned in `Cargo.toml`. This
keeps the public repo buildable without requiring a sibling TailTalk checkout.

## License

GPL-3.0-only. TailTalk is GPLv3, so `chooser` is GPLv3 as well.
