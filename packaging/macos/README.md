# macOS Packaging

`neomist` already embeds UI during Rust build, so macOS packaging only needs to wrap existing binary in `.app` bundle.

## Build

Run on macOS:

```bash
scripts/build-macos-pkg.sh
```

Signing is opt-in. Build scripts only sign when `--sign` is passed.

Standalone signed app bundle:

```bash
NEOMIST_APP_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" scripts/build-macos-app.sh --sign
```

Signed installer package:

```bash
NEOMIST_APP_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
NEOMIST_INSTALLER_SIGN_IDENTITY="Developer ID Installer: Your Name (TEAMID)" \
scripts/build-macos-pkg.sh --sign
```

Notarize signed package:

```bash
NEOMIST_NOTARY_PROFILE="neomist-notary" scripts/notarize-macos-pkg.sh
```

`scripts/build-macos-pkg.sh` builds temporary staged `.app` internally for installer payload. Keep `build-macos-app.sh` if you want standalone `.app` output for local testing.

All macOS build/notarization scripts load project `.env` automatically when present. Use `NEOMIST_ENV_FILE=/path/to/file` to override.

Outputs:

- `dist/neomist-<version>-macos-<arch>.pkg` for Installer.app distribution
- `dist/neomist-<version>-macos-<arch>.app` only when you run `scripts/build-macos-app.sh`

Bundle defaults:

- app name: `NeoMist`
- executable: `neomist`
- bundle id: `eth.neomist.app`
- `LSUIElement=true` so app behaves like menu bar app instead of regular Dock app

## Installer Package

`.pkg` can be distributed directly. `.dmg` not needed for current install flow.

Current `pkg` flow installs:

- `NeoMist.app` into `/Applications`
- moves any existing `/Applications/NeoMist.app` aside before install so upgrades can cross bundle ID changes
- NeoMist certificate files and trust for current console user during package `postinstall`
- DNS resolver files during package `postinstall`
- `/usr/local/bin/neomist` symlink during package `postinstall`

Current `pkg` flow assumes interactive install with logged-in console user. If package is installed headlessly or with no console user, cert preparation may be skipped and first app launch can still prompt for cert trust.

Installer.app pages now include:

- Welcome summary of what will be installed
- Conclusion page telling user to restart browser before changes take effect

Optional signing:

- app signing uses `NEOMIST_APP_SIGN_IDENTITY` with `Developer ID Application`
- pkg signing uses `NEOMIST_INSTALLER_SIGN_IDENTITY` with `Developer ID Installer`
- signing happens only when `--sign` is passed to build script
- if pkg signing is enabled, app must already be signed or `NEOMIST_APP_SIGN_IDENTITY` must be set too
- optional entitlements file path: `NEOMIST_APP_ENTITLEMENTS`
- default app signing enables hardened runtime and timestamp
- without signing identities, scripts build unsigned artifacts

Useful checks:

```bash
security find-identity -v -p basic
codesign --verify --strict --verbose=2 dist/neomist-<version>-macos-<arch>.app
pkgutil --check-signature dist/neomist-<version>-macos-<arch>.pkg
```

## Notarization

Recommended path:

1. sign app with `Developer ID Application`
2. sign pkg with `Developer ID Installer`
3. notarize signed pkg with `xcrun notarytool`
4. staple pkg ticket with `xcrun stapler`

Store credentials once in macOS keychain:

```bash
xcrun notarytool store-credentials "neomist-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "app-specific-password"
```

Then notarize built pkg:

```bash
NEOMIST_NOTARY_PROFILE="neomist-notary" scripts/notarize-macos-pkg.sh
```

Optional:

- use `NEOMIST_NOTARY_KEYCHAIN` if profile stored in non-default keychain
- use `NEOMIST_NOTARY_TIMEOUT` to change submit wait timeout

Validation after notarization:

```bash
xcrun stapler validate dist/neomist-<version>-macos-<arch>.pkg
spctl -a -vv -t install dist/neomist-<version>-macos-<arch>.pkg
```
