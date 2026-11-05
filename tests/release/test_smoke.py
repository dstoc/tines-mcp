"""Run the real shared shell script against deterministic engine/curl responses."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
STUB = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
cmd = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["MOCK_LOG"], "a") as log:
    log.write(json.dumps([cmd, *args]) + "\\n")
if cmd == "docker":
    if args[:2] == ["image", "inspect"]:
        print(os.environ.get("MOCK_UID", "10001:10001"))
    elif args[0] == "run":
        sys.exit(int(os.environ.get("MOCK_RUN_EXIT", "0")))
    elif args[0] == "port":
        print(os.environ.get("MOCK_PORT", "127.0.0.1:54321"))
    elif args[0] == "inspect":
        print(os.environ.get("MOCK_CONTAINER_EXIT", "0"))
    elif args[0] == "stop":
        sys.exit(int(os.environ.get("MOCK_STOP_EXIT", "0")))
elif cmd == "curl":
    print(os.environ.get("MOCK_STATUS", "204"), end="")
'''


class SmokeTests(unittest.TestCase):
    def invoke(self, **values):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            for name in ["docker", "curl", "sleep"]:
                executable = base / name
                executable.write_text(STUB)
                executable.chmod(0o755)
            log = base / "commands"
            env = dict(os.environ, PATH=f"{base}:{os.environ['PATH']}", MOCK_LOG=str(log), **values)
            result = subprocess.run(["bash", str(ROOT / "scripts/container-smoke.sh"), "tested-image-id"],
                                    env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            return result, calls

    def test_runtime_contract_loopback_and_cleanup(self):
        result, calls = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        start = next(c for c in calls if c[:2] == ["docker", "run"])
        self.assertIn("127.0.0.1::8080", start)
        self.assertIn("MCP_BIND_ADDR=0.0.0.0:8080", start)
        self.assertEqual(start[-1], "tested-image-id")
        self.assertFalse(any("TOKEN" in a for a in start))
        name = start[start.index("--name") + 1]
        self.assertIn(["docker", "stop", "--time", "60", name], calls)
        self.assertIn(["docker", "inspect", "--format", "{{.State.ExitCode}}", name], calls)
        self.assertEqual(calls[-1], ["docker", "rm", "--force", name])
        health = next(c for c in calls if c[0] == "curl")
        self.assertIn("http://127.0.0.1:54321/healthz", health)
        self.assertIn("--noproxy", health)

    def test_bad_uid_never_starts(self):
        result, calls = self.invoke(MOCK_UID="0:0")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[:2] == ["docker", "run"] for c in calls))

    def test_cleanup_on_all_runtime_failures(self):
        for overrides in [dict(MOCK_RUN_EXIT="1"), dict(MOCK_PORT="0.0.0.0:1234"),
                          dict(MOCK_STATUS="200"), dict(MOCK_STOP_EXIT="1"), dict(MOCK_CONTAINER_EXIT="137")]:
            with self.subTest(overrides=overrides):
                result, calls = self.invoke(**overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls[-1][:3], ["docker", "rm", "--force"])
