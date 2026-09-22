"""Element selection, action refusal and local wait acceptance contracts."""
import asyncio
from pathlib import Path
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tao_elements import ElementTools


class ElementToolsTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.element = {'text': 'Search', 'resource-id': 'app:id/search',
                        'bounds': '[10,20][110,60]', 'enabled': 'true'}
        # The adapter reads elements through the server's hierarchy-only path, so
        # that is the seam these contracts stub rather than a controller call.
        self.observe = AsyncMock(return_value=[self.element])
        self.server = SimpleNamespace(elements=self.observe,
                                      tap=AsyncMock(), _type_into=AsyncMock())
        self.tools = ElementTools(self.server)

    async def test_exact_target_uses_bounds_and_explicit_device(self):
        result = await self.tools.tap_element('device-b', {'text': 'Search'})
        self.observe.assert_awaited_once_with('device-b')
        self.server.tap.assert_awaited_once_with('device-b', 60, 40)
        self.assertEqual('action_sent', result['status'])

    async def test_duplicates_refuse_without_action(self):
        self.observe.return_value = [self.element, dict(self.element)]
        with self.assertRaisesRegex(ValueError, 'Ambiguous'):
            await self.tools.tap_element('device-b', {'text': 'Search'})
        self.server.tap.assert_not_awaited()

    async def test_selector_fields_are_combined_exactly(self):
        other = {**self.element, 'resource-id': 'other'}
        self.observe.return_value = [other, self.element]
        await self.tools.tap_element('device-b', {'text': 'Search', 'resource-id': 'app:id/search'})
        with self.assertRaisesRegex(ValueError, 'not found'):
            await self.tools.tap_element('device-b', {'text': 'Sea'})

    async def test_invalid_selectors_do_not_access_device(self):
        for selector in ({}, {'index': '0'}, {'text': ''}, {'text': ' '}):
            with self.assertRaises(ValueError):
                await self.tools.tap_element('device-b', selector)
        self.observe.assert_not_awaited()

    async def test_disabled_unknown_or_invalid_bounds_refuse(self):
        for update in ({'enabled': 'false'}, {'enabled': None}, {'bounds': ''},
                       {'bounds': '[1,1][1,2]'}, {'bounds': '[-1,0][30,40]'}):
            self.observe.return_value = [{**self.element, **update}]
            with self.assertRaises(ValueError):
                await self.tools.tap_element('device-b', {'text': 'Search'})
        self.server.tap.assert_not_awaited()

    async def test_input_forwards_options_without_echoing_text(self):
        result = await self.tools.input_element('device-b', {'text': 'Search'}, 'private input', True)
        self.server._type_into.assert_awaited_once_with(
            'device-b', 60, 40, 'private input', True, False)
        self.assertNotIn('private input', str(result))

    async def test_input_reads_the_screen_once_and_reuses_its_focus_state(self):
        self.observe.return_value = [{**self.element, 'focused': 'true'}]
        await self.tools.input_element('device-b', {'text': 'Search'}, 'private input')
        self.observe.assert_awaited_once_with('device-b')
        self.server._type_into.assert_awaited_once_with(
            'device-b', 60, 40, 'private input', False, True)

    async def test_action_failure_propagates(self):
        self.server.tap.side_effect = RuntimeError('device refused')
        with self.assertRaisesRegex(RuntimeError, 'device refused'):
            await self.tools.tap_element('device-b', {'text': 'Search'})

    async def test_wait_polls_locally_until_enabled(self):
        # Every hierarchy observation is a list, including transitional states.
        self.observe.side_effect = [[], [{**self.element, 'enabled': 'false'}], [self.element]]
        result = await self.tools.wait_for_element('device-b', {'text': 'Search'}, 'enabled', 1000, 100)
        self.assertTrue(result['matched'])
        self.assertEqual(3, result['observations'])
        self.assertEqual([(('device-b',), {})] * 3,
                         [(c.args, c.kwargs) for c in self.observe.await_args_list])
        self.server.tap.assert_not_awaited()

    async def test_timeout_is_not_success(self):
        self.observe.return_value = []
        result = await self.tools.wait_for_element('device-b', {'text': 'Search'}, timeout_ms=20)
        self.assertFalse(result['matched'])
        self.assertEqual('timeout', result['status'])

    async def test_absent_requires_successful_observation(self):
        self.observe.return_value = []
        self.assertTrue((await self.tools.wait_for_element('device-b', {'text': 'Search'}, 'absent'))['matched'])
        self.observe.side_effect = RuntimeError('disconnected')
        with self.assertRaisesRegex(RuntimeError, 'disconnected'):
            await self.tools.wait_for_element('device-b', {'text': 'Search'}, 'absent')

    async def test_wait_duplicates_are_not_success_or_absence(self):
        self.observe.return_value = [self.element, self.element]
        for state in ('present', 'absent', 'enabled'):
            with self.assertRaisesRegex(ValueError, 'Ambiguous'):
                await self.tools.wait_for_element('device-b', {'text': 'Search'}, state)

    async def test_unknown_boolean_is_not_disabled(self):
        self.observe.return_value = [{'text': 'Search'}]
        result = await self.tools.wait_for_element('device-b', {'text': 'Search'}, 'disabled', 20)
        self.assertFalse(result['matched'])

    async def test_boolean_states_accept_ui_strings_and_bools(self):
        for state in ('enabled', 'disabled', 'focused', 'checked', 'selected'):
            key = 'enabled' if state == 'disabled' else state
            for value in ((False, 'false') if state == 'disabled' else (True, 'true')):
                self.observe.return_value = [{**self.element, key: value}]
                self.assertTrue((await self.tools.wait_for_element('device-b', {'text': 'Search'}, state))['matched'])

    async def test_invalid_wait_settings_fail_before_device_access(self):
        for kwargs in ({'state': 'anything'}, {'timeout_ms': 0}, {'timeout_ms': 30001}, {'poll_interval_ms': 1}):
            with self.assertRaises(ValueError):
                await self.tools.wait_for_element('device-b', {'text': 'Search'}, **kwargs)
        self.observe.assert_not_awaited()

    async def test_slow_async_observation_respects_deadline(self):
        async def slow(_device_serial):
            await asyncio.sleep(10)
            return [self.element]
        self.observe.side_effect = slow
        async with asyncio.timeout(1):
            result = await self.tools.wait_for_element('device-b', {'text': 'Search'}, timeout_ms=20)
        self.assertFalse(result['matched'])
        self.assertEqual(0, result['observations'])


if __name__ == '__main__':
    unittest.main()
