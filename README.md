# MatterLite

A native Rust desktop client for [Mattermost](https://mattermost.com) servers, for
Windows x64, Linux x64 and macOS arm64. The UI is built on WarpUI, the GPU-rendered UI
framework from the [Warp](https://github.com/warpdotdev/warp) terminal.

Status: **M1 (protocol)** in progress. Nothing usable yet.

## Building

Requires the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by rustup).

```sh
cargo run -p matterlite
cargo run -p warpui --example list   # WarpUI smoke test
cargo run -p mm_cli -- https://chat.example.com bootstrap --login alice   # protocol harness
```

Platform prerequisites:

- **Windows**: Visual Studio Build Tools (MSVC, Windows SDK).
- **macOS**: Xcode (Metal shader compiler via `xcrun metal`).
- **Linux**: `libx11-dev libxkbcommon-dev libwayland-dev libfontconfig-dev libdbus-1-dev`
  (fontconfig is loaded at runtime with dlopen) and a Vulkan or GL driver.

## License

MatterLite is licensed under the GNU Affero General Public License v3.0 only (`LICENSE`).
Code derived from Warp's `warpui` and `warpui_core` crates remains under the MIT license
(`LICENSE-MIT`). `PROVENANCE.md` lists every piece of code derived from Warp.
