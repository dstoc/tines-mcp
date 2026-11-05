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
                  RELEASE_TAG="v0.1.0", RELEASE_SOURCE=SOURCE_SHA, RELEASE_MODE="normal",
                  BOOTSTRAP_CONFIRMATION="", GITHUB_ACTOR="dstoc", GITHUB_ACTOR_ID="539597",
                  GITHUB_TRIGGERING_ACTOR="dstoc", GITHUB_RUN_ATTEMPT="1", GITHUB_RUN_ID="123",
                  GH_TOKEN="test-only-token")
    result.update(overrides)
    return result


def package(identifier=17):
    return dict(id=identifier, name="tines-mcp", package_type="container",
                owner=dict(login="dstoc", id=539597), repository=dict(full_name=r.REPO))


def version(identifier=1, digest=DIGEST, tags=None):
    return dict(id=identifier, name=digest, metadata=dict(container=dict(tags=tags or [])))


def absent():
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
    def test_valid_and_bootstrap(self):
        self.assertEqual(r.dispatch(environment())["version"], "0.1.0")
        self.assertEqual(r.dispatch(environment(RELEASE_MODE="bootstrap",
                         BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA)))["mode"], "bootstrap")

    def test_dispatch_gates(self):
        for field, value in [("GITHUB_REPOSITORY", "other/tines-mcp"), ("GITHUB_REF", "refs/tags/v0.1.0"),
                             ("GITHUB_EVENT_NAME", "push"), ("RELEASE_SOURCE", "main"),
                             ("RELEASE_SOURCE", "a" * 39), ("GITHUB_SHA", "main"),
                             ("RELEASE_TAG", "v01.1.0"), ("RELEASE_TAG", "v1.2.3-rc.1"),
                             ("RELEASE_TAG", "v1.2.3\n"), ("RELEASE_TAG", "$(touch unexpected)"),
                             ("RELEASE_MODE", "auto"), ("BOOTSTRAP_CONFIRMATION", "anything")]:
            with self.subTest(field=field, value=value), self.assertRaises(r.ReleaseError):
                r.dispatch(environment(**{field: value}))

    def test_bootstrap_owner_rerun_and_confirmation_gates(self):
        base = environment(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA))
        for field, value in [("GITHUB_ACTOR_ID", "123"), ("GITHUB_ACTOR", "other"),
                             ("GITHUB_TRIGGERING_ACTOR", "other"), ("GITHUB_RUN_ATTEMPT", "2"),
                             ("RELEASE_TAG", "v0.2.0"), ("RELEASE_SOURCE", "d" * 40),
                             ("BOOTSTRAP_CONFIRMATION", ""), ("BOOTSTRAP_CONFIRMATION", r.confirmation(SOURCE_SHA) + " ")]:
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                r.dispatch(dict(base, **{field: value}))


class RegistryTests(unittest.TestCase):
    def lookup(self, replies, bootstrap=False):
        with patch.object(r.Registry, "get", side_effect=replies):
            return r.Registry("test").lookup("0.1.0", bootstrap)

    def test_positive_access_and_absent_tag(self):
        self.assertEqual(self.lookup(visible()), (17, None))
        self.assertEqual(self.lookup(visible([version(tags=["0.1.0"])])), (17, DIGEST))

    def test_http_errors_never_mean_absence(self):
        for status in [301, 401, 403, 404, 429, 500, 503]:
            with self.subTest(status=status), self.assertRaises(r.ReleaseError):
                self.lookup([(status, {}, None)])
            with self.subTest(version_status=status), self.assertRaises(r.ReleaseError):
                self.lookup([(200, package(), None), (status, {}, None)])

    def test_bootstrap_requires_structured_exact_route_404(self):
        with patch.object(r.Registry, "get", return_value=absent()) as get:
            self.assertEqual(r.Registry("test").lookup("0.1.0", True), (None, None))
            get.assert_called_once_with(r.PACKAGE)
        for response in [(200, package(), None), (404, {}, None), (404, "Not Found", None),
                         (404, dict(message="Not Found", documentation_url="https://evil.example/"), None),
                         *[(s, {}, None) for s in [401, 403, 429, 500, 503]]]:
            with self.subTest(response=response), self.assertRaises(r.ReleaseError):
                self.lookup([response], True)

    def test_authentication_and_transport_fail_closed(self):
        with self.assertRaises(r.ReleaseError):
            r.Registry("")
        for fail in [OSError("offline"), r.http.client.HTTPException("offline")]:
            with patch.object(r.http.client.HTTPSConnection, "request", side_effect=fail), self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.0")

    def test_fixed_api_origin_no_redirect_and_malformed_json(self):
        with patch.object(r.http.client, "HTTPSConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.read.return_value = 302, b'{}'
            with self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.0")
            connection.assert_called_once_with("api.github.com", timeout=60)
            self.assertEqual(connection.return_value.request.call_args.args[:2], ("GET", r.PACKAGE))
            self.assertEqual(connection.return_value.request.call_args.kwargs["headers"]["Authorization"], "Bearer test")
            response.read.return_value = b'not json'
            with self.assertRaises(r.ReleaseError):
                r.Registry("test").lookup("0.1.0", True)

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
                   (200, [version(101, DIGEST, ["0.1.0"])], link(page=1, relation="prev"))]
        self.assertEqual(self.lookup(replies), (17, DIGEST))
        for fail in [(503, {}, None), (200, {}, None)]:
            with self.subTest(fail=fail), self.assertRaises(r.ReleaseError):
                self.lookup(replies[:2] + [fail])

    def test_malformed_duplicate_or_conflicting_versions(self):
        for rows in [{}, [dict(id=1)], [version(tags=["0.1.0", "0.1.0"])],
                     [version(), version()], [version(), version(2)],
                     [version(tags=["0.1.0"]), version(2, OTHER, ["0.1.0"])],
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
        (self.source / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "tines-mcp"\nversion = "0.1.0"\n')
        self.env = environment(GITHUB_OUTPUT=str(self.base / "outputs"), DOCKER_CONFIG=str(self.base / "docker"))
        self.patch_env = patch.dict(os.environ, self.env, clear=True)
        self.patch_env.start()
        self.addCleanup(self.patch_env.stop)
        self.commands, self.fail_command = [], None
        self.manifest_version = "0.1.0"
        self.head = SOURCE_SHA
        self.remote = SOURCE_SHA
        self.user = "10001:10001"
        self.label_overrides = {}
        self.replies = visible() + visible() + visible([version(tags=["0.1.0"])])
        self.signer = WORKFLOW_SHA
        self.patch_run = patch.object(r, "run", side_effect=self.command)
        self.patch_run.start()
        self.addCleanup(self.patch_run.stop)
        self.patch_http = patch.object(r.Registry, "get", side_effect=self.http)
        self.patch_http.start()
        self.addCleanup(self.patch_http.stop)
        self.release = r.Release(self.workflow, self.source, self.base / "release/record.json")
        self.release.resolve()

    def http(self, path):
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
            return f"{self.remote}\trefs/tags/v0.1.0"
        if args[:2] == ("cargo", "metadata"):
            return json.dumps(dict(packages=[dict(name="tines-mcp", version=self.manifest_version, source=None)]))
        if args[:3] == ("docker", "image", "inspect"):
            return json.dumps([dict(Id="local-immutable-image-id", Os="linux", Architecture="amd64",
                                   Config=dict(User=self.user, Labels=dict(r.labels(self.release.state), **self.label_overrides)))])
        if args[:2] == ("docker", "--config"):
            self.assertEqual(json.loads((Path(args[2]) / "config.json").read_text()),
                             {"auths": {"ghcr.io": {}}})
        if args[:2] == ("docker", "push"):
            # The record MUST exist before the real write command is dispatched.
            record = json.loads(self.release.record.read_text())
            self.assertTrue(record["push_attempted"])
            self.assertEqual(record["stage"], "push_attempted_outcome_uncertain")
            return f"0.1.0: digest: {DIGEST} size: 1234"
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

    def publish_bootstrap(self):
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA))
        self.release.resolve()
        self.replies = [absent(), absent()] + visible([version(tags=["0.1.0"])])
        self.release.publish()

    def test_publish_once_then_verify_and_anonymous_runtime(self):
        self.release.publish()
        self.assertEqual(len(self.pushes()), 1)
        self.assertEqual(self.release.state["stage"], "digest_observed")
        self.assertEqual(json.loads((self.release.record.parent / "release-identity.json").read_text()),
                         r.identity(self.release.state, WORKFLOW_SHA))
        self.replies += visible([version(tags=["0.1.0"])])
        self.release.accept()
        self.assertEqual(self.release.state["validation"], "passed")
        calls = [args for args, *_ in self.commands]
        self.assertEqual(sum(c[:2] == ("docker", "build") for c in calls), 1)
        self.assertEqual(sum(c[0] == "bash" for c in calls), 2)
        anonymous = next(c for c in calls if c[:2] == ("docker", "--config"))
        self.assertIn(f"{r.IMAGE}@{DIGEST}", anonymous)
        self.assertFalse(Path(anonymous[2]).exists())

    def test_bootstrap_then_positive_access_and_normal_recovery(self):
        self.publish_bootstrap()
        self.assertEqual(self.release.state["package_id"], 17)
        self.assertEqual(len(self.pushes()), 1)
        # A fresh normal dispatch reuses the digest, without building or writing it.
        os.environ.update(RELEASE_MODE="normal", BOOTSTRAP_CONFIRMATION="")
        self.release = r.Release(self.workflow, self.source, self.base / "recovery/record.json")
        self.release.resolve()
        self.replies = visible([version(tags=["0.1.0"])]) * 2
        self.release.publish()
        self.signer = OLD_WORKFLOW
        self.release.accept()
        self.assertTrue(self.release.state["reused"])
        self.assertEqual(self.release.state["original_workflow_sha"], OLD_WORKFLOW)
        self.assertEqual(len(self.pushes()), 1)
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

    def test_bootstrap_actor_gates_run_before_any_registry_work(self):
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA),
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
        for replies in [[absent()], [(403, {}, None)], [r.ReleaseError("transport")],
                        [(200, package(), None), (503, {}, None)]]:
            self.replies = replies
            with self.subTest(replies=replies), self.assertRaises(r.ReleaseError):
                self.release.publish()
            self.assertEqual(self.pushes(), [])

    def test_bootstrap_preflight_errors_zero_pushes(self):
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA))
        self.release.resolve()
        for reply in [(200, package(), None), (404, {}, None), (401, {}, None), (403, {}, None),
                      (429, {}, None), (503, {}, None), r.ReleaseError("transport")]:
            self.replies = [reply]
            with self.subTest(reply=reply), self.assertRaises(r.ReleaseError):
                self.release.publish()
            self.assertEqual(self.pushes(), [])

    def test_package_or_tag_appearing_before_push_zero_writes(self):
        for second in [visible([version(tags=["0.1.0"])]), visible(identifier=18)]:
            self.replies = visible() + second
            with self.assertRaises(r.ReleaseError):
                self.release.publish()
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA))
        self.release.resolve()
        self.replies = [absent(), (200, package(), None)]
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
        self.assertIn("Normal-mode investigation only", self.release.state["next_action"])

    def test_post_push_metadata_error_or_digest_conflict_no_second_push(self):
        for post in [[(403, {}, None)], visible([version(tags=["0.1.0"], digest=OTHER)]),
                     visible([version(tags=["0.1.0"])], identifier=18)]:
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
        self.assertIn("Normal-mode investigation only", self.release.state["next_action"])
        self.assertEqual(len(self.pushes()), 1)

    def test_post_bootstrap_inaccessible_stops_normal_recovery(self):
        os.environ.update(RELEASE_MODE="bootstrap", BOOTSTRAP_CONFIRMATION=r.confirmation(SOURCE_SHA))
        self.release.resolve()
        self.replies = [absent(), absent(), absent()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)
        os.environ.update(RELEASE_MODE="normal", BOOTSTRAP_CONFIRMATION="")
        self.release = r.Release(self.workflow, self.source, self.base / "normal/record.json")
        self.release.resolve()
        self.replies = [absent()]
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(len(self.pushes()), 1)

    def select_existing(self):
        self.fresh_record()
        self.replies = visible([version(tags=["0.1.0"])])
        self.release.publish()
        self.replies = visible([version(tags=["0.1.0"])])

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
        self.replies = visible([version(tags=["0.1.0"], digest=OTHER)])
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
            (work / "source").write_text("version 0.1.0\n")
            git("add", ".")
            git("commit", "-m", "feat: source")
            sha = git("rev-parse", "HEAD")
            git("tag", "-a", "v0.1.0", "-m", "source")
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
            result = subprocess.run(cli, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"source_sha={sha}", (base / "output").read_text())
            # Move only the remote tag; local tag remains the old source.
            git("push", "--force", "origin", "HEAD:refs/tags/v0.1.0")
            result = subprocess.run(cli, env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Remote tag/source mismatch", result.stderr)
            self.assertFalse(json.loads((base / "record.json").read_text())["push_attempted"])


class WorkflowTests(unittest.TestCase):
    def test_authority_and_execution_boundaries(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        triggers = workflow.split("\non:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(re.findall(r"^  ([a-z_]+):", triggers, re.M), ["workflow_dispatch"])
        self.assertIn("options: [normal, bootstrap]", triggers)
        self.assertIn("default: normal", triggers)
        self.assertIn("group: tines-mcp-release\n  cancel-in-progress: false", workflow)
        permissions = workflow.split("    permissions:\n", 1)[1].split("    env:\n", 1)[0]
        self.assertEqual(set(permissions.splitlines()),
                         {"      contents: read", "      packages: write", "      attestations: write", "      id-token: write"})
        self.assertIn("ref: ${{ github.sha }}\n          path: workflow", workflow)
        self.assertIn("ref: ${{ steps.source.outputs.source_sha }}\n          path: source", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        commands = re.findall(r"^        run: (.*)$", workflow, re.M)
        helpers = [c for c in commands if c.startswith("python3")]
        self.assertEqual([c.split()[2] for c in helpers], ["resolve", "publish", "accept", "report"])
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
