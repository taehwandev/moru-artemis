# moru-artemis: host-driven Android MCP (Rust)

This repository is an independent Apache-2.0 fork of [Google's Artemis](https://github.com/google/artemis),
rewritten in Rust as a standalone host-driven stdio MCP server. It is not maintained,
endorsed, or sponsored by Google. The original Python fork is preserved on the `python`
branch (`python-archive` tag).

The host reasons about screenshots and UI elements; `moru-artemis` executes explicit
device operations via `adb`. No Gemini, OpenAI, or Anthropic API key is required.

## Build and connect

```sh
git clone https://github.com/taehwandev/moru-artemis.git
cd moru-artemis
cargo build --release
```

Configure an MCP server named `moru-artemis` in your host with:

- **command:** the absolute path to `target/release/moru-artemis`
- **args:** none (`[]`)
- **optional env:** `ADB_HOST` (default `localhost`), `ADB_PORT` (default `5037`), `ADB_PATH` (default `adb`)

Keep machine-specific paths and device serials out of committed configuration.

## Available tools

### Read-only observation tools
| Tool | Purpose |
| --- | --- |
| `devices` | List connected ADB device serials |
| `screenshot` | Capture current screen as a JPEG MCP image |
| `hierarchy` | Dump and filter UI hierarchy JSON (bounded to 4096 bytes when filtered) |
| `wait_for_element` | Poll for `present`, `absent`, `enabled`, `disabled`, `focused`, `checked`, `selected`, or `stable` |
| `capture_baseline` | Capture an in-memory reference screenshot + UI hierarchy |
| `compare_current` | Compute `pixel_delta` and `semantic_delta` against the baseline |

### Action & performance tools
| Tool | Purpose |
| --- | --- |
| `tap` | Tap screen pixel coordinates |
| `swipe` | Swipe between coordinates with `duration_ms` |
| `input_text` | Focus coordinates and type text |
| `press_key` | Send an Android keycode (`KEYCODE_BACK`, `KEYCODE_ENTER`, etc.) |
| `launch_app` | Launch an installed Android package |
| `tap_element` | Tap one exact UI selector match |
| `tap_element_and_wait` | Tap one exact match and wait for an expected UI state |
| `input_element` | Focus and type into one exact UI selector match |
| `scroll_to_element` | Bounded swipe loop until an exact selector appears |
| `perf_frame_stats` | Read or reset `dumpsys gfxinfo` frame statistics |
| `record_frames` | Record screen via `screenrecord` + `ffmpeg` and return a PNG contact sheet |
