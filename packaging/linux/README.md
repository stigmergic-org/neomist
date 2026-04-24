# Linux Packaging

## Outputs

- `dist/neomist_<version>_<arch>.deb`
- `dist/NeoMist-x86_64.AppImage`

Current Linux app ID / desktop ID: `eth.neomist.app`

## Build Debian Package

```bash
scripts/build-deb.sh
```

Notes:

- This is primary Linux package format for NeoMist.
- Package installs binary to `/usr/bin/neomist`, desktop entry, icons, and AppStream metadata in normal Debian/Ubuntu locations.
- NeoMist keeps privileged DNS / CA trust / port-443 setup in app-driven first-run flow. Package does not silently mutate host state during install.
- `.deb` runtime deps are handled two ways: `dpkg-shlibdeps` computes ELF shared-library deps, and script adds explicit host-tool/runtime deps NeoMist uses outside ELF linkage like `libayatana-appindicator3-1`, `pkexec`, `setcap`, `update-ca-certificates`, `xdg-open`, and `systemd-resolved`.
- Script currently builds host-native Debian arch. Cross-arch `.deb` on this arm64 host is feasible, but needs extra multiarch/cross-toolchain setup not yet wired into repo.
- Build host needs `dpkg-deb` and `dpkg-shlibdeps` available. On Ubuntu that usually means standard `dpkg` + `dpkg-dev` tooling.

## Build AppImage

```bash
scripts/build-appimage.sh
```

Notes:

- Script supports host-native `x86_64` and `aarch64` AppImage builds.
- Script downloads `linuxdeploy` tools into `dist/tools/` when missing.
- AppImage is secondary convenience build.
- AppImage dependency handling is partial by design: `linuxdeploy` bundles shared libraries for supported architectures and script explicitly bundles `libayatana-appindicator`, but host tools like `pkexec`, `setcap`, `systemctl`, `update-ca-certificates`, and `xdg-open` still come from target system.
- Cross-arch AppImage from this arm64 host is not wired yet. That path needs x86_64 sysroot/toolchain plus emulation or containerized packaging.
- NeoMist's current Linux setup still uses stable executable path for `setcap` and autostart. Raw AppImage runs usually execute from temporary read-only mount paths, so AppImage path still has integration caveats that `.deb` avoids.
