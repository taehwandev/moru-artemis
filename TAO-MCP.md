# tao-artemis: host-driven Android MCP

This fork retains Google's Apache-2.0 Artemis source and copyright notices.
`tao_mcp.py` adds a small stdio MCP adapter for existing Codex or Claude sessions.
The host reasons about screenshots/UI elements; Artemis executes explicit device
operations. No Gemini, OpenAI or Anthropic API key is required for this adapter.
The host's normal subscription or usage charges still apply.

## Install and connect

Use Python 3.12 and the existing lockfile:

```sh
uv sync --frozen --no-dev --python python3.12
```

Configure an MCP server named `tao-artemis` in each host with:

- command: the absolute path to this checkout's `.venv/bin/python`
- args: `-B` and the absolute path to this checkout's `tao_mcp.py`
- optional env: `ARTEMIS_APP_DIR` for private local runtime state

Keep machine-specific paths and device serials out of committed configuration.
Restart/reconnect the host after registration. Standard stdio MCP works with
both Codex and Claude; it does not start either host CLI or a second model agent.
Do not register the upstream autonomous `mcp_server.server` for this mode.

## Use only when device evidence is needed

1. Call `devices` and select the intended authorized Android device/emulator.
2. Pass its exact `device_serial` to every observation or action. Never assume
   the first device is the right target when several are connected.
3. Use `screenshot` (native MCP image) and `hierarchy` to inspect the current UI.
4. Use `tap`, `swipe`, `input_text`, `press_key`, or `launch_app` within the user's
   requested scope. Read the screen again to verify the result.

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
