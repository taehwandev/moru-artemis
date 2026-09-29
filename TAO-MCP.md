# tao-artemis: host-driven Android MCP

This is an independent fork of [Google's Artemis](https://github.com/google/artemis),
not maintained, endorsed, or sponsored by Google. It retains the original
Apache-2.0 source and copyright notices. Fork additions are distributed under
the same [license](LICENSE). Google's trademarks are not licensed by Apache-2.0.
Artemis derives parts of its device layer from
[Mobile-Use](https://github.com/minitap-ai/mobile-use) by Minitap, Inc., whose
NOTICE this fork carries in [NOTICE](NOTICE); the element parser this adapter
reads the hierarchy through is theirs. Minitap is not affiliated with this fork.
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
| `tap_element_and_wait` | Tap one exact UI match, then check a bounded UI condition |
| `input_element` | Focus and type into one exact UI match |
| `wait_for_element` | Poll locally for a bounded UI condition and return a compact result |
| `capture_baseline` | Capture one same-device static reference in memory and return its image |
| `compare_current` | Return the current image plus pixel and compact hierarchy deltas from that reference |
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

For a regression or improvement claim, define the expected oracle first. Call
`capture_baseline` on the known-good or pre-change build before replacing it, run
the same bounded scenario on the candidate using the same device and display
state, then call `compare_current`. Both calls include the observed screen image;
the comparison also reports thresholded pixel change and compact added/removed UI
elements. It deliberately returns no pass verdict: the host must evaluate the
delta against explicit acceptance criteria. A current-state invariant can be
verified without an older build, but must not be reported as a before/after claim.

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

`hierarchy`, `tap_element`, `tap_element_and_wait`, `input_element` and
`wait_for_element` read the UI
dump on its own. Upstream's combined screen read returns a screenshot alongside
the hierarchy and the screenshot dominates its cost, so an element observation
that discarded it paid for a capture nobody used: measured here as 1774 ms
falling to 206 ms on the same screen, with the returned element view identical to
`controller.get_ui_elements()`. `screenshot` is the mirror image: it captures
through the screen client instead, skipping the hierarchy dump and the fixed
settling pause the combined read adds. That pause protects an agent chaining
actions in-process; these tools are separated by a host round trip that is far
longer. A failed capture now raises instead of returning an empty image, and the
result is always JPEG, where the combined read's own fallback could emit PNG
under a `jpeg` label. On a Galaxy S22 the tool went from 1142 ms to 134 ms across
that change and the capture-order fix below.

Upstream captured through `adb screencap` and treated UIAutomator2 as the
fallback, on the grounds that screencap is more reliable for layers such as
popups. That did not survive measurement, so the order is now reversed here.
Raising the notification shade changed 96.017% of the UIAutomator2 frame against
95.886% of the screencap frame, with a 0.000% floor between two consecutive
frames, so both see a system overlay; the adapter's own `screenshot` shows the
same 95.804%. UIAutomator2 costs 107 ms against 480 ms on the same device, and it
is what upstream Mobile-Use uses. `FLAG_SECURE` surfaces were not compared, so
that case remains unmeasured. screencap still covers a UIAutomator2 failure, and
a multi-display device's warning line no longer discards that capture.

`capture_baseline`/`compare_current` are the only callers that want both halves,
and they now take each from this adapter's own screenshot and element reads, in
the order upstream's combined read takes them, so the two halves stay as close
together as that read makes them. What they no longer pay is its fixed 300 ms
pre-capture pause: on a Galaxy S22+ a baseline went from 764 ms to 374 ms, the
element view is the one the combined read produces on the same screen (76 of 76
elements, identical), and two consecutive captures of a static screen still
report a 0.000% pixel floor, so the pause was not what kept that floor clean.
Nothing in the adapter calls the combined read any more.

Every element read calls the same `dump_hierarchy(compressed=True)` and the same
upstream parser and filter the combined read uses, so the element list is the
one that read produces. What differs is where the filter's screen size comes
from: the combined read refreshed it from each screenshot, while this path takes
the size recorded when the controller was created. After a rotation the recorded
size is stale until the controller is recreated, which affects only which
offscreen elements the filter prunes; `compare_current` refuses a capture whose
own image dimensions changed. Element bounds come from the dump either way, so
`tap_element` and `input_element` still compute the same coordinates.

Text input reads the screen once. Upstream's focus helper decides whether the
field already holds focus, and so skips a redundant tap, from its own combined
read; `input_element` had already observed that same field to compute its
coordinates, so the decision now comes from that observation and the second read
is gone. On a Galaxy S22+ the focus decision cost 758 ms through the helper and
285 ms through this adapter's element read, and `input_element` no longer pays it
twice. A failed observation refuses the action instead of tapping blindly, and
an unfocused field is still tapped and given upstream's same one-second wait for
the keyboard.

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

Text focus uses the exported `find_element_at_coords` actuator hit test, with the
tap and the keyboard wait in the adapter, in place of the exported
`ensure_focus_at_coords` helper that reads the screen a second time to make the
same decision. Controller creation still uses upstream's private
`_get_controller`, which has no equivalent public factory in the pinned source.
Reading elements without a screenshot uses two more upstream helpers:
`parse_hierarchy_xml_to_elements` from
`third_party.mobile_use.clients.ui_automator_client` and
`filter_ui_hierarchy` from `artemis.utils.ui_filter`, neither of which the
package exports. Startup checks all three call signatures without opening a
device and fails with a compatibility error on drift. This catches interface
changes, not every behavioral change: run device observation/action checks before
accepting an upstream update, and compare the element view against
`controller.get_ui_elements()` when upstream's parser or filter moves.

A trace directory can be created during initialization; it is not evidence that
a device tool ran. Likewise, configured host registration does not prove that a
live chat loaded the server. Confirm tool discovery in the active client and an
actual observation before reporting device verification. Animation timing needs
a sequence or recording; a single screenshot cannot establish motion behavior.

`capture_baseline` keeps one observation in process memory, overwriting the prior
one; the comparison helper itself does not write that screen content to disk.
`compare_current` refuses a missing baseline, another device serial, or changed
screen dimensions. It is for stable static states. Use matched recordings or
bounded frame sequences for motion and keep the baseline/candidate trigger and
interval the same.

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

`tap_element_and_wait` takes the tap `selector` plus an `expected_selector` and
optional `expected_state`, `timeout_ms`, and `poll_interval_ms` using the same
rules as `wait_for_element`. It validates the expectation before tapping, then
returns separate `action` and `verification` objects. Only
`verification.matched: true` establishes that the expected state was observed;
`action.status: action_sent` alone does not. Choose a condition that was not
already true before the tap when verifying a transition.

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

Reconnect the host MCP client after updating to discover all 14 tools. Verify
new selection/wait contracts with:

```sh
.venv/bin/python -B -m unittest discover -s tests/unit -p 'test_tao_elements.py'
.venv/bin/python -B -m unittest discover -s tests/unit -p 'test_tao_comparison.py'
```

One element action replaces the usual host hierarchy-read/coordinate-calculation/
action round trip; one wait can perform several local observations. Actual speed
and model-token savings depend on the device and scenario and are not benchmarked
by these contract tests. No screenshot-diff or animation-analysis feature is added.
