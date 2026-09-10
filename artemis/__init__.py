# Copyright 2026 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""☕ ARTEMIS: Autonomous Multimodal Android Agent & Testing Framework."""

from artemis._version import __version__

# Load optional SDK/agent dependencies only when their public exports are used.
# Importing a device MCP submodule must not bootstrap the autonomous agent.
from importlib import import_module

_EXPORT_MODULES = {
    'ArtemisClient': 'artemis.interfaces.sdk.client',
    'ConcurrencyMode': 'artemis.interfaces.sdk.client',
    'StreamEvent': 'artemis.interfaces.sdk.task',
    'StreamEventType': 'artemis.interfaces.sdk.task',
    'Task': 'artemis.interfaces.sdk.task',
    'Capabilities': 'artemis_client',
    'Device': 'artemis_client',
    'TaskHandle': 'artemis_client',
    'TaskResult': 'artemis_client',
    'Agent': 'artemis.sdk.agent',
    'Builders': 'artemis.sdk.builders',
    'BaseDeviceDriver': 'artemis.drivers.base',
}

__all__ = [
    "__version__",
    "ArtemisClient",
    "Capabilities",
    "ConcurrencyMode",
    "Device",
    "StreamEvent",
    "StreamEventType",
    "Agent",
    "Builders",
    "Task",
    "TaskHandle",
    "TaskResult",
    "BaseDeviceDriver",
]


def __getattr__(name: str):
    module = _EXPORT_MODULES.get(name)
    if module is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    value = getattr(import_module(module), name)
    globals()[name] = value
    return value


def __dir__():
    return sorted(set(globals()) | set(__all__))
