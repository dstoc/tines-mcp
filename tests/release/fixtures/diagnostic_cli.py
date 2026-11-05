"""Subprocess fixture: exercise CLI main and HTTP parsing with no network/engine I/O."""

import json
import os
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tests/release"))
from test_release import r, environment, SOURCE_SHA, WORKFLOW_SHA, DIGEST, unavailable

base, failure = Path(sys.argv[1]), sys.argv[2]
marker = "SECRET_RESPONSE_MARKER\x1b[31m\r\n::error::unsafe"
record = base / "release/record.json"
commands, paths = [], []
variant = sys.argv[3] if len(sys.argv) > 3 else "predicate"
os.environ.update(environment(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                              RELEASE_VERSION="0.1.2", RELEASE_PLEASE_RESULT="success",
                              GH_TOKEN="TOKEN_" + marker, RELEASE_TAG="v0.1.2",
                              DOCKER_CONFIG=str(base / "docker"), GITHUB_OUTPUT=str(base / "outputs")))
release = r.Release(base, base, record)


def command(*args, **kwargs):
    commands.append(args)
    (base / "commands.json").write_text(json.dumps(commands))
    if args == ("gh", "--version"):
        return "gh version 2.103.3 (test)"
    if args == ("gh", "attestation", "verify", "--help"):
        return "--bundle-from-oci --cert-identity --source-ref --predicate-type --format"
    if args[:3] == ("docker", "image", "inspect"):
        return json.dumps([dict(Id="local-image", Os="linux", Architecture="amd64",
                                Config=dict(User="10001:10001", Labels=r.labels(release.state)))])
    if args[:2] == ("docker", "push"):
        saved = json.loads(record.read_text())
        assert saved["push_attempted"] and saved["stage"] == "push_attempted_outcome_uncertain"
        if failure == "push_failure":
            raise r.ReleaseError("Push failed; outcome uncertain")
        return f"0.1.2: digest: {DIGEST} size: 1234"
    if args[:2] in [("docker", "login"), ("docker", "build")] or args[0] == "bash":
        return ""
    raise AssertionError("Unexpected command in offline fixture")


class Connection:
    def __init__(self, host, timeout):
        assert host == "api.github.com" and timeout == 60

    def request(self, method, path, headers):
        assert method == "GET" and path == r.PACKAGE
        paths.append(path)
        (base / "paths.json").write_text(json.dumps(paths))
        if self.fails_now() and variant == "request_failure":
            raise OSError(marker)

    def getresponse(self):
        return self

    status = 404

    def getheader(self, name):
        assert name == "Link"
        return marker if self.fails_now() else None

    def fails_now(self):
        return len(paths) == {"initial_authority": 1, "prepush": 2, "postpush": 3,
                              "sink_failure": 1, "postpush_sink_failure": 3,
                              "persistent_sink_failure": 1, "postpush_persistent_sink_failure": 3, "push_failure": 99}[failure]

    def read(self, limit):
        assert limit == r.PACKAGE_BODY_LIMIT + 1
        if self.fails_now():
            if variant == "body_failure":
                raise r.http.client.IncompleteRead(marker.encode())
            if variant == "invalid":
                return marker.encode()
            if variant == "duplicate_field":
                key = json.dumps(marker)
                return ("{" + key + ":1," + key + ":2}").encode()
            if variant == "invalid_constant":
                return ("{" + json.dumps(marker) + ":NaN}").encode()
            if variant == "too_large":
                return marker.encode() + b"x" * (limit - len(marker.encode()))
        return json.dumps(dict(message=marker, documentation_url=marker, **{marker: marker})
                          if self.fails_now() else unavailable()[1]).encode()

    def close(self):
        pass


def release_request(method, path, body=None, host="api.github.com", binary=False):
    assert method == "GET" and host == "api.github.com" and not binary
    row = dict(id=41, tag_name="v0.1.2", target_commitish=SOURCE_SHA, prerelease=False,
               draft=True, immutable=False, published_at=None)
    if path == r.RELEASES + "?per_page=100&page=1":
        value = [row]
    elif path == r.RELEASES + "/41":
        value = row
    else:
        assert path == r.RELEASES + "/41/assets?per_page=100&page=1"
        value = []
    return 200, json.dumps(value).encode(), None, None


original_write = r.write_json
failed = False


def write(path, value):
    global failed
    if "sink_failure" in failure and (not failed or "persistent" in failure) and Path(path) == record and paths and (
            not failure.startswith("postpush") or len(paths) == 3):
        failed = True
        # Fail after a safe temporary file was written but before atomic replace.
        Path(path).with_suffix(".tmp").write_text(json.dumps(value))
        raise OSError(marker)
    original_write(path, value)


sys.argv = [str(r.__file__), "publish", "--workflow", str(base), "--source", str(base), "--record", str(record)]
# Exercise hypothetical admitted creation diagnostics downstream of production's
# disabled gate. No environment input can perform this replacement in production.
original_dispatch = r.dispatch
def admitted(env):
    return dict(original_dispatch(env), creation_eligible=True)

with patch.object(r, "dispatch", side_effect=admitted), patch.object(r, "run", side_effect=command), patch.object(r, "source_version"), \
        patch.object(r, "resolve"), patch.object(r, "remote_tag"), \
        patch.object(r.http.client, "HTTPSConnection", Connection), \
        patch.object(r.Releases, "request", side_effect=release_request), patch.object(r, "write_json", side_effect=write):
    release.resolve()
    sys.exit(r.main())
