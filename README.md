<p align="center">
  <img src="assets/logo.png" alt="NeoMist logo" width="220">
</p>

# NeoMist

NeoMist is a local-first desktop app for browsing `.eth` and `.wei` sites without relying on centralized gateways. It runs a tray app, an embedded web dashboard, a loopback-only HTTPS server, local DNS for Ethereum names, a Helios light client, and an IPFS node or proxy.

## What NeoMist Does

- Opens `.eth` and `.wei` dapps from a local dashboard at `https://neomist.localhost`
- Resolves onchain contenthash records locally and proxies them through IPFS
- Serves `neomist.localhost`, `ipfs.localhost`, and `*.ipfs.localhost` over locally trusted TLS
- Caches site versions, shows how much content is stored locally, and can follow later updates
- Exposes consensus checkpoints so users can compare what chain state they are seeing
- Lets you configure fallback consensus and execution RPC endpoints from the UI
- Uses an existing local Kubo instance when available, or downloads and runs a managed one

## Architecture

- Rust desktop app and tray UI built with `tao`, `tray-icon`, and `axum`
- Helios for Ethereum light-client sync
- `alloy` for ENS and `.wei` contenthash resolution
- Kubo for local IPFS access
- React + Vite + Tailwind + DaisyUI frontend embedded into the Rust binary at build time

## Local Endpoints

- `https://neomist.localhost` for the dashboard, launcher, settings, and seeding UI
- `https://neomist.localhost/rpc` for loopback-only JSON-RPC access backed by the embedded Helios client
- `https://ipfs.localhost/webui` for the local IPFS WebUI
- `https://<cid>.ipfs.localhost` for the local IPFS gateway
- `https://<name>.eth` and `https://<name>.wei` once NeoMist installs local DNS integration

## Requirements

- macOS or Linux
- Rust toolchain with Cargo
- Node.js and npm for the embedded UI build
- A desktop session capable of running a tray app
- On Ubuntu/Kubuntu for builds: `pkg-config`, `libglib2.0-dev`, `libgtk-3-dev`, and `libayatana-appindicator3-dev`
- On Linux: `systemd-resolved`, `pkexec`, `update-ca-certificates`, and `libcap2-bin`

## Running Locally

```bash
cargo run
```

Notes:

- The Rust build automatically rebuilds the UI from `ui/` when needed.
- On first launch, NeoMist may prompt for administrator or root access to install local certificate trust and DNS handling.
- On Linux, NeoMist may restart itself once after first launch so it can bind local HTTPS on port `443`.
- After startup, open `https://neomist.localhost` or use the tray menu's `Dashboard` item.

To build an optimized binary:

```bash
cargo build --release
```

## Frontend Development

For UI-only iteration:

```bash
cd ui
npm install
npm run dev
```

The production app does not serve the Vite dev server directly. NeoMist embeds the built files from `ui/dist` into the Rust binary.

## CLI

```bash
neomist --help
```

Useful commands:

- `neomist` starts the app
- `sudo neomist system install --yes` installs system integration on macOS
- `sudo neomist system uninstall --yes` removes DNS resolver configuration and NeoMist certificates on macOS or Linux

## Configuration

NeoMist stores its user configuration at `~/.config/neomist/config.json` and runtime data under `~/.local/share/neomist`.

The settings UI currently manages:

- consensus RPC fallback order
- execution RPC fallback order
- background follow interval in minutes
- start NeoMist automatically at login

Default endpoints in the current codebase are:

- consensus: `https://ethereum.operationsolarstorm.org`
- execution: `https://eth.drpc.org`

## macOS Packaging

Build a standalone app bundle:

```bash
scripts/build-macos-app.sh
```

Build an installer package:

```bash
scripts/build-macos-pkg.sh
```

Current macOS outputs:

- `dist/neomist-<version>-macos-<arch>.app`
- `dist/neomist-<version>-macos-<arch>.pkg`

If you launch the packaged `.app` directly, place it in `/Applications` before first run so NeoMist can complete its system setup.

More packaging details live in `packaging/macos/README.md`.

## Linux Packaging

Build a Debian package:

```bash
scripts/build-deb.sh
```

Build an AppImage:

```bash
scripts/build-appimage.sh
```

Trigger Linux CI release builds from Cargo version:

1. Open GitHub `Actions`
2. Run `Tag And Build`
3. Workflow reads `Cargo.toml`, creates `v<version>` tag, and fails if that tag already exists
4. Tag push triggers `Linux Packages`

Current Linux outputs:

- `dist/neomist-<version>-linux-x86_64.deb`
- `dist/neomist-<version>-linux-arm64.deb`
- `dist/neomist-<version>-linux-x86_64.AppImage`
- `dist/neomist-<version>-linux-arm64.AppImage`

Linux packaging note:

- `.deb` is primary Linux package because it installs NeoMist into stable native system paths and fits current DNS / CA trust / privileged-port model.
- `.deb` manages runtime deps with `dpkg-shlibdeps` plus explicit host-tool deps for NeoMist's tray, DNS, cert, and integration flow.
- AppImage is secondary convenience artifact. It remains useful for direct download, but current Linux setup still assumes stable writable executable path for `setcap` and autostart.
- AppImage bundles shared libraries, but still relies on host system for integration tools like `pkexec`, `setcap`, `systemctl`, `update-ca-certificates`, and `xdg-open`.
- Current repo supports host-native Linux builds cleanly: `.deb` and AppImage on arm64 here, same scripts on x86_64 hosts. Full 4-target local matrix from this arm64 machine still needs extra cross/emulation setup.

Current packaging ID defaults:

- Linux desktop ID: `eth.neomist.app`
- macOS bundle ID: `eth.neomist.app`

More packaging details live in `packaging/linux/README.md`.

## Project Layout

- `src/` Rust app, DNS, TLS, ENS, IPFS, tray, and HTTP server code
- `ui/` React dashboard source
- `assets/` icons and logo files
- `scripts/` packaging helpers
- `packaging/` macOS app and installer templates
