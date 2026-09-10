"""Device-only imports must not initialize the optional AI SDK."""
import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


class LazyPublicExportsTest(unittest.TestCase):
    def run_isolated(self, code):
        env = {k: v for k, v in os.environ.items() if not k.endswith("API_KEY")}
        result = subprocess.run(
            [sys.executable, "-I", "-B", "-c", "import sys; sys.path.insert(0, " + repr(str(ROOT)) + "); " + code],
            env=env, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(0, result.returncode, result.stderr)

    def test_import_does_not_load_sdk_or_model_dependencies(self):
        self.run_isolated(
            "import artemis; "
            "assert not any(n.startswith(('artemis.sdk', 'artemis.interfaces', 'langchain', 'google.genai', 'anthropic', 'openai', 'pydantic')) for n in sys.modules); "
            "assert 'Agent' in dir(artemis)"
        )

    def test_public_export_is_loaded_once_on_access(self):
        self.run_isolated(
            "from types import ModuleType; import artemis; "
            "m = ModuleType('artemis.sdk.agent'); m.Agent = object(); "
            "sys.modules[m.__name__] = m; "
            "assert artemis.Agent is m.Agent; del sys.modules[m.__name__]; "
            "assert artemis.Agent is m.Agent"
        )

    def test_unknown_export_keeps_attribute_error(self):
        self.run_isolated("import artemis; assert not hasattr(artemis, 'missing_public_export')")


if __name__ == "__main__":
    unittest.main()
