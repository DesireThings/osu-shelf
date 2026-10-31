# Arch Linux packaging

Builds `osu-shelf` (terminal app) and `osu-shelf-gui` (desktop app) from the
tagged source, and installs the desktop entry and hicolor icons.

```bash
makepkg -si
```

Or build without installing, then install the package yourself:

```bash
makepkg -s
sudo pacman -U osu-shelf-*.pkg.tar.zst
```

`makepkg -s` pulls the build dependencies: Rust, CMake, a C++ compiler
(`gcc`), Node.js and pnpm.

## Notes

- `system-openssl.patch` makes realm-core link the distribution's OpenSSL
  instead of downloading its own prebuilt copy at configure time, so the
  build needs no network beyond crates.io and npm.
- `options=('!lto')` is required. With makepkg's `lto` option enabled
  (as on CachyOS), `-flto` reaches the C/C++ compilers and the GCC bitcode
  objects in realm-core's and aws-lc's static archives cannot be linked by
  rustc's LLVM linker; every symbol comes back undefined.
- Expect roughly 10–30 minutes and about 6 GB of free space: realm-core and
  the Tauri app are both compiled from source.
- After editing `PKGBUILD`, regenerate the metadata with
  `makepkg --printsrcinfo > .SRCINFO` so the files stay ready for an AUR
  submission when registrations reopen.
- makepkg's source cache (`SRCDEST`) defaults to this directory. When
  `pkgver` changes, the previous release's `THIRD-PARTY-LICENSES-desktop.txt`
  is still here and is reused as if it had been downloaded for the new
  version, so it fails the checksum. Run `makepkg --clean` (or delete that
  file) before building a new version.
