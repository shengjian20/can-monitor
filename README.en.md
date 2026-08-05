# can-monitor

[![CI](https://github.com/raw/can-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/raw/can-monitor/actions/workflows/ci.yml)
<!-- The CI badge is a placeholder: it activates once the repo is public and GitHub Actions has run. If the public repo lives elsewhere, replace raw/can-monitor with the actual owner/repo. -->

中文版: [README.md](README.md)

A cross-platform CAN bus monitor. One core, three front ends:

- **TUI** — live terminal monitoring (ratatui), aimed at embedded debugging and production-line diagnostics
- **Web** — browser UI (React SPA over REST/WebSocket), a loopback-only local service
- **GUI** — desktop app (Tauri v2), a native shell around the same Web UI

Two backends (Linux SocketCAN / ZLG USB-CAN), two protocol parsers (CANopen CiA 301 / SAE J1939), plus automatic device discovery, frame filtering, protocol highlighting, CANopen transmit, and candump-compatible logging.

> **Screenshots**: none yet. Real captures of the TUI / Web / GUI forms will be added once the repo is public.

## Features

**Three forms**

| Form | Stack | How to run |
|------|-------|------------|
| TUI | Rust + ratatui + crossterm | `cargo run -- --backend none` |
| Web | Rust (axum REST/WS) + React SPA | `cargo run -- --backend none --web-write`, then open `http://127.0.0.1:8080` |
| GUI | Tauri v2 (Web front end + Rust back end) | `cd src-tauri && cargo tauri dev` |

**Bus backends**

- **Linux SocketCAN**: classic CAN + CAN FD (`--fd`), auto-discovery of `can0` / `vcan0` etc. via `/sys/class/net`
- **ZLG USB-CAN**: USBCAN-I/II, USBCAN-E-U/2E-U and other VCI-compatible devices. `ControlCAN.dll` / `libcontrolcan.so` is loaded at runtime, with hot-plug auto-reconnect (re-enumeration after >= 2 s, up to 5 tries). **Classic CAN only, no CANFD** (hardware limitation)
- **No-device mode** (`--backend none`): run the TUI or Web UI with zero hardware for smoke tests and debugging

**Protocol parsing**

- 11-bit standard frames → **CANopen** (CiA 301): NMT / SDO / PDO / EMCY / SYNC / TIME / heartbeat, with node health monitoring and a transmit panel
- 29-bit extended frames → **J1939**: PGN bit-field parsing + TP.BAM multi-packet reassembly (up to 1785 bytes, DM1/DM2 diagnostic classification)
- Everything else → Raw

**Engineering highlights**

- Classify once: each frame is classified exactly once in the reader thread, and the result travels with the frame as a `StreamItem` to every consumer (TUI / Web / GUI). No re-classification anywhere
- Bounded fan-out: the broadcast layer gives each consumer an independent bounded queue (1024), slow consumers drop new frames and the count shows up as `dropped`. The reader is **never blocked**
- Device discovery abstraction: the `DeviceDiscoverer` trait, with `can-devices` aggregating SocketCAN + USB-CAN into one list
- Web write gate: `POST /api/send` only works when the server was started with `--web-write`; read-only by default, loopback bind only
- Three-platform CI: cross-check on Ubuntu / Windows / macOS, full quality gates on Linux (test / clippy / fmt / core purity)
- 215 cargo unit tests + 7 Playwright e2e tests

## Quick start

Prerequisites: Rust stable (1.88 or newer recommended). On Linux, the TUI and Web forms need no extra system packages; the GUI form needs webkit2gtk / dbus (see below).

### Form 1: TUI (terminal)

```bash
# No hardware, just look at the UI
cargo run -- --backend none

# Against a SocketCAN interface (can0, or virtual vcan0)
cargo run -- --backend socketcan --iface can0

# No hardware? Use vcan0 (needs can-utils to inject frames)
bash scripts/vcan-setup.sh
cansend vcan0 181#01020304          # other terminal: CANopen TPDO1
cargo run -- --backend socketcan --iface vcan0
```

Inside the TUI, press `SPACE` to start monitoring, `q` to quit. Keymap below.

### Form 2: Web (browser)

```bash
# 1. Build the front end once (the backend serves web/dist)
cd web && npm install && npm run build && cd ..

# 2. Start the backend (REST + WebSocket + static page)
cargo run -- --backend none --web-write
```

Open <http://127.0.0.1:8080> in a browser. `--web-write` is what allows sending frames from the UI; without it the web server does not start at all (the CLI only launches the service in write mode).

Front-end development mode (hot reload):

```bash
# Terminal 1: backend (REST/WS data source, listening on 8080)
cargo run -- --backend none --web-write
# Terminal 2: Vite dev server (page on 1420, data still on 8080)
cd web && npm run dev
# Open http://localhost:1420
```

### Form 3: GUI (desktop)

```bash
cd src-tauri && cargo tauri dev
```

On Linux this needs system packages: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` (and `libdbus-1-dev`). The bundled devcontainer already has the full toolchain. The GUI shares the same React code as the Web form (`web/`) and talks to Rust over Tauri IPC (invoke + Channel), with commands mirroring the Web API one to one.

## Device support

| Backend | Platform | Interface / device | Auto-discovery | CANFD | Notes |
|---------|----------|--------------------|----------------|-------|-------|
| SocketCAN | Linux | `can0` / `vcan0` etc. | Yes, sysfs scan (`/sys/class/net`) | Yes (`--fd`) | Matches `can*` / `vcan*` / `slcan*` prefixes |
| USB-CAN | Linux (x86_64 / aarch64) | USBCAN-I/II, USBCAN-E-U/2E-U and other VCI devices | Yes, `VCI_FindUsbDevice2` | No, hardware limitation | Fixed 500 kbit/s, loads `ControlCAN.dll` / `libcontrolcan.so` at runtime |
| None | Any | — | — | — | `--backend none` debug mode, no bus attached |

- **CANFD note**: USB-CAN hardware does not support CANFD; FD frames return `Unsupported`. Only the SocketCAN backend enables FD via `--fd`
- **Vendor library**: the ControlCAN library used by `can-usbvci` comes from the vendor-bundled `CAN分析仪资料20250624_Linux` directory (Linux package V1.45) and is committed under `third_party/controlcan/{aarch64,x86_64}/`. Origin and distribution notes live in [docs/VENDOR.md](docs/VENDOR.md); re-copy with `bash scripts/fetch-vendor.sh`
- **USB permissions**: the running user needs read/write access to `/dev/bus/usb/*/*` (set up your own udev rules or run as root). The repo ships no udev rules
- Adding a new device (third-party adapter / test stub): see the [docs/devices.md](docs/devices.md) extension guide

## Cross-compilation (aarch64 / RK3588)

Target: aarch64, glibc >= 2.23 (verified on Ubuntu 16.04 / RK3588).

```bash
bash scripts/build-cross.sh                 # build the whole workspace
bash scripts/build-cross.sh -p can-monitor  # build only can-monitor
```

Output: `target/aarch64-unknown-linux-gnu/release/can-monitor`. The script runs `rustup target add`, fixes the vendor `.so` SONAME (`patchelf --set-soname libcontrolcan.so`), builds with `cargo zigbuild`, and double-checks the result (`file` architecture + `readelf -V` glibc <= 2.23).

Deployment: copy the binary together with `libcontrolcan.so`, then set `LD_LIBRARY_PATH` (dynamic loading looks in the exe directory first, then `LD_LIBRARY_PATH`, then the system search path). See [docs/architecture.md](docs/architecture.md).

## Command line

```
can-monitor [options]

--backend <socketcan|usbvci|none>  backend type (default none)
--iface <name>                     SocketCAN interface name (default can0)
--fd                               enable CANFD (SocketCAN only)
--log-file <path>                  log file path (candump -L format, append)
--web-write                        enable web write mode (also starts HTTP; read-only by default)
--web-port <host:port>             web listen address (default 127.0.0.1:8080; loopback only)
--help, -h                         show help
```

Examples:

```bash
can-monitor --backend socketcan --iface can0 --log-file /tmp/can.log
can-monitor --backend usbvci --iface can0          # --iface is display-only for USB-CAN
can-monitor --backend none --web-write             # no hardware + web UI
can-monitor --backend none                         # TUI only, no backend
```

## Keymap (TUI)

| Key | Action |
|-----|--------|
| `q` | Quit |
| `SPACE` / `s` | Toggle monitoring (off by default) |
| `f` | Toggle filtering |
| `l` | Toggle logging (requires `--log-file`) |
| `x` | Open the CANopen transmit panel |
| `↑` / `↓` | Scroll the message list |
| `PageUp` / `PageDown` | Page (10 rows) |
| `End` | Jump to the newest frame (resume tail-follow) |

Inside the transmit panel: `Esc` cancel, `Tab` switch field, `Enter` confirm / send, `1`-`4` pick a service (NMT / SDO read / SDO write / raw frame), `0-9 a-f` type hex.

## Testing

```bash
cargo test                                   # all crate unit tests (215)
cargo test -p can-usbvci --features mock     # USB-CAN mock tests (no hardware / vendor lib)
cargo test -p can-socketcan --features vcan-test  # vcan0 integration tests (needs local vcan0)
cd web && npm run test:e2e                   # Playwright e2e (7 cases, backend must run with --web-write)
```

Quality gates: `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings), `cargo fmt --check`, `bash scripts/check-core-purity.sh` (core has no UI deps, whitelisted deps only).

## Layout

```
crates/
  can-types/            protocol-agnostic contract layer (CanBackend / CanFrame / CanId / CanDeviceInfo / DeviceDiscoverer)
  can-socketcan/        Linux SocketCAN backend (classic + FD, sysfs device discovery)
  can-usbvci/           ZLG USB-CAN backend (VCI dynamic loading + discovery)
  can-devices/          device discovery aggregation (SocketCAN + USBCAN, one list)
  canopen-stack/        CANopen (CiA 301) parser and transmit services
  j1939-stack/          J1939 parser (incl. TP multi-packet reassembly)
  can-monitor-core/     core stream (bus / single classification / filter / logger / fan-out / CLI)
  can-monitor-server/   web service (axum REST + WebSocket batched frames + static hosting)
  can-monitor/          TUI binary
src-tauri/              Tauri v2 desktop GUI (7 IPC commands + Channel frame stream)
web/                    React SPA (Vite + TS, Tauri/browser mode auto-detection)
scripts/                build / deploy / test scripts
third_party/controlcan/ vendor library (aarch64/ + x86_64/, committed)
docs/                   architecture / devices / web API / vendor notes
```

## Documentation

- [docs/architecture.md](docs/architecture.md) — core layering, fan-out broadcast, classify-once, three-form wiring
- [docs/devices.md](docs/devices.md) — device extension guide: implement `DeviceDiscoverer` + `CanBackend` + `BackendConfig`
- [docs/web-api.md](docs/web-api.md) — Web API: REST endpoints / WebSocket contract / frame JSON schema / error codes
- [docs/VENDOR.md](docs/VENDOR.md) — vendor SDK origin and distribution notes

## Known limitations

- The USB-CAN backend is classic CAN only; no CANFD (hardware limitation)
- Without hardware (`--backend none`) the frame stream is only verified down to the WS/REST layer; vcan0 covers no-hardware integration
- The Tauri GUI is build-verified in CI but has not been run in a local graphical session (the 7 IPC commands map one to one to the Web front end, see the T27 alignment record)
- The `can-socketcan` crate has 7 pre-existing intra-doc link warnings in rustdoc (historical baseline, not introduced here)

## License

MIT — see [LICENSE](LICENSE).
