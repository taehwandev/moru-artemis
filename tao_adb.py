"""Explicit ADB device selection shared by the host MCP tools."""

import os


def adb_client(adb_server):
    """Use the same ADB endpoint as upstream's controller factory."""
    port = os.environ.get('ADB_PORT', '5037')
    return adb_server.AdbClient(
        host=os.environ.get('ADB_HOST', 'localhost'),
        port=int(port) if port.isdigit() else 5037,
    )


def selected_adb_device(adb_server, device_serial):
    """Resolve only an explicitly selected connected device, without opening its UI."""
    if not isinstance(device_serial, str) or not device_serial.strip():
        raise ValueError('device_serial must be explicitly selected from devices')
    for device in adb_client(adb_server).device_list():
        if device.serial == device_serial.strip():
            return device
    raise ValueError('Selected device is not connected')
