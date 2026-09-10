# tao-artemis: host-driven Android MCP

This is an independent fork of [Google's Artemis](https://github.com/google/artemis),
not maintained, endorsed, or sponsored by Google. It retains the original
Apache-2.0 source and copyright notices. Fork additions are distributed under
the same [license](LICENSE). Google's trademarks are not licensed by Apache-2.0.
`tao_mcp.py` adds a small stdio MCP adapter for existing Codex or Claude sessions.
The host reasons about screenshots/UI elements; Artemis executes explicit device
operations. No Gemini, OpenAI or Anthropic API key is required for this adapter.
The host's normal authentication, subscription, usage charges, and data policies
still apply. No separate model call is made by this adapter; screen content can
still be sent to the host model. Upstream benchmark results in the original
README are not measurements of this adapter.

## Install and connect

Install Python 3.12, `uv`, and ADB. Clone this fork and use its existing lockfile.
Device operations require an authorized Android device or running emulator;
install the app under test through your normal build/install process:

```sh
git clone https://github.com/taehwandev/tao-artemis.git
cd tao-artemis
uv sync --frozen --no-dev --python python3.12
```

Configure an MCP server named `tao-artemis` in each host with:

- command: the absolute path to this checkout's `.venv/bin/python`
- args: `-B` and the absolute path to this checkout's `tao_mcp.py`
- optional env: `ARTEMIS_APP_DIR` for private local runtime state

On Windows, use `.venv/Scripts/python.exe`. The lockfile installs the upstream
dependency set; this adapter is not a minimal dependency distribution.

Keep machine-specific paths and device serials out of committed configuration.
Restart/reconnect the host after registration. Standard stdio MCP works with
both Codex and Claude; it does not start either host CLI or a second model agent.
Do not register the upstream autonomous `mcp_server.server` for this mode.

## Available tools

| Tool | Purpose |
| --- | --- |
| `devices` | List connected device serials |
| `screenshot` | Return a native MCP image |
| `hierarchy` | Read UI elements without a separate OCR/model call |
| `tap` | Tap observed pixel coordinates |
| `swipe` | Swipe between coordinates with an explicit duration |
| `input_text` | Focus a field and enter or replace text |
| `press_key` | Send an Android keycode |
| `launch_app` | Launch an installed app; does not guarantee a cold start |

## Use only when device evidence is needed

1. Call `devices` and select the intended authorized Android device/emulator.
2. Pass its exact `device_serial` to every observation or action. Never assume
   the first device is the right target when several are connected.
3. Use `screenshot` (native MCP image) and `hierarchy` to inspect the current UI.
4. Use `tap`, `swipe`, `input_text`, `press_key`, or `launch_app` within the user's
   requested scope. Read the screen again to verify the result.

Registration alone does not route tests through MCP. The host workflow must
select and call the tools. Tao's `device-testing` guidance supports preparation
when the operator has configured the local device MCP registry. Keep unit tests
in the project's normal test runner.

The adapter forces local mode and does not start an awake-service thread, OCR
provider, autonomous task runner, or cloud device service. ADB must already be
available and the device authorized; upstream UIAutomator may start its on-device
helper during the first observation. With no device, startup/tool discovery still
works, while device operations require a connected target. Firebase Test Lab
provisioning and cloud-only testing are not supplied by this adapter.
Screenshots and UI text may contain private data and are returned to the host
model. Use only devices and content permitted for that session.

## Verification

Run the adapter tests and existing lazy-export regression tests:

```sh
.venv/bin/python -B -m unittest discover -s tests/unit -p 'test_tao_mcp.py'
.venv/bin/python -B -m unittest discover -s tests/unit -p 'test_lazy_public_exports.py'
```

Tool discovery proves MCP readiness, not successful UI interaction. Report device
observation/action checks separately, and do not claim an end-to-end app test
from installation alone.

## Upstream compatibility

Keep the lazy public exports in `artemis/__init__.py`: Python executes a package's
initializer even when importing a specific submodule. Direct submodule imports
alone cannot isolate this adapter from eager autonomous-agent initialization.
The lazy-export regression tests protect the existing SDK exports.

Text focus uses the exported `ensure_focus_at_coords` actuator helper. Controller
creation still uses upstream's private `_get_controller`, which has no equivalent
public factory in the pinned source. Startup checks its synchronous, explicit-serial
call signature without opening a device and fails with a compatibility error on
drift. This catches interface changes, not every behavioral change: run device
observation/action checks before accepting an upstream update.

A trace directory can be created during initialization; it is not evidence that
a device tool ran. Likewise, configured host registration does not prove that a
live chat loaded the server. Confirm tool discovery in the active client and an
actual observation before reporting device verification. Animation timing needs
a sequence or recording; a single screenshot cannot establish motion behavior.
