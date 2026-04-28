# Arch Packaging

## Outputs

- `dist/neomist-<version>-linux-x86_64.pkg.tar.zst`

## Build Package

Build from the release source configured in `PKGBUILD`:

```bash
scripts/build-arch.sh
```

Build from the current checkout, including uncommitted non-ignored files:

```bash
scripts/build-arch.sh --local
```

The local mode is intended for CI and pre-release testing because it does not require the matching
GitHub `v<version>` tag to exist yet.

## Notes

- Package installs the binary to `/usr/bin/neomist` plus the existing Linux desktop entry,
  AppStream metadata, and icons.
- The PKGBUILD disables makepkg LTO and forces system OpenSSL/zstd linkage. Several Rust
  dependencies build C/ASM objects, and Arch's global LTO flags can otherwise surface as missing
  `OPENSSL_*`, `ZSTD_*`, or `ring_core_*` symbols at the final Rust link step.
- Package install is intentionally passive. NeoMist still performs privileged DNS, certificate
  trust, and port-443 setup through its first-run flow.
- Runtime integration expects `systemd-resolved`, `pkexec`, `setcap`, `update-ca-trust`, and
  `xdg-open` from the target Arch system.
- Only `x86_64` is packaged in v1.
