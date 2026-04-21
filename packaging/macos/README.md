# macOS Packaging

`neomist` already embeds UI during Rust build, so macOS packaging only needs to wrap existing binary in `.app` bundle.

## Build

Run on macOS:

```bash
scripts/build-macos-pkg.sh
```

`scripts/build-macos-pkg.sh` calls `scripts/build-macos-app.sh` internally. Keep `build-macos-app.sh` only if you want standalone `.app` output for local testing.

Outputs:

- `dist/NeoMist.app`
- `dist/NeoMist-<version>-<arch>.pkg` for Installer.app distribution

Bundle defaults:

- app name: `NeoMist`
- executable: `neomist`
- bundle id: `org.neomist.app`
- `LSUIElement=true` so app behaves like menu bar app instead of regular Dock app

## Installer Package

`.pkg` can be distributed directly. `.dmg` not needed for current install flow.

Current `pkg` flow installs:

- `NeoMist.app` into `/Applications`
- NeoMist certificate files and trust for current console user during package `postinstall`
- DNS resolver files during package `postinstall`
- `/usr/local/bin/neomist` symlink during package `postinstall`

Current `pkg` flow assumes interactive install with logged-in console user. If package is installed headlessly or with no console user, cert preparation may be skipped and first app launch can still prompt for cert trust.

Installer.app pages now include:

- Welcome summary of what will be installed
- Conclusion page telling user to restart browser before changes take effect

Optional signing:

- set `NEOMIST_INSTALLER_SIGN_IDENTITY` before running `scripts/build-macos-pkg.sh`
- without signing identity, script builds unsigned `.pkg`
