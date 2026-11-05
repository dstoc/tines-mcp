"""Release failures must preserve the publication boundary. All registry I/O is mocked."""

import copy
import importlib.util
import json
import contextlib
import io
import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
SOURCE_SHA, WORKFLOW_SHA, OLD_WORKFLOW = "a" * 40, "b" * 40, "c" * 40
DIGEST, OTHER = "sha256:" + "1" * 64, "sha256:" + "2" * 64


def environment(**overrides):
    result = dict(GITHUB_REPOSITORY=r.REPO, GITHUB_EVENT_NAME="workflow_dispatch",
                  GITHUB_REF="refs/heads/main", GITHUB_SHA=WORKFLOW_SHA,
                  RELEASE_TAG="v0.1.1", RELEASE_SOURCE=SOURCE_SHA, GITHUB_ACTOR="dstoc", GITHUB_ACTOR_ID="539597",
                  GITHUB_TRIGGERING_ACTOR="dstoc", GITHUB_RUN_ATTEMPT="1", GITHUB_RUN_ID="123",
                  GH_TOKEN="test-only-token", RELEASE_ID="41", RELEASE_DRAFT="true")
    result.update(overrides)
    return result


def package(identifier=17):
    return dict(id=identifier, name="tines-mcp", package_type="container",
                owner=dict(login="dstoc", id=539597), repository=dict(full_name=r.REPO))


def version(identifier=1, digest=DIGEST, tags=None):
    return dict(id=identifier, name=digest, metadata=dict(container=dict(tags=tags or [])))


def unavailable():
    # An absent package and a hidden package can return this identical response.
    return 404, dict(message="Not Found", documentation_url="https://docs.github.com/rest/packages/packages"), None


def visible(rows=None, identifier=17):
    return [(200, package(identifier), None), (200, rows or [], None)]


def link(page=2, **overrides):
    values = dict(origin="https://api.github.com", route=r.VERSIONS,
                  query=f"state=active&per_page=100&page={page}", relation="next")
    values.update(overrides)
    return '<{origin}{route}?{query}>; rel="{relation}"'.format(**values)


def verified(state, predicate_type=r.PREDICATE, signer=WORKFLOW_SHA):
    # Shape from gh 2.103.3 / sigstore-go 1.3.0, not the unverified bundle object.
    return [dict(verificationResult=dict(signature=dict(certificate=dict(
        subjectAlternativeName=r.WORKFLOW, issuer="https://token.actions.githubusercontent.com",
        sourceRepositoryURI=r.SOURCE, sourceRepositoryRef="refs/heads/main",
        buildSignerDigest=signer, sourceRepositoryDigest=signer)), statement=dict(
            subject=[dict(name=r.IMAGE, digest=dict(sha256=state["digest"][7:]))],
            predicateType=predicate_type,
            predicate=r.identity(state, signer) if predicate_type == r.PREDICATE else dict(buildDefinition={}))))]


class DispatchTests(unittest.TestCase):
    def test_manual_dispatch_requires_positive_access(self):
        state = r.dispatch(environment())
        self.assertEqual(state["version"], "0.1.1")
        self.assertFalse(state["creation_eligible"])

    def test_dispatch_gates(self):
        for field, value in [("GITHUB_REPOSITORY", "other/tines-mcp"), ("GITHUB_REF", "refs/tags/v0.1.1"),
                             ("GITHUB_EVENT_NAME", "pull_request"), ("RELEASE_SOURCE", "main"),
                             ("RELEASE_SOURCE", "a" * 39), ("GITHUB_SHA", "main"),
                             ("RELEASE_TAG", "v01.1.0"), ("RELEASE_TAG", "v1.2.3-rc.1"),
                             ("RELEASE_TAG", "v1.2.3\n"), ("RELEASE_TAG", "$(touch unexpected)"),
                             ("RELEASE_MODE", "auto"), ("BOOTSTRAP_CONFIRMATION", "anything")]:
            with self.subTest(field=field, value=value), self.assertRaises(r.ReleaseError):
                r.dispatch(environment(**{field: value}))

    def test_obsolete_mode_and_confirmation_are_rejected(self):
        for values in [dict(RELEASE_MODE="normal"), dict(RELEASE_MODE="bootstrap"),
                       dict(BOOTSTRAP_CONFIRMATION="old owner confirmation")]:
            with self.subTest(values=values), self.assertRaises(r.ReleaseError):
                r.dispatch(environment(**values))


class AutomaticDispatchTests(unittest.TestCase):
    def automatic(self, **overrides):
        return environment(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                           RELEASE_VERSION="0.1.1", RELEASE_PLEASE_RESULT="success", **overrides)

    def test_creation_is_derived_only_for_first_011_handoff(self):
        self.assertTrue(r.dispatch(self.automatic())["creation_eligible"])
        for values in [dict(GITHUB_RUN_ATTEMPT="2"), dict(GITHUB_RUN_ATTEMPT=""),
                       dict(GITHUB_RUN_ATTEMPT="01"), dict(RELEASE_TAG="v0.1.0", RELEASE_VERSION="0.1.0"),
                       dict(RELEASE_TAG="v0.1.2", RELEASE_VERSION="0.1.2")]:
            with self.subTest(values=values):
                self.assertFalse(r.dispatch(dict(self.automatic(), **values))["creation_eligible"])


    def test_handoff_failures(self):
        base = self.automatic()
        for field, values in {
            "RELEASE_CREATED": ["", "false", "TRUE", "True"],
            "RELEASE_PLEASE_RESULT": ["", "failure", "cancelled", "skipped"],
            "RELEASE_VERSION": ["", "0.2.0", "0.1.1-rc.1", "0.1.1\n"],
            "RELEASE_SOURCE": ["", "a" * 39, "A" * 40, "main"],
            "RELEASE_TAG": ["", "v0.1.1-rc.1", "v01.1.0"],
            "GITHUB_REPOSITORY": ["other/repo"],
            "GITHUB_REF": ["refs/heads/other", "refs/tags/v0.1.1"],
            "GITHUB_EVENT_NAME": ["release", "pull_request"],
            "RELEASE_MODE": ["bootstrap"],
            "BOOTSTRAP_CONFIRMATION": ["obsolete confirmation"],
        }.items():
            for value in values:
                with self.subTest(field=field, value=value), self.assertRaises(r.ReleaseError):
                    r.dispatch(dict(base, **{field: value}))
        # Even the confirmed owner cannot bootstrap from a push.
        with self.assertRaises(r.ReleaseError):
            r.dispatch(dict(base, RELEASE_MODE="bootstrap",
                            BOOTSTRAP_CONFIRMATION="obsolete confirmation"))


class RegistryTests(unittest.TestCase):
    def lookup(self, replies, allow_unavailable=False):
        with patch.object(r.Registry, "get", side_effect=replies):
            return r.Registry("test").lookup("0.1.1", allow_unavailable)

    def test_positive_access_and_absent_tag(self):
        self.assertEqual(self.lookup(visible()), (17, None))
        self.assertEqual(self.lookup(visible([version(tags=["0.1.1"])])), (17, DIGEST))

    def test_http_errors_never_mean_absence(self):
        for status in [301, 401, 403, 404, 429, 500, 503]:
            with self.subTest(status=status), self.assertRaises(r.ReleaseError):
                self.lookup([(status, {}, None)])
            with self.subTest(version_status=status), self.assertRaises(r.ReleaseError):
                self.lookup([(200, package(), None), (status, {}, None)])

    def test_creation_allowance_requires_structured_exact_route_404(self):
        with patch.object(r.Registry, "get", return_value=unavailable()) as get:
            self.assertEqual(r.Registry("test").lookup("0.1.1", True), (None, None))
            get.assert_called_once_with(r.PACKAGE)
        for response in [(404, {}, None), (404, unavailable()[1], link()), (404, unavailable()[1], ""), (404, "Not Found", None),
                         (404, dict(message="Not Found", documentation_url="https://evil.example/"), None),
                         *[(s, {}, None) for s in [401, 403, 429, 500, 503]]]:
            with self.subTest(response=response), self.assertRaises(r.ReleaseError):
                self.lookup([response], True)

    def test_creation_allowance_keeps_positive_access_enumeration(self):
        self.assertEqual(self.lookup(visible(), True), (17, None))
        self.assertEqual(self.lookup(visible([version(tags=["0.1.1"])]), True), (17, DIGEST))
        for status in [301, 401, 403, 404, 429, 500, 503]:
            with self.subTest(status=status), self.assertRaises(r.ReleaseError):
                self.lookup([(200, package(), None), (status, unavailable()[1], None)], True)

    def test_authentication_and_transport_fail_closed(self):
        with self.assertRaises(r.ReleaseError):
            r.Registry("")
        for fail in [OSError("offline"), r.http.client.HTTPException("offline")]:
            with patch.object(r.http.client.HTTPSConnection, "request", side_effect=fail), self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.1")

    def test_fixed_api_origin_no_redirect_and_malformed_json(self):
        with patch.object(r.http.client, "HTTPSConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.read.return_value = 302, b'{}'
            with self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.1")
            connection.assert_called_once_with("api.github.com", timeout=60)
            self.assertEqual(connection.return_value.request.call_args.args[:2], ("GET", r.PACKAGE))
            self.assertEqual(connection.return_value.request.call_args.kwargs["headers"]["Authorization"], "Bearer test")
            response.read.return_value = b'not json'
            with self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.1", True)

    def test_oversized_and_corrupt_404_cannot_authorize_creation(self):
        for body in [b'x' * (8 * 1024 * 1024 + 1), b'{"message":"Not Found","message":"Not Found"}',
                     b'{"message":NaN}', b'not json']:
            with self.subTest(size=len(body)), patch.object(r.http.client, "HTTPSConnection") as connection:
                response = connection.return_value.getresponse.return_value
                response.status, response.read.return_value = 404, body
                response.getheader.return_value = None
                with self.assertRaises(r.ReleaseError):
                    r.Registry("test").lookup("0.1.1", True)
                connection.return_value.close.assert_called_once()

    def test_wrong_package_identity(self):
        for field, value in [("id", True), ("id", 0), ("name", "other"), ("package_type", "npm"),
                             ("owner", {"login": "other", "id": 539597}),
                             ("owner", {"login": "dstoc", "id": 1}), ("repository", {})]:
            data = package()
            data[field] = value
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                self.lookup([(200, data, None)])

    def test_find_version_beyond_page_one(self):
        rows = [version(i + 1, "sha256:" + f"{i:064x}") for i in range(100)]
        replies = [(200, package(), None), (200, rows, link()),
                   (200, [version(101, DIGEST, ["0.1.1"])], link(page=1, relation="prev"))]
        self.assertEqual(self.lookup(replies), (17, DIGEST))
        for fail in [(503, {}, None), (200, {}, None)]:
            with self.subTest(fail=fail), self.assertRaises(r.ReleaseError):
                self.lookup(replies[:2] + [fail])

    def test_malformed_duplicate_or_conflicting_versions(self):
        for rows in [{}, [dict(id=1)], [version(tags=["0.1.1", "0.1.1"])],
                     [version(), version()], [version(), version(2)],
                     [version(tags=["0.1.1"]), version(2, OTHER, ["0.1.1"])],
                     [version(digest="not-digest")], [version(identifier=True)], [version(tags=[1])]]:
            with self.subTest(rows=rows), self.assertRaises(r.ReleaseError):
                self.lookup([(200, package(), None), (200, rows, None)])

    def test_structural_corruption_stops_lookup(self):
        for data in [None, [], "package", dict(package(), owner=None), dict(package(), repository=[]),
                     dict(package(), owner="dstoc")]:
            with self.subTest(data=data), self.assertRaises(r.ReleaseError):
                self.lookup([(200, data, None)])
        for row in [None, "version", dict(version(), metadata=None),
                    dict(version(), metadata=dict(container=[]))]:
            with self.subTest(row=row), self.assertRaises(r.ReleaseError):
                self.lookup(visible([row]))
        for text in ['{"id":1,"id":2}', '{"value":NaN}', '']:
            with self.subTest(text=text), self.assertRaises(r.ReleaseError):
                r.read_json(text)

    def test_pagination_origin_route_shape_and_completeness(self):
        for header in ["", "invalid", link(origin="http://api.github.com"), link(origin="https://evil.example"),
                       link(origin="https://api.github.com:443"), link(route=r.PACKAGE),
                       link(page=1), link(page=3), link(query="page=2"),
                       link(query="state=active&per_page=100&page=2&page=3"),
                       link() + ", " + link(), link(relation="last"), link(relation="unknown")]:
            with self.subTest(header=header), self.assertRaises(r.ReleaseError):
                r.next_page(header, 1)
        with self.assertRaises(r.ReleaseError):
            self.lookup([(200, package(), None), (200, [], link())])


class IdentityTests(unittest.TestCase):
    def setUp(self):
        self.state = dict(r.dispatch(environment()), digest=DIGEST)

    def test_verified_source_and_original_execution_sha(self):
        for signer in (WORKFLOW_SHA, OLD_WORKFLOW):
            self.assertEqual(r.verified_identity(verified(self.state, signer=signer), self.state, r.PREDICATE), signer)

    def test_unverified_bundle_and_provenance_cannot_authorize_source(self):
        for data in [[], {}, [dict(attestation=verified(self.state)[0])], verified(self.state, r.PROVENANCE)]:
            with self.subTest(data=data), self.assertRaises(r.ReleaseError):
                r.verified_identity(data, self.state, r.PREDICATE)

    def test_malformed_verified_results_are_rejected(self):
        for data in [[None], ["bundle"], [dict(verificationResult=None)],
                     [dict(verificationResult=dict(signature="certificate"))]]:
            with self.subTest(data=data), self.assertRaises(r.ReleaseError):
                r.verified_identity(data, self.state, r.PREDICATE)

    def test_every_signed_field_and_certificate_binding(self):
        paths = [("statement", "predicate", k) for k in r.identity(self.state, WORKFLOW_SHA)]
        paths += [("signature", "certificate", k) for k in verified(self.state)[0]["verificationResult"]["signature"]["certificate"]]
        paths += [("statement", "predicateType"), ("statement", "subject")]
        for path in paths:
            data = verified(self.state)
            target = data[0]["verificationResult"]
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = "wrong"
            with self.subTest(path=path), self.assertRaises(r.ReleaseError):
                r.verified_identity(data, self.state, r.PREDICATE)
        data = verified(self.state)
        data[0]["verificationResult"]["statement"]["predicate"]["workflow_sha"] = OLD_WORKFLOW
        with self.assertRaises(r.ReleaseError):
            r.verified_identity(data, self.state, r.PREDICATE)


class OrchestrationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.workflow, self.source = self.base / "workflow", self.base / "source"
        self.workflow.mkdir()
        self.source.mkdir()
        (self.source / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "tines-mcp"\nversion = "0.1.1"\n')
        self.env = environment(GITHUB_OUTPUT=str(self.base / "outputs"), DOCKER_CONFIG=str(self.base / "docker"))
        self.patch_env = patch.dict(os.environ, self.env, clear=True)
        self.patch_env.start()
        self.addCleanup(self.patch_env.stop)
        self.commands, self.fail_command = [], None
        self.paths = []
        self.push_output = f"0.1.1: digest: {DIGEST} size: 1234"
        self.local_image_id = "local-immutable-image-id"
        self.manifest_version = "0.1.1"
        self.head = SOURCE_SHA
        self.remote = SOURCE_SHA
        self.remote_tag_name = "v0.1.1"
        self.user = "10001:10001"
        self.label_overrides = {}
        self.replies = visible() + visible() + visible([version(tags=["0.1.1"])])
        self.signer = WORKFLOW_SHA
        self.patch_run = patch.object(r, "run", side_effect=self.command)
        self.patch_run.start()
        self.addCleanup(self.patch_run.stop)
        self.patch_http = patch.object(r.Registry, "get", side_effect=self.http)
        self.patch_http.start()
        self.addCleanup(self.patch_http.stop)
        self.patch_releases = patch.object(r, "Releases")
        self.api = self.patch_releases.start().return_value
        self.addCleanup(self.patch_releases.stop)
        self.api.listing.return_value = [dict(id=41, tag_name="v0.1.1", target_commitish=SOURCE_SHA,
                                            prerelease=False, draft=True, immutable=False, published_at=None)]
        self.api.release.return_value = self.api.listing.return_value[0]
        self.api.evidence.return_value = None
        self.release = r.Release(self.workflow, self.source, self.base / "release/record.json")
        self.release.resolve()

    def http(self, path):
        self.paths.append(path)
        response = self.replies.pop(0)
        if isinstance(response, Exception):
            raise response
        return response

    def command(self, *args, cwd=None, env=None, input=None):
        self.commands.append((args, cwd, env, input))
        if self.fail_command and self.fail_command(args):
            raise r.ReleaseError("Injected command failure")
        if args[:3] == ("git", "rev-parse", "HEAD"):
            return WORKFLOW_SHA if Path(cwd) == self.workflow else self.head
        if args[:2] == ("git", "rev-parse"):
            return SOURCE_SHA
        if args[:2] == ("git", "ls-remote"):
            return f"{self.remote}\trefs/tags/{self.remote_tag_name}"
        if args[:2] == ("cargo", "metadata"):
            return json.dumps(dict(packages=[dict(name="tines-mcp", version=self.manifest_version, source=None)]))
        if args[:3] == ("docker", "image", "inspect"):
            return json.dumps([dict(Id=self.local_image_id, Os="linux", Architecture="amd64",
                                   Config=dict(User=self.user, Labels=dict(r.labels(self.release.state), **self.label_overrides)))])
        if args[:2] == ("docker", "--config"):
            self.assertEqual(json.loads((Path(args[2]) / "config.json").read_text()),
                             {"auths": {"ghcr.io": {}}})
        if args[:2] == ("docker", "push"):
            # The record MUST exist before the real write command is dispatched.
            record = json.loads(self.release.record.read_text())
            self.assertTrue(record["push_attempted"])
            self.assertEqual(record["stage"], "push_attempted_outcome_uncertain")
            return self.push_output
        if args == ("gh", "--version"):
            return "gh version 2.103.3 (test)"
        if args == ("gh", "attestation", "verify", "--help"):
            return "--bundle-from-oci --cert-identity --source-ref --predicate-type --format"
        if args[:3] == ("gh", "attestation", "verify"):
            self.assertIn("--bundle-from-oci", args)
            self.assertIn("--cert-identity", args)
            predicate = args[args.index("--predicate-type") + 1]
            return json.dumps(verified(self.release.state, predicate, self.signer))
        return ""

    def fresh_record(self):
        record = self.release.record.parent / "next" / "record.json"
        self.release = r.Release(self.workflow, self.source, record)
        self.release.resolve()

    def pushes(self):
        return [c for c, *_ in self.commands if c[:2] == ("docker", "push")]

    def first_handoff(self):
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                          RELEASE_VERSION="0.1.1", RELEASE_PLEASE_RESULT="success")
        self.release.resolve()
        self.replies = [unavailable(), unavailable()] + visible([version(tags=["0.1.1"])])

    def test_publish_once_then_verify_and_anonymous_runtime(self):
        self.release.publish()
        self.assertEqual(len(self.pushes()), 1)
        self.assertEqual(self.release.state["stage"], "digest_observed")
        self.assertEqual(json.loads((self.release.record.parent / "release-identity.json").read_text()),
                         r.identity(self.release.state, WORKFLOW_SHA))
        self.replies += visible([version(tags=["0.1.1"])])
        self.release.accept()
        self.assertEqual(self.release.state["validation"], "passed")
        calls = [args for args, *_ in self.commands]
        self.assertEqual(sum(c[:2] == ("docker", "build") for c in calls), 1)
        self.assertEqual(sum(c[0] == "bash" for c in calls), 2)
        anonymous = next(c for c in calls if c[:2] == ("docker", "--config"))
        self.assertIn(f"{r.IMAGE}@{DIGEST}", anonymous)
        self.assertFalse(Path(anonymous[2]).exists())

    def test_automatic_normal_publication_then_manual_recovery(self):
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                          RELEASE_VERSION="0.1.1", RELEASE_PLEASE_RESULT="success")
        self.release.resolve()
        self.release.publish()
        self.replies += visible([version(tags=["0.1.1"])])
        self.release.accept()
        self.assertEqual(self.release.state["validation"], "passed")
        os.environ.update(GITHUB_EVENT_NAME="workflow_dispatch", RELEASE_CREATED="",
                          RELEASE_VERSION="", RELEASE_PLEASE_RESULT="skipped")
        self.fresh_record()
        self.replies = visible([version(tags=["0.1.1"])]) * 2
        self.release.publish()
        self.release.accept()
        self.assertTrue(self.release.state["reused"])
        self.assertEqual(len(self.pushes()), 1)
        self.assertFalse((self.release.record.parent / "release-identity.json").exists())

    def test_automatic_rerun_unavailable_package_cannot_create(self):
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                          RELEASE_VERSION="0.1.1", RELEASE_PLEASE_RESULT="success", GITHUB_RUN_ATTEMPT="2")
        self.release.resolve()
        self.replies = [unavailable()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])
        self.assertFalse(self.release.state["creation_eligible"])

    def test_automatic_invalid_handoff_zero_registry_or_engine_commands(self):
        self.commands.clear()
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                          RELEASE_VERSION="0.1.1", RELEASE_PLEASE_RESULT="failure")
        before = copy.deepcopy(self.replies)
        with self.assertRaises(r.ReleaseError):
            self.release.resolve()
        self.assertEqual(self.commands, [])
        self.assertEqual(self.replies, before)

    def test_first_publication_acceptance_then_positive_access_recovery(self):
        self.first_handoff()
        self.release.publish()
        self.replies += visible([version(tags=["0.1.1"])])
        self.release.accept()
        self.assertEqual(self.release.state["validation"], "passed")
        self.assertTrue(self.release.state["creation_eligible"])
        self.assertEqual(self.release.state["package_observation"], "unavailable")
        self.assertEqual(self.release.state["prepush_package_observation"], "unavailable")
        self.assertEqual(self.paths, [r.PACKAGE, r.PACKAGE, r.PACKAGE,
                                     r.VERSIONS + "?state=active&per_page=100", r.PACKAGE,
                                     r.VERSIONS + "?state=active&per_page=100"])
        self.assertIn("new_image=true", (self.base / "outputs").read_text())
        self.assertEqual(sum(c[:2] == ("docker", "build") for c, *_ in self.commands), 1)
        self.assertEqual(self.release.state["package_id"], 17)
        self.assertEqual(len(self.pushes()), 1)
        # A fresh manual dispatch reuses the digest, without building or writing it.
        self.commands.clear()
        os.environ.update(GITHUB_EVENT_NAME="workflow_dispatch")
        self.release = r.Release(self.workflow, self.source, self.base / "recovery/record.json")
        self.release.resolve()
        self.replies = visible([version(tags=["0.1.1"])]) * 2
        self.release.publish()
        self.signer = OLD_WORKFLOW
        self.release.accept()
        self.assertTrue(self.release.state["reused"])
        self.assertEqual(self.release.state["original_workflow_sha"], OLD_WORKFLOW)
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[:2] == ("docker", "build") for c, *_ in self.commands))
        self.assertIn("new_image=false", (self.base / "outputs").read_text())
        self.assertFalse((self.release.record.parent / "release-identity.json").exists())

    def test_source_resolution_failures_zero_writes(self):
        for prefix in [("git", "rev-parse", "--verify"), ("git", "merge-base"), ("git", "ls-remote")]:
            self.fail_command = lambda c: c[:len(prefix)] == prefix
            with self.subTest(prefix=prefix), self.assertRaises(r.ReleaseError):
                self.release.resolve()
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[0] == "docker" for c, *_ in self.commands))

    def test_tag_moves_after_local_smoke_zero_pushes(self):
        def change_tag(command):
            if command[0] == "bash":
                self.remote = "d" * 40
            return False
        self.fail_command = change_tag
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_source_version_and_source_sha_mismatch_zero_pushes(self):
        for field, value in [("manifest_version", "0.2.0"), ("head", "d" * 40), ("remote", "e" * 40)]:
            previous = getattr(self, field)
            setattr(self, field, value)
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                self.release.publish()
            setattr(self, field, previous)
        (self.source / "Cargo.lock").write_text('[[package]]\nname="tines-mcp"\nversion="0.2.0"\n')
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_obsolete_bootstrap_is_rejected_before_any_registry_work(self):
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION="obsolete confirmation",
                          GITHUB_ACTOR_ID="123")
        with self.assertRaises(r.ReleaseError):
            self.release.resolve()
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[0] == "docker" for c, *_ in self.commands))

    def test_same_record_cannot_dispatch_twice(self):
        self.release.publish()
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)

    def test_unsupported_gh_stops_before_publication(self):
        self.fail_command = lambda c: c == ("gh", "attestation", "verify", "--help")
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertFalse(any(c[0] == "docker" for c, *_ in self.commands))
        self.assertEqual(self.pushes(), [])

    def test_login_failure_zero_reads_zero_pushes(self):
        self.fail_command = lambda c: c[:2] == ("docker", "login")
        original = copy.deepcopy(self.replies)
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.replies, original)
        self.assertEqual(self.pushes(), [])

    def test_normal_lookup_failures_zero_pushes(self):
        for replies in [[unavailable()], [(403, {}, None)], [r.ReleaseError("transport")],
                        [(200, package(), None), (503, {}, None)]]:
            self.replies = replies
            with self.subTest(replies=replies), self.assertRaises(r.ReleaseError):
                self.release.publish()
            self.assertEqual(self.pushes(), [])

    def test_first_publication_preflight_errors_zero_pushes(self):
        self.first_handoff()
        for reply in [(404, {}, None), (404, unavailable()[1], link()), (301, {}, None), (401, {}, None), (403, {}, None),
                      (429, {}, None), (503, {}, None), r.ReleaseError("transport")]:
            self.replies = [reply]
            with self.subTest(reply=reply), self.assertRaises(r.ReleaseError):
                self.release.publish()
            self.assertEqual(self.pushes(), [])

    def test_package_or_tag_appearing_before_push_zero_writes(self):
        for second in [visible([version(tags=["0.1.1"])]), visible(identifier=18)]:
            self.replies = visible() + second
            with self.assertRaises(r.ReleaseError):
                self.release.publish()
        self.first_handoff()
        self.replies = [unavailable()] + visible()
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_local_runtime_and_label_failures_zero_pushes(self):
        self.fail_command = lambda c: c[0] == "bash"
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.fail_command = None
        self.replies = visible()
        self.user = "root"
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_push_ambiguity_is_durable_and_never_retried(self):
        self.fail_command = lambda c: c[:2] == ("docker", "push")
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)
        self.assertEqual(self.release.state["stage"], "push_attempted_outcome_uncertain")
        os.environ["RELEASE_JOB_STATUS"] = "failure"
        with contextlib.redirect_stdout(io.StringIO()):
            self.release.report()
        self.assertIn("Positive-access investigation only", self.release.state["next_action"])

    def test_post_push_metadata_error_or_digest_conflict_no_second_push(self):
        for post in [[(403, {}, None)], visible([version(tags=["0.1.1"], digest=OTHER)]),
                     visible([version(tags=["0.1.1"])], identifier=18)]:
            self.commands.clear()
            self.fresh_record()
            self.replies = visible() + visible() + post
            with self.subTest(post=post), self.assertRaises(r.ReleaseError):
                self.release.publish()
            self.assertEqual(len(self.pushes()), 1)
            self.assertTrue(self.release.state["push_attempted"])
            self.assertEqual(self.release.state["stage"], "digest_observed")

    def test_attestation_action_failure_reports_partial_publication(self):
        self.release.publish()
        # Workflow actions fail between publish and accept: report must preserve the digest.
        os.environ["RELEASE_JOB_STATUS"] = "failure"
        with contextlib.redirect_stdout(io.StringIO()):
            self.release.report()
        self.assertEqual(self.release.state["stage"], "digest_observed")
        self.assertEqual(self.release.state["digest"], DIGEST)
        self.assertEqual(self.release.state["validation"], "incomplete")
        self.assertIn("Positive-access investigation only", self.release.state["next_action"])
        self.assertEqual(len(self.pushes()), 1)

    def test_post_creation_inaccessible_stops_recovery(self):
        self.first_handoff()
        self.replies = [unavailable(), unavailable(), unavailable()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)
        os.environ.update(GITHUB_EVENT_NAME="workflow_dispatch")
        self.release = r.Release(self.workflow, self.source, self.base / "normal/record.json")
        self.release.resolve()
        self.replies = [unavailable()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)

    def test_first_creation_push_error_or_missing_digest_never_reopens_on_rerun(self):
        for failure in [True, False]:
            with self.subTest(push_error=failure):
                self.commands.clear()
                os.environ["GITHUB_RUN_ATTEMPT"] = "1"
                self.fresh_record()
                self.first_handoff()
                self.fail_command = (lambda c: c[:2] == ("docker", "push")) if failure else None
                self.push_output = "push completed without a digest"
                with self.assertRaises(r.ReleaseError):
                    self.release.publish()
                record = json.loads(self.release.record.read_text())
                self.assertTrue(record["push_attempted"])
                self.assertNotIn("digest", record)
                self.assertEqual(record["stage"], "push_attempted_outcome_uncertain")
                self.assertEqual(len(self.pushes()), 1)
                self.fail_command = None
                os.environ["GITHUB_RUN_ATTEMPT"] = "2"
                self.fresh_record()
                self.replies = [unavailable()]
                with self.assertRaises(r.ReleaseError):
                    self.release.publish()
                self.assertEqual(len(self.pushes()), 1)

    def test_first_creation_pre_push_failure_has_no_ordinary_retry_allowance(self):
        self.first_handoff()
        self.fail_command = lambda c: c[0] == "bash"
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertFalse(self.release.state["push_attempted"])
        os.environ["RELEASE_JOB_STATUS"] = "failure"
        with contextlib.redirect_stdout(io.StringIO()):
            self.release.report()
        self.assertIn("narrowly reviewed creation adjustment", self.release.state["next_action"])
        self.fail_command = None
        for event, attempt in [("push", "2"), ("workflow_dispatch", "1")]:
            os.environ.update(GITHUB_EVENT_NAME=event, GITHUB_RUN_ATTEMPT=attempt)
            self.fresh_record()
            self.replies = [unavailable()]
            with self.subTest(event=event), self.assertRaises(r.ReleaseError):
                self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_first_handoff_reuses_visible_digest_without_build_or_new_attestations(self):
        self.first_handoff()
        self.replies = visible([version(tags=["0.1.1"])]) * 2
        self.release.publish()
        self.signer = OLD_WORKFLOW
        self.release.accept()
        self.assertTrue(self.release.state["creation_eligible"])
        self.assertEqual(self.release.state["package_observation"], "accessible")
        self.assertEqual(self.release.state["validation"], "passed")
        self.assertIn("new_image=false", (self.base / "outputs").read_text())
        self.assertFalse((self.release.record.parent / "release-identity.json").exists())
        self.assertFalse(any(c[:2] in [("docker", "build"), ("docker", "push")] for c, *_ in self.commands))

    def test_visible_package_cannot_fall_back_to_unavailable_during_first_handoff(self):
        self.first_handoff()
        self.replies = visible() + [unavailable()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_local_image_change_before_first_push_stops_creation(self):
        self.first_handoff()
        def change_image(command):
            if command[0] == "bash":
                self.local_image_id = "replacement-local-image"
            return False
        self.fail_command = change_image
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])

    def test_first_creation_postpush_conflicts_never_emit_attestation_handoff(self):
        for post in [[unavailable()], [(403, {}, None)], visible(),
                     visible([version(tags=["0.1.1"], digest=OTHER)]),
                     [(200, dict(package(), repository={}), None)]]:
            with self.subTest(post=post):
                self.commands.clear()
                self.fresh_record()
                self.first_handoff()
                (self.base / "outputs").write_text("")
                self.replies = [unavailable(), unavailable()] + post
                with self.assertRaises(r.ReleaseError):
                    self.release.publish()
                self.assertEqual(len(self.pushes()), 1)
                self.assertTrue(self.release.state["push_attempted"])
                self.assertEqual(self.release.state["digest"], DIGEST)
                self.assertNotIn("new_image=true", (self.base / "outputs").read_text())
                self.assertFalse((self.release.record.parent / "release-identity.json").exists())

    def test_first_creation_incomplete_identity_or_private_pull_stays_incomplete(self):
        for prefix, stage in [(('gh', 'attestation', 'verify'), 'digest_observed'),
                              (('docker', '--config'), 'identity_verified')]:
            with self.subTest(prefix=prefix):
                self.commands.clear()
                self.fresh_record()
                self.first_handoff()
                self.release.publish()
                self.fail_command = lambda c: c[:len(prefix)] == prefix
                with self.assertRaises(r.ReleaseError):
                    self.release.accept()
                self.assertEqual(self.release.state["stage"], stage)
                self.assertEqual(self.release.state["validation"], "incomplete")
                self.assertEqual(len(self.pushes()), 1)
                self.fail_command = None

    def test_later_version_unavailable_package_stops_before_build(self):
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true",
                          RELEASE_TAG="v0.1.2", RELEASE_VERSION="0.1.2",
                          RELEASE_PLEASE_RESULT="success")
        self.manifest_version = "0.1.2"
        (self.source / "Cargo.lock").write_text('[[package]]\nname="tines-mcp"\nversion="0.1.2"\n')
        self.remote_tag_name = "v0.1.2"
        self.api.release.return_value["tag_name"] = "v0.1.2"
        self.release.resolve()
        self.replies = [unavailable()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertFalse(self.release.state["creation_eligible"])
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[:2] == ("docker", "build") for c, *_ in self.commands))


    def select_existing(self):
        self.fresh_record()
        self.replies = visible([version(tags=["0.1.1"])])
        self.release.publish()
        self.replies = visible([version(tags=["0.1.1"])])

    def test_existing_image_bad_attestation_zero_builds_zero_pushes(self):
        self.select_existing()
        self.fail_command = lambda c: c[:3] == ("gh", "attestation", "verify")
        with self.assertRaises(r.ReleaseError):
            self.release.accept()
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[:2] == ("docker", "build") for c, *_ in self.commands))
        self.assertFalse((self.release.record.parent / "release-identity.json").exists())

    def test_existing_image_label_conflict_zero_replacement_writes(self):
        self.select_existing()
        self.label_overrides["org.opencontainers.image.revision"] = "d" * 40
        with self.assertRaises(r.ReleaseError):
            self.release.accept()
        self.assertEqual(self.pushes(), [])
        self.assertNotEqual(self.release.state["validation"], "passed")

    def test_acceptance_failures_never_publish(self):
        for failed in [lambda c: c[:3] == ("gh", "attestation", "verify"),
                       lambda c: c[:2] == ("docker", "--config"), lambda c: c[0] == "bash"]:
            self.select_existing()
            self.fail_command = failed
            with self.assertRaises(r.ReleaseError):
                self.release.accept()
            self.fail_command = None
            self.assertEqual(self.pushes(), [])
            self.assertNotEqual(self.release.state["validation"], "passed")

    def test_completion_conflicts_and_tag_movement(self):
        self.select_existing()
        self.remote = "d" * 40
        with self.assertRaises(r.ReleaseError):
            self.release.accept()
        self.remote = SOURCE_SHA
        self.replies = visible([version(tags=["0.1.1"], digest=OTHER)])
        with self.assertRaises(r.ReleaseError):
            self.release.accept()
        self.assertEqual(self.pushes(), [])


class GitSourceTests(unittest.TestCase):
    """Real Git and the CLI entry point; no registry calls or synthetic source resolution."""
    def test_real_annotated_tag_remote_and_separate_workflow_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            remote, work = base / "remote", base / "work"
            def git(*args, cwd=work):
                return subprocess.check_output(["git", *args], cwd=cwd, stderr=subprocess.DEVNULL, text=True).strip()
            remote.mkdir()
            git("init", "--bare", str(remote), cwd=base)
            git("clone", str(remote), str(work), cwd=base)
            git("config", "user.name", "Release test")
            git("config", "user.email", "release@example.invalid")
            (work / "source").write_text("version 0.1.1\n")
            git("add", ".")
            git("commit", "-m", "feat: source")
            sha = git("rev-parse", "HEAD")
            git("tag", "-a", "v0.1.1", "-m", "source")
            (work / "orchestration").write_text("workflow\n")
            git("add", ".")
            git("commit", "-m", "ci: orchestration")
            workflow_sha = git("rev-parse", "HEAD")
            git("push", "origin", "HEAD:main", "--tags")
            env = dict(os.environ, **environment(RELEASE_SOURCE=sha, GITHUB_SHA=workflow_sha,
                                                GITHUB_OUTPUT=str(base / "output")))
            # Re-enable real subprocess execution for the helper imported in this process.
            with patch.dict(os.environ, env):
                state = r.dispatch(env)
                r.resolve(work, state)
            cli = ["python3", str(ROOT / "scripts/release.py"), "resolve", "--workflow", str(work),
                   "--source", str(base / "source"), "--record", str(base / "record.json")]
            row = dict(id=41, tag_name="v0.1.1", target_commitish=sha,
                       prerelease=False, draft=True, immutable=False, published_at=None)
            with patch.dict(os.environ, env), patch.object(r, "Releases") as api:
                api.return_value.listing.return_value = [row]
                api.return_value.release.return_value = row
                api.return_value.evidence.return_value = None
                r.Release(work, base / "source", base / "record.json").resolve()
            self.assertIn(f"source_sha={sha}", (base / "output").read_text())
            # Move only the remote tag; local tag remains the old source.
            git("push", "--force", "origin", "HEAD:refs/tags/v0.1.1")
            result = subprocess.run(cli, env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Remote tag/source mismatch", result.stderr)
            self.assertFalse(json.loads((base / "record.json").read_text())["push_attempted"])


class WorkflowTests(unittest.TestCase):
    def test_ephemeral_docker_configuration_and_cleanup(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        steps = workflow.split("\n  release:\n", 1)[1].split("    steps:\n", 1)[1]
        initialize, *_, cleanup = steps.split("      - name: ")[1:]
        self.assertTrue(initialize.startswith("Initialize ephemeral Docker configuration\n"))
        self.assertTrue(cleanup.startswith("Remove ephemeral registry login\n"))
        self.assertIn("if: always()", cleanup)
        initialize_command = re.search(r"^        run: (.*)$", initialize, re.M)[1]
        cleanup_command = re.search(r"^        run: (.*)$", cleanup, re.M)[1]
        with tempfile.TemporaryDirectory(prefix="release runner ") as directory:
            runner_temp = Path(directory)
            github_env = runner_temp / "github-env"
            config = runner_temp / "release-docker"
            unrelated = runner_temp / "unrelated"
            unrelated.write_text("keep")
            env = dict(os.environ, RUNNER_TEMP=str(runner_temp), GITHUB_ENV=str(github_env))
            env.pop("DOCKER_CONFIG", None)
            # Cleanup also works if the initialization step did not complete.
            subprocess.run(["bash", "-e", "-c", cleanup_command], env=env, check=True)
            subprocess.run(["bash", "-e", "-c", initialize_command], env=env, check=True)
            self.assertEqual(github_env.read_text(), f"DOCKER_CONFIG={config}\n")
            # Actions exports GITHUB_ENV to every later step, including attestation actions.
            env["DOCKER_CONFIG"] = str(config)
            config.mkdir()
            (config / "config.json").write_text('{"auths":{"ghcr.io":{"auth":"test-only"}}}')
            subprocess.run(["bash", "-e", "-c", cleanup_command], env=env, check=True)
            self.assertFalse(config.exists())
            self.assertEqual(unrelated.read_text(), "keep")

    def test_authority_and_execution_boundaries(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        triggers = workflow.split("\non:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(re.findall(r"^  ([a-z_]+):", triggers, re.M), ["push", "workflow_dispatch"])
        self.assertNotIn("bootstrap", triggers)
        self.assertEqual(re.findall(r"^      ([a-z_]+):", triggers, re.M), ["tag", "source_sha"])
        self.assertIn("group: tines-mcp-release\n  cancel-in-progress: false", workflow)
        permissions = workflow.split("\n  release:\n", 1)[1].split("    permissions:\n", 1)[1].split("    env:\n", 1)[0]
        self.assertEqual(set(permissions.splitlines()),
                         {"      contents: write", "      packages: write", "      attestations: write", "      id-token: write"})
        self.assertIn("ref: ${{ github.sha }}\n          path: workflow", workflow)
        self.assertIn("ref: ${{ steps.source.outputs.source_sha }}\n          path: source", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        commands = re.findall(r"^        run: (.*)$", workflow, re.M)
        helpers = [c for c in commands if c.startswith("python3")]
        self.assertEqual([c.split()[2] for c in helpers], ["resolve", "publish", "accept", "retain", "finalize", "report"])
        for command in helpers:
            self.assertNotIn("${{", command)
            self.assertEqual(command.split()[3:],
                             ["--workflow", "workflow", "--source", "source", "--record", "release/record.json"])
        self.assertEqual(workflow.count("if: steps.image.outputs.new_image == 'true'"), 2)
        self.assertEqual(workflow.count("create-storage-record: false"), 2)
        self.assertEqual(workflow.count("push-to-registry: true"), 2)
        self.assertLess(workflow.index("release.py publish"), workflow.index("uses: actions/attest@"))
        self.assertLess(workflow.rindex("uses: actions/attest@"), workflow.index("release.py accept"))
        for action in re.findall(r"uses: (.*)", workflow):
            self.assertRegex(action, r"^[a-zA-Z0-9/-]+@[0-9a-f]{40}(?: # .*)?$")
        self.assertIn("GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}", workflow)
        self.assertNotRegex(workflow, r"secrets\.(?!GITHUB_TOKEN)[A-Z_]+")
        self.assertIn("if: always()\n        env:\n          RELEASE_JOB_STATUS:", workflow)
        self.assertIn("retention-days: 90", workflow)
        for command in commands:
            result = subprocess.run(["bash", "-n"], input=command, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_ci_keeps_rust_and_shared_real_container_gate(self):
        workflow = (ROOT / ".github/workflows/test.yml").read_text()
        for command in ["cargo fmt --all -- --check", "cargo clippy --locked --all-targets -- -D warnings",
                        "cargo test --locked", "docker build --file Containerfile --tag tines-mcp:ci .",
                        "bash scripts/container-smoke.sh tines-mcp:ci", "python3 -m unittest discover -s tests/release -v"]:
            self.assertIn(command, workflow)


if __name__ == "__main__":
    unittest.main()
