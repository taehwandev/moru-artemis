# moru-artemis

Host-driven Android MCP server written in Rust (Apache-2.0).

> **Legacy Python fork:** The original Python fork of `google/artemis` (including the autonomous agent, web UI, and `moru_mcp.py`) is preserved on the `python` branch (`python-archive` tag).

`moru-artemis` exposes Android device observation, UI element targeting, baseline/current comparison, and graphics performance tools over **stdio MCP** so your host AI client (Codex, Claude Code, Antigravity, etc.) drives the device directly without a separate model API key.

## Quick Start

### Prerequisites
- Rust toolchain (`cargo`, `rustc`)
- `adb` on `PATH` and an authorized Android device or emulator
- Optional: `ffmpeg` on `PATH` for `record_frames`

### Build

```sh
cargo build --release
```

The binary is built at `target/release/moru-artemis`.

### Register as an MCP Server

Configure your MCP host to run the compiled binary over `stdio`:

- **command:** `/abs/path/to/moru-artemis/target/release/moru-artemis`
- **args:** `[]`
- **optional env:** `ADB_HOST`, `ADB_PORT`, `ADB_PATH`

See [MORU-MCP.md](MORU-MCP.md) for the full tool reference and host contract.
