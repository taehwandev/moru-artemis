"""ADB endpoint contracts shared by the host MCP tools."""

import os
import unittest
from unittest.mock import Mock, patch

from tao_adb import adb_client


class AdbEndpointTest(unittest.TestCase):
    def test_legacy_server_variables_do_not_select_a_different_endpoint(self):
        server = Mock()
        with patch.dict(os.environ, {'ADB_SERVER_HOST': 'remote-host',
                                    'ADB_SERVER_PORT': '5040'}, clear=True):
            adb_client(server)
        server.AdbClient.assert_called_once_with(host='localhost', port=5037)

    def test_explicit_endpoint_matches_controller_settings(self):
        server = Mock()
        with patch.dict(os.environ, {'ADB_HOST': 'remote-host', 'ADB_PORT': '5040'},
                        clear=True):
            adb_client(server)
        server.AdbClient.assert_called_once_with(host='remote-host', port=5040)

    def test_malformed_port_uses_the_controller_default(self):
        for port in ('', 'invalid', '-1'):
            with self.subTest(port=port):
                server = Mock()
                with patch.dict(os.environ, {'ADB_PORT': port}, clear=True):
                    adb_client(server)
                server.AdbClient.assert_called_once_with(host='localhost', port=5037)
