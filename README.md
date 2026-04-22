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
- On Linux: `systemd-resolved`, `pkexec`, and `update-ca-certificates`

## Running Locally

```bash
cargo run
```

Notes:

- The Rust build automatically rebuilds the UI from `ui/` when needed.
- On first launch, NeoMist may prompt for administrator or root access to install local certificate trust and DNS handling.
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

- `dist/NeoMist.app`
- `dist/NeoMist-<version>-<arch>.pkg`

If you launch the packaged `.app` directly, place it in `/Applications` before first run so NeoMist can complete its system setup.

More packaging details live in `packaging/macos/README.md`.

## Project Layout

- `src/` Rust app, DNS, TLS, ENS, IPFS, tray, and HTTP server code
- `ui/` React dashboard source
- `assets/` icons and logo files
- `scripts/` packaging helpers
- `packaging/` macOS app and installer templates
