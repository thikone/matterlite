# MatterLite

A native Rust desktop client for [Mattermost](https://mattermost.com) servers, for
Windows x64, Linux x64 and macOS arm64. The UI is built on WarpUI, the GPU-rendered UI
framework from the [Warp](https://github.com/warpdotdev/warp) terminal.

Status: **M3, the first usable version**: login (with MFA) and stored accounts; chat tabs,
the Chats sidebar, the Ctrl/Cmd+K switcher; the channel view with Markdown, emoji and code
highlighting, older history as you scroll, a composer with drafts and the typing indicator;
the Channel tab (info, members, pinned) and the History pane with local search; Warp's
themes and the five Mattermost themes, following the account's theme; a reconnect banner.
Everything is cached locally, so it opens at once and works offline for reading. Threads,
reactions, files and notifications come next (`docs/MatterLite.md` §18).

## Running

Download the archive for your platform from the releases (or a CI run's artifacts), unpack
it and start `matterlite` (`matterlite.exe` on Windows). The `data` folder next to it keeps
the settings, the cache and the logs, so the folder can live on a USB stick; delete `data` to
use your system's per-user folders instead. The builds aren't signed: Windows SmartScreen
asks once ("More info" › "Run anyway"), and on macOS remove the quarantine flag with
`xattr -d com.apple.quarantine matterlite`.

## Building

Requires the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by rustup).

```sh
cargo run -p matterlite
cargo run -p warpui --example list   # WarpUI smoke test
cargo run -p mm_cli -- https://chat.example.com bootstrap --login alice   # protocol harness
```

Portable archives (the same as the releases), into `dist/`:

```sh
script/build-portable.sh [v0.3.0]          # Linux x64, macOS arm64
script\build-portable.ps1 [-Tag v0.3.0]    # Windows x64 (PowerShell)
```

Without a tag the version is `v<crate version>-dev.<commit>`. Pushing a `v*` tag makes CI
build all three archives and publish them as a GitHub release.

Platform prerequisites:

- **Windows**: Visual Studio Build Tools (MSVC, Windows SDK).
- **macOS**: Xcode (Metal shader compiler via `xcrun metal`).
- **Linux**: `libx11-dev libxkbcommon-dev libwayland-dev libfontconfig-dev libdbus-1-dev`
  (fontconfig is loaded at runtime with dlopen) and a Vulkan or GL driver.

## License

MatterLite is licensed under the GNU Affero General Public License v3.0 only (`LICENSE`).
Code derived from Warp's `warpui` and `warpui_core` crates remains under the MIT license
(`LICENSE-MIT`). `PROVENANCE.md` lists every piece of code derived from Warp. The emoji images (Google's Noto Emoji)
and the emoji and emoticon data from the Mattermost webapp are under the Apache License 2.0
(`LICENSE-APACHE`); see `PROVENANCE.md`, "Other third-party material".
