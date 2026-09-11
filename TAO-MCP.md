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

Use the same Python environment setup as upstream: install `uv`, clone the
repository, and run `uv sync`. The project declares Python >=3.12 in
`pyproject.toml`; uv selects or downloads a compatible interpreter, so a separate
Python installation or a machine-specific `python3.12` executable is unnecessary.
ADB and an authorized Android device or running emulator are needed for device
operations. Install the app under test through your normal build/install process:

```sh
git clone https://github.com/taehwandev/tao-artemis.git
cd tao-artemis
uv sync
```

Upstream [Quick Start](README.md#quick-start) uses `./start.sh` on macOS/Linux
or `.\start.bat` on Windows. Those scripts also prepare system tools, launch the
web UI, and offer to register the upstream autonomous MCP and global rules.
For this host-driven adapter, use the shared `uv sync` environment step above
and register `tao_mcp.py` below; running the web UI bootstrap is optional and
does not configure this adapter. Keep the original upstream startup scripts
unchanged. Use `uv sync --frozen` when reproducing an already-reviewed lockfile.

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
| `tap_element` | Tap one exact UI match without host coordinate calculation |
| `input_element` | Focus and type into one exact UI match |
| `wait_for_element` | Poll locally for a bounded UI condition and return a compact result |
| `devices` | List connected device serials |
| `screenshot` | Return a native MCP image |
| `hierarchy` | Read UI elements without a separate OCR/model call, as the adapter's element view |
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

The adapter observes the accessibility helper but never provisions it, so it
defaults `ARTEMIS_HIERARCHY_BACKEND` to `uiautomator`. Under upstream's `auto`
the fallback client retries the helper every 30 seconds, and on a device with no
helper installed each retry stops and restarts the UIAutomator2 server: measured
here as one 11.6 s observation every 30 s against 0.33 s for the others. Set the
variable yourself to keep another backend, including `auto` once the helper is
installed through upstream's own provisioning path.

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

## Element actions and local waits

Prefer `tap_element` and `input_element` for accessible elements. Pass an explicit
`device_serial` and a `selector` object with exact `text`, `content-desc`,
`resource-id`, `class`, or `package` values. Multiple fields are ANDed. Empty or
unknown selectors, multiple matches, disabled/unknown enabled state, and invalid
bounds refuse actions. No first-match fallback or fuzzy model selection occurs.
The coordinates come from a fresh hierarchy; UI movement between observation and
action is still possible. Observe the outcome rather than assuming success.

Example tool arguments (fixture values only):

```json
{"device_serial":"SELECTED_SERIAL","selector":{"text":"Search"}}
```

For `input_element`, also supply `text` and optionally `clear_existing`.
Action results use `status: action_sent`; they are not acceptance assertions.
Input results do not echo the supplied text. Existing pixel tools stay available.

`hierarchy` and every element result report the same element view: the five
selector fields, `bounds`, and `enabled` as a JSON boolean whenever upstream
states it, plus `checkable`, `checked`, `clickable`, `focused`, `long-clickable`,
`scrollable` and `selected` only where they are true. `enabled` stays explicit
even when false because that is what explains a refused action; a flag upstream
leaves unknown is omitted. Upstream duplicates dropped from the view — `parsed_bounds`,
`accessibilityText` — and per-node metadata such as `index`, `drawing-order`,
`display-id` and `hint` are not part of any selector or action contract; the two
screens measured here fell from 36.1 KB to 10.2 KB and 17.8 KB to 6.7 KB. Read
the raw upstream hierarchy through the SDK when a dropped field is needed.

`wait_for_element` supports `present`, `absent`, `enabled`, `disabled`, `focused`,
`checked`, and `selected`. Timeout is 1–30000 ms (default 5000), polling interval
100–2000 ms (default 250). Polling runs inside one tool call without additional
LLM turns; only the final matching-element summary, observation count, elapsed
time and `matched` flag are returned. `matched: false` / `status: timeout` is a
failed condition, never a passing test. Device failures and ambiguous matches
are errors, not proof of absence. Timeouts use cooperative async cancellation;
a blocking upstream device call can delay return. This is not a motion recorder.

Reconnect the host MCP client after updating to discover all 11 tools. Verify
new selection/wait contracts with:

```sh
.venv/bin/python -B -m unittest discover -s tests/unit -p 'test_tao_elements.py'
```

One element action replaces the usual host hierarchy-read/coordinate-calculation/
action round trip; one wait can perform several local observations. Actual speed
and model-token savings depend on the device and scenario and are not benchmarked
by these contract tests. No screenshot-diff or animation-analysis feature is added.
