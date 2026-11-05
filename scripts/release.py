#!/usr/bin/env python3
"""GHCR release checks. Package creation is limited to the first genuine 0.1.1 handoff."""

import argparse
import http.client
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
from urllib.parse import parse_qs, urlsplit

REPO = "dstoc/tines-mcp"
SOURCE = f"https://github.com/{REPO}"
IMAGE = f"ghcr.io/{REPO}"
WORKFLOW = f"{SOURCE}/.github/workflows/release.yml@refs/heads/main"
PREDICATE = f"{SOURCE}/attestations/release-identity/v1"
PROVENANCE = "https://slsa.dev/provenance/v1"
PACKAGE = "/users/dstoc/packages/container/tines-mcp"
VERSIONS = f"{PACKAGE}/versions"
SHA = r"[0-9a-f]{40}"
DIGEST = r"sha256:[0-9a-f]{64}"
TAG = r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"


class ReleaseError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def run(*args, cwd=None, env=None, input=None):
    # Never print token-bearing input, response bodies, or subprocess exceptions.
    # Build output contains no injected credentials and is needed to diagnose compilation.
    capture = args[:2] != ("docker", "build")
    result = subprocess.run(args, cwd=cwd, env=env, input=input, text=True,
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.PIPE if capture else None, check=False)
    require(result.returncode == 0,
            f"{args[0]} {args[1]} failed (exit {result.returncode}); stop and inspect this stage")
    return (result.stdout or "").strip()


def read_json(value):
    def unique_object(pairs):
        result = {}
        for key, item in pairs:
            require(key not in result, "Duplicate JSON field")
            result[key] = item
        return result

    def invalid_constant(value):
        raise ReleaseError("Invalid JSON constant")

    try:
        return json.loads(value, object_pairs_hook=unique_object, parse_constant=invalid_constant)
    except (ValueError, TypeError):
        raise ReleaseError("Invalid JSON response") from None


def nested(value, *keys):
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value, indent=2) + "\n")
    temp.replace(path)


def output(**values):
    with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
        for key, value in values.items():
            stream.write(f"{key}={value}\n")


def dispatch(env):
    require(env.get("GITHUB_REPOSITORY") == REPO, "Wrong repository")
    event = env.get("GITHUB_EVENT_NAME")
    require(event in ("workflow_dispatch", "push"), "Use main push handoff or explicit dispatch")
    require(env.get("GITHUB_REF") == "refs/heads/main", "Release from main")
    require(re.fullmatch(SHA, env.get("GITHUB_SHA", "")), "Invalid workflow SHA")
    tag, sha = env.get("RELEASE_TAG", ""), env.get("RELEASE_SOURCE", "")
    require(re.fullmatch(TAG, tag), "Use a stable vX.Y.Z tag without leading zeroes")
    require(re.fullmatch(SHA, sha), "Use the full lowercase source SHA")
    require(not env.get("RELEASE_MODE") and not env.get("BOOTSTRAP_CONFIRMATION"),
            "Obsolete publication mode/confirmation supplied; use the reviewed workflow")
    if event == "push":
        require(env.get("RELEASE_PLEASE_RESULT") == "success"
                and env.get("RELEASE_CREATED") == "true", "Successful release-created handoff required")
        require(env.get("RELEASE_VERSION") == tag[1:], "Release Please tag/version mismatch")
        require(env.get("RELEASE_DRAFT") == "true" and
                re.fullmatch(r"[1-9][0-9]*", env.get("RELEASE_ID", "")),
                "Draft release ID handoff required")
    # This allowance is not proof of absence: release authorization must establish
    # authoritative owner absence, exclusive writers, and no unresolved prior push.
    creation_eligible = (event == "push" and tag == "v0.1.1"
                         and env.get("GITHUB_RUN_ATTEMPT") == "1")
    return dict(tag=tag, version=tag[1:], source_sha=sha, workflow_sha=env["GITHUB_SHA"],
                platform="linux/amd64", creation_eligible=creation_eligible, image=f"{IMAGE}:{tag[1:]}",
                actor=env.get("GITHUB_ACTOR"), actor_id=env.get("GITHUB_ACTOR_ID"),
                triggering_actor=env.get("GITHUB_TRIGGERING_ACTOR"),
                run_attempt=env.get("GITHUB_RUN_ATTEMPT"), run_id=env["GITHUB_RUN_ID"],
                run_url=f"{SOURCE}/actions/runs/{env['GITHUB_RUN_ID']}")


def remote_tag(workflow, state):
    tag = f"refs/tags/{state['tag']}"
    lines = run("git", "ls-remote", "--exit-code", "origin", tag, tag + "^{}", cwd=workflow)
    refs = {}
    for line in lines.splitlines():
        parts = line.split("\t")
        require(len(parts) == 2 and re.fullmatch(SHA, parts[0])
                and parts[1] in (tag, tag + "^{}") and parts[1] not in refs,
                "Malformed remote tag response")
        refs[parts[1]] = parts[0]
    require(tag in refs, "Remote tag missing")
    require(refs.get(tag + "^{}", refs[tag]) == state["source_sha"], "Remote tag/source mismatch")


def resolve(workflow, state):
    require(run("git", "rev-parse", "HEAD", cwd=workflow) == state["workflow_sha"],
            "Workflow checkout mismatch")
    require(run("git", "rev-parse", "--verify", state["tag"] + "^{commit}", cwd=workflow)
            == state["source_sha"], "Local tag/source mismatch")
    run("git", "merge-base", "--is-ancestor", state["source_sha"], "HEAD", cwd=workflow)
    remote_tag(workflow, state)


def source_version(source, state):
    require(run("git", "rev-parse", "HEAD", cwd=source) == state["source_sha"],
            "Source checkout mismatch")
    metadata = read_json(run("cargo", "metadata", "--no-deps", "--format-version", "1",
                             "--locked", cwd=source))
    packages = [p for p in metadata["packages"] if p["name"] == "tines-mcp" and p["source"] is None]
    require(len(packages) == 1 and packages[0]["version"] == state["version"],
            "Cargo manifest version mismatch")
    lock = tomllib.loads((Path(source) / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] == "tines-mcp" and "source" not in p]
    require(len(packages) == 1 and packages[0]["version"] == state["version"],
            "Cargo.lock version mismatch")


class Registry:
    def __init__(self, token):
        require(bool(token), "Actions GITHUB_TOKEN required")
        self.token = token

    def get(self, path):
        # Fixed TLS origin, no redirect following and no error-to-absence conversion.
        connection = http.client.HTTPSConnection("api.github.com", timeout=60)
        try:
            connection.request("GET", path, headers={
                "Authorization": f"Bearer {self.token}", "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "tines-mcp-release"})
            response = connection.getresponse()
            body = response.read(8 * 1024 * 1024 + 1)
            require(len(body) <= 8 * 1024 * 1024, "Package response too large")
            return response.status, read_json(body), response.getheader("Link")
        except (OSError, http.client.HTTPException):
            raise ReleaseError("Package API transport failure; no absence established") from None
        finally:
            connection.close()

    def lookup(self, version, allow_unavailable=False):
        status, package, link = self.get(PACKAGE)
        if status == 404 and allow_unavailable:
            require(isinstance(package, dict)
                    and package.get("message") == "Not Found"
                    and isinstance(package.get("documentation_url"), str)
                    and package["documentation_url"].startswith("https://docs.github.com/rest/")
                    and link is None,
                    "Creation allowance requires the exact package route's structured 404 without pagination")
            return None, None
        require(status == 200, f"Package metadata HTTP {status}; no absence established")
        require(isinstance(package, dict) and type(package.get("id")) is int and package["id"] > 0
                and package.get("name") == "tines-mcp" and package.get("package_type") == "container"
                and nested(package, "owner", "login") == "dstoc"
                and nested(package, "owner", "id") == 539597
                and nested(package, "repository", "full_name") == REPO,
                "Wrong package identity or repository association")
        package_id = package["id"]
        path, page, seen_ids, seen_digests, matches = f"{VERSIONS}?state=active&per_page=100", 1, set(), set(), []
        while True:
            status, rows, link = self.get(path)
            require(status == 200, f"Versions page {page} HTTP {status}; no absence established")
            require(isinstance(rows, list) and len(rows) <= 100, "Invalid versions response")
            for row in rows:
                require(isinstance(row, dict) and type(row.get("id")) is int and row["id"] > 0,
                        "Invalid version ID")
                digest = row.get("name", "")
                tags = nested(row, "metadata", "container", "tags")
                require(isinstance(digest, str) and re.fullmatch(DIGEST, digest)
                        and isinstance(tags, list) and all(isinstance(t, str) and t for t in tags)
                        and len(tags) == len(set(tags)), "Invalid version digest or tags")
                require(row["id"] not in seen_ids and digest not in seen_digests, "Duplicate version")
                seen_ids.add(row["id"])
                seen_digests.add(digest)
                if version in tags:
                    matches.append(digest)
            next_path = next_page(link, page)
            if next_path is None:
                break
            require(len(rows) == 100 and page < 1000, "Incomplete or excessive version pagination")
            path, page = next_path, page + 1
        require(len(matches) <= 1, "Conflicting release digests")
        return package_id, matches[0] if matches else None


def next_page(link, page):
    if link is None:
        return None
    require(bool(link), "Empty pagination header")
    next_path, relations = None, {}
    for item in link.split(","):
        match = re.fullmatch(r'\s*<([^>]+)>;\s*rel="(next|prev|first|last)"\s*', item)
        require(match is not None, "Malformed pagination")
        url, relation = match.groups()
        require(relation not in relations, "Duplicate pagination relation")
        parsed = urlsplit(url)
        require(parsed.scheme == "https" and parsed.netloc == "api.github.com"
                and parsed.path == VERSIONS and not parsed.fragment, "Unexpected pagination endpoint")
        query = parse_qs(parsed.query, keep_blank_values=True)
        require(set(query) == {"state", "per_page", "page"} and query["state"] == ["active"]
                and query["per_page"] == ["100"] and len(query["page"]) == 1
                and re.fullmatch(r"[1-9][0-9]*", query["page"][0]), "Invalid pagination query")
        target = int(query["page"][0])
        relations[relation] = target
        if relation == "next":
            require(target == page + 1, "Skipped or repeated version page")
            next_path = parsed.path + "?" + parsed.query
        elif relation == "prev":
            require(target == page - 1, "Invalid previous page")
        elif relation == "first":
            require(target == 1, "Invalid first page")
        elif relation == "last":
            require(target >= page, "Invalid last page")
    require(relations.get("last", page) == page or next_path is not None, "Incomplete pagination")
    return next_path


def labels(state):
    return {"org.opencontainers.image.source": SOURCE,
            "org.opencontainers.image.revision": state["source_sha"],
            "org.opencontainers.image.version": state["version"]}


def inspect_image(image, state):
    rows = read_json(run("docker", "image", "inspect", image))
    require(isinstance(rows, list) and len(rows) == 1, "Image inspect shape")
    data = rows[0]
    require(isinstance(data, dict), "Image inspect object missing")
    require(data.get("Os") == "linux" and data.get("Architecture") == "amd64", "Wrong image platform")
    require(nested(data, "Config", "User") == "10001:10001", "Wrong image user")
    actual = nested(data, "Config", "Labels") or {}
    require(isinstance(actual, dict) and all(actual.get(k) == v for k, v in labels(state).items()),
            "Image labels mismatch")
    require(isinstance(data.get("Id"), str) and bool(data["Id"]), "Image ID missing")
    return data["Id"]


def smoke(workflow, image):
    run("bash", str(Path(workflow) / "scripts/container-smoke.sh"), image)


def identity(state, workflow_sha):
    return dict(repository=SOURCE, tag=state["tag"], version=state["version"],
                source_sha=state["source_sha"], workflow_sha=workflow_sha, platform="linux/amd64")


def verified_identity(results, state, predicate_type):
    require(isinstance(results, list) and results, "No verified attestations")
    valid = []
    for entry in results:
        # Only gh's cryptographically verified result is admissible evidence.
        result = nested(entry, "verificationResult")
        cert = nested(result, "signature", "certificate")
        statement = nested(result, "statement")
        if not isinstance(cert, dict) or not isinstance(statement, dict):
            continue
        sha = cert.get("buildSignerDigest", "")
        if not (cert.get("subjectAlternativeName") == WORKFLOW
                and cert.get("issuer") == "https://token.actions.githubusercontent.com"
                and cert.get("sourceRepositoryURI") == SOURCE
                and cert.get("sourceRepositoryRef") == "refs/heads/main"
                and isinstance(sha, str) and re.fullmatch(SHA, sha) and cert.get("sourceRepositoryDigest") == sha
                and statement.get("predicateType") == predicate_type
                and statement.get("subject") == [{"name": IMAGE, "digest": {"sha256": state["digest"][7:]}}]):
            continue
        if predicate_type == PREDICATE and statement.get("predicate") != identity(state, sha):
            continue
        valid.append(sha)
    require(bool(valid), "Verified statement/certificate does not match release identity")
    require(len(set(valid)) == 1, "Conflicting verified signer commits")
    return valid[0]


RELEASES = f"/repos/{REPO}/releases"
EVIDENCE = "adapter-release-evidence-v1.json"
MAX_EVIDENCE = 8 * 1024 * 1024


def positive_id(value):
    return type(value) is int and value > 0


def content_digest(body):
    return "sha256:" + hashlib.sha256(body).hexdigest()


class Releases:
    """Fixed repository API; redirects never receive the Actions token."""

    def __init__(self, token):
        require(bool(token), "Actions GITHUB_TOKEN required for draft access")
        self.token = token

    def request(self, method, path, body=None, host="api.github.com", binary=False):
        require(host in ("api.github.com", "uploads.github.com")
                and path.startswith(RELEASES), "Unexpected release endpoint")
        headers = {"Authorization": f"Bearer {self.token}",
                   "Accept": "application/octet-stream" if binary else "application/vnd.github+json",
                   "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "tines-mcp-release"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        connection = http.client.HTTPSConnection(host, timeout=60)
        try:
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            data = response.read(MAX_EVIDENCE + 1)
            require(len(data) <= MAX_EVIDENCE, "Release response too large")
            return response.status, data, response.getheader("Link"), response.getheader("Location")
        except (OSError, http.client.HTTPException):
            raise ReleaseError("Release API transport failure; preserve resources and inspect outcome") from None
        finally:
            connection.close()

    def json(self, method, path, body=None, expected=200, host="api.github.com"):
        status, data, link, _ = self.request(method, path, body, host)
        require(status == expected, f"Release API {method} HTTP {status}; preserve draft/tag/evidence; "
                "inspect authority and outcome without blind retries or alternate credentials")
        return read_json(data), link

    def listing(self, route):
        path, page, rows, ids = route + "?per_page=100&page=1", 1, [], set()
        while True:
            data, link = self.json("GET", path)
            require(isinstance(data, list) and len(data) <= 100, "Malformed release/asset listing")
            for row in data:
                require(isinstance(row, dict) and positive_id(row.get("id"))
                        and row["id"] not in ids, "Duplicate or malformed release/asset ID")
                ids.add(row["id"])
                rows.append(row)
            if link is None:
                return rows
            # Validate every relation, not only next. No foreign or skipped pages.
            relations = {}
            for item in link.split(","):
                match = re.fullmatch(r'\s*<([^>]+)>;\s*rel="(next|prev|first|last)"\s*', item)
                require(match is not None, "Malformed release pagination")
                url, relation = match.groups()
                parsed = urlsplit(url)
                query = parse_qs(parsed.query, keep_blank_values=True)
                require(parsed.scheme == "https" and parsed.netloc == "api.github.com"
                        and parsed.path == route and not parsed.fragment
                        and set(query) == {"per_page", "page"} and query["per_page"] == ["100"]
                        and len(query["page"]) == 1 and re.fullmatch(r"[1-9][0-9]*", query["page"][0])
                        and relation not in relations, "Unexpected release pagination endpoint")
                target = int(query["page"][0])
                require((relation == "next" and target == page + 1)
                        or (relation == "prev" and target == page - 1)
                        or (relation == "first" and target == 1)
                        or (relation == "last" and target >= page), "Invalid release page order")
                relations[relation] = target
            require(relations and (relations.get("last", page) == page or "next" in relations),
                    "Incomplete release pagination")
            if "next" not in relations:
                return rows
            require(len(data) == 100 and page < 1000, "Incomplete or excessive release pagination")
            page += 1
            path = route + f"?per_page=100&page={page}"

    def release(self, identifier):
        return self.json("GET", f"{RELEASES}/{identifier}")[0]

    def asset(self, identifier):
        status, body, link, location = self.request(
            "GET", f"{RELEASES}/assets/{identifier}", binary=True)
        require(link is None, "Unexpected binary pagination")
        if status == 302:
            parsed = urlsplit(location or "")
            require(parsed.scheme == "https" and parsed.hostname is not None
                    and parsed.hostname.endswith(".githubusercontent.com")
                    and parsed.netloc == parsed.hostname and not parsed.fragment,
                    "Unexpected asset download redirect")
            connection = http.client.HTTPSConnection(parsed.hostname, timeout=60)
            try:
                connection.request("GET", parsed.path + ("?" + parsed.query if parsed.query else ""),
                                   headers={"Accept": "application/octet-stream",
                                            "User-Agent": "tines-mcp-release"})
                response = connection.getresponse()
                status, body = response.status, response.read(MAX_EVIDENCE + 1)
            except (OSError, http.client.HTTPException):
                raise ReleaseError("Asset download transport failure; preserve existing evidence") from None
            finally:
                connection.close()
        require(status == 200 and 0 < len(body) <= MAX_EVIDENCE, "Asset download failed or exceeded bound")
        return body

    def evidence(self, identifier):
        rows = self.listing(f"{RELEASES}/{identifier}/assets")
        matches = [row for row in rows if row.get("name") == EVIDENCE]
        require(len(matches) <= 1, "Duplicate durable evidence assets")
        if not matches:
            return None
        row = matches[0]
        require(row.get("state") == "uploaded" and type(row.get("size")) is int
                and 0 < row["size"] <= MAX_EVIDENCE and row.get("content_type") == "application/json",
                "Incomplete or invalid durable evidence asset; never replace it")
        body = self.asset(row["id"])
        require(len(body) == row["size"] and row.get("digest") == content_digest(body),
                "Durable evidence size/digest mismatch")
        return row, body


def release_identity(row, state):
    require(isinstance(row, dict) and positive_id(row.get("id"))
            and row.get("tag_name") == state["tag"]
            and row.get("target_commitish") == state["source_sha"]
            and row.get("prerelease") is False and type(row.get("draft")) is bool
            and type(row.get("immutable")) is bool, "Release identity/source mismatch")
    require((row["draft"] and row.get("published_at") is None and not row["immutable"])
            or (not row["draft"] and isinstance(row.get("published_at"), str)
                and bool(row["published_at"])), "Invalid release publication state")
    if "release_id" in state:
        require(row["id"] == state["release_id"], "Pinned release ID changed")
    return row


SNAPSHOT_FIELDS = ("tag", "version", "source_sha", "platform", "package_id", "digest", "pinned_image",
                   "original_workflow_sha", "workflow_sha", "run_url", "run_id", "run_attempt", "actor", "actor_id",
                   "triggering_actor", "creation_eligible", "package_observation", "prepush_package_observation")


def evidence_document(state, directory):
    snapshot = {key: state[key] for key in SNAPSHOT_FIELDS if key in state}
    return dict(schema_version=1, repository=SOURCE, release_id=state["release_id"],
                acceptance="passed", runtime="passed", snapshot=snapshot,
                provenance=read_json((directory / "verified-provenance.json").read_bytes()),
                identity=read_json((directory / "verified-identity.json").read_bytes()))


def validate_evidence(body, state, fresh=True):
    value = read_json(body)
    require(isinstance(value, dict) and type(value.get("schema_version")) is int
            and value["schema_version"] == 1 and value.get("repository") == SOURCE
            and set(value) == {"schema_version", "repository", "release_id", "acceptance", "runtime",
                               "snapshot", "provenance", "identity"}
            and positive_id(value.get("release_id")) and value["release_id"] == state["release_id"]
            and value.get("acceptance") == "passed" and value.get("runtime") == "passed",
            "Invalid durable acceptance evidence")
    snapshot = value.get("snapshot")
    require(isinstance(snapshot, dict) and set(snapshot) <= set(SNAPSHOT_FIELDS)
            and all(snapshot.get(key) == state[key] for key in ("tag", "version", "source_sha", "platform"))
            and positive_id(snapshot.get("package_id"))
            and isinstance(snapshot.get("digest"), str) and re.fullmatch(DIGEST, snapshot["digest"])
            and snapshot.get("pinned_image") == f"{IMAGE}@{snapshot['digest']}"
            and all(isinstance(snapshot.get(key), str) and re.fullmatch(SHA, snapshot[key])
                    for key in ("workflow_sha", "original_workflow_sha"))
            and isinstance(snapshot.get("run_id"), str) and re.fullmatch(r"[1-9][0-9]*", snapshot["run_id"])
            and snapshot.get("run_url") == f"{SOURCE}/actions/runs/{snapshot['run_id']}"
            and isinstance(snapshot.get("run_url"), str)
            and re.fullmatch(re.escape(SOURCE) + r"/actions/runs/[1-9][0-9]*", snapshot["run_url"])
            and isinstance(snapshot.get("run_attempt"), str)
            and re.fullmatch(r"[1-9][0-9]*", snapshot["run_attempt"])
            and all(isinstance(snapshot.get(key), str) and bool(snapshot[key])
                    for key in ("actor", "actor_id", "triggering_actor"))
            and type(snapshot.get("creation_eligible")) is bool
            and snapshot.get("package_observation") in ("accessible", "unavailable")
            and snapshot.get("prepush_package_observation", "accessible") in ("accessible", "unavailable"),
            "Durable evidence snapshot mismatch")
    for field, predicate in (("provenance", PROVENANCE), ("identity", PREDICATE)):
        require(verified_identity(value.get(field), snapshot, predicate) == snapshot["original_workflow_sha"],
                "Durable evidence original signer mismatch")
    if fresh:
        require(all(snapshot[key] == state[key] for key in
                    ("package_id", "digest", "pinned_image", "original_workflow_sha")),
                "Durable evidence conflicts with fresh image acceptance")
    return value


class Release:
    def __init__(self, workflow, source, record):
        self.workflow, self.source, self.record = Path(workflow).resolve(), Path(source).resolve(), Path(record)
        self.state = read_json(self.record.read_text()) if self.record.exists() else {
            "stage": "no_push_attempted", "push_attempted": False, "validation": "incomplete"}

    def save(self, **values):
        self.state.update(values)
        write_json(self.record, self.state)

    def resolve(self):
        self.save(**dispatch(os.environ))
        resolve(self.workflow, self.state)
        api = Releases(os.environ.get("GH_TOKEN", ""))
        rows = api.listing(RELEASES)
        matches = [row for row in rows if row.get("tag_name") == self.state["tag"]]
        require(len(matches) == 1, "Missing or duplicate release; never recreate it")
        row = release_identity(matches[0], self.state)
        if os.environ["GITHUB_EVENT_NAME"] == "push":
            require(row["id"] == int(os.environ["RELEASE_ID"]) and row["draft"],
                    "Automatic handoff must select the exact draft")
        self.save(release_id=row["id"], release_draft=row["draft"], immutable=row["immutable"])
        self.check_release()
        asset = api.evidence(row["id"])
        require(row["draft"] or asset is not None,
                "Published legacy release has no evidence; no backfill authorized")
        if asset is not None:
            validate_evidence(asset[1], self.state, fresh=False)
            self.pin_evidence(*asset)
        output(source_sha=self.state["source_sha"])

    def check_release(self):
        row = release_identity(Releases(os.environ.get("GH_TOKEN", "")).release(self.state["release_id"]), self.state)
        require(row["draft"] == self.state["release_draft"], "Release publication state changed externally")
        return row

    def pin_evidence(self, row, body):
        (self.record.parent / EVIDENCE).write_bytes(body)
        self.save(evidence_id=row["id"], evidence_size=len(body), evidence_digest=content_digest(body))

    def check_evidence(self):
        asset = Releases(os.environ.get("GH_TOKEN", "")).evidence(self.state["release_id"])
        require(asset is not None, "Required durable evidence missing")
        row, body = asset
        require(row["id"] == self.state["evidence_id"] and len(body) == self.state["evidence_size"]
                and content_digest(body) == self.state["evidence_digest"], "Pinned durable evidence changed")
        validate_evidence(body, self.state)
        return asset

    def publish(self):
        current = dispatch(os.environ)
        require(current == {k: self.state[k] for k in current}, "Dispatch identity changed")
        require(not self.state.get("push_attempted") and not self.state.get("digest"),
                "This record already selected or attempted publication; use a fresh recovery dispatch")
        self.check_release()
        self.save(operation="source_and_tool_checks")
        source_version(self.source, self.state)
        resolve(self.workflow, self.state)
        help_text = run("gh", "attestation", "verify", "--help")
        require(all(flag in help_text for flag in ("--bundle-from-oci", "--cert-identity", "--source-ref",
                                                  "--predicate-type", "--format")),
                "Installed gh lacks required attestation verification flags")
        self.save(gh_version=run("gh", "--version").splitlines()[0], operation="registry_authority")
        token = os.environ.get("GH_TOKEN", "")
        registry = Registry(token)
        # Keep login in the job's disposable file store, never an auto-detected OS helper.
        write_json(Path(os.environ["DOCKER_CONFIG"]) / "config.json", {"auths": {"ghcr.io": {}}})
        run("docker", "login", "ghcr.io", "--username", os.environ["GITHUB_ACTOR"], "--password-stdin", input=token)
        package_id, digest = registry.lookup(self.state["version"], self.state["creation_eligible"])
        self.save(package_id=package_id,
                  package_observation="unavailable" if package_id is None else "accessible")
        if digest:
            if self.state.get("evidence_id"):
                expected = read_json((self.record.parent / EVIDENCE).read_bytes())["snapshot"]
                require(expected["package_id"] == package_id and expected["digest"] == digest,
                        "Existing evidence conflicts with selected image")
            self.save(digest=digest, pinned_image=f"{IMAGE}@{digest}", reused=True, stage="digest_observed")
            output(new_image="false", digest=digest)
            return
        require(self.state["release_draft"] and not self.state.get("evidence_id"),
                "Published release or retained evidence requires an existing image; no build authorized")
        build = ["docker", "build", "--platform", "linux/amd64", "--file", str(self.source / "Containerfile"),
                 "--tag", self.state["image"]]
        for key, value in labels(self.state).items():
            build.extend(["--label", f"{key}={value}"])
        self.save(operation="local_build")
        run(*build, str(self.source))
        self.save(operation="local_runtime")
        image_id = inspect_image(self.state["image"], self.state)
        smoke(self.workflow, image_id)
        self.save(operation="prepush_recheck")
        self.check_release()
        remote_tag(self.workflow, self.state)
        before_id, before_digest = registry.lookup(self.state["version"], package_id is None)
        self.save(prepush_package_observation="unavailable" if before_id is None else "accessible")
        require(before_id == package_id and before_digest is None,
                "Package identity changed or tag appeared; restart through positive-access recovery")
        require(inspect_image(self.state["image"], self.state) == image_id, "Local image changed")
        # Persist BEFORE dispatch. Any error or termination after this point is uncertain.
        self.save(push_attempted=True, stage="push_attempted_outcome_uncertain", reused=False)
        pushed = run("docker", "push", self.state["image"])
        digests = re.findall(r"(?m)^\S+: digest: (sha256:[0-9a-f]{64}) size: [0-9]+\s*$", pushed)
        require(len(digests) == 1, "Push digest not observed; positive-access investigation required")
        digest = digests[0]
        self.save(digest=digest, pinned_image=f"{IMAGE}@{digest}", stage="digest_observed")
        self.save(operation="postpush_registry_check")
        after_id, after_digest = registry.lookup(self.state["version"])
        require(after_digest == digest and (package_id is None or after_id == package_id),
                "Post-push package identity/digest conflict")
        self.save(package_id=after_id)
        write_json(self.record.parent / "release-identity.json", identity(self.state, self.state["workflow_sha"]))
        output(new_image="true", digest=digest)

    def accept(self):
        require(bool(self.state.get("digest")), "No selected digest")
        self.save(operation="attestation_verification")
        signer_shas = []
        for predicate_type, filename in [(PROVENANCE, "verified-provenance.json"),
                                         (PREDICATE, "verified-identity.json")]:
            results = read_json(run("gh", "attestation", "verify", "oci://" + self.state["pinned_image"],
                                    "--repo", REPO, "--bundle-from-oci", "--cert-identity", WORKFLOW,
                                    "--source-ref", "refs/heads/main", "--predicate-type", predicate_type,
                                    "--format", "json"))
            signer_shas.append(verified_identity(results, self.state, predicate_type))
            write_json(self.record.parent / filename, results)
        require(signer_shas[0] == signer_shas[1], "Provenance and identity signer differ")
        require(self.state.get("reused") or signer_shas[0] == self.state["workflow_sha"],
                "New image signed by unexpected workflow commit")
        self.save(stage="identity_verified", original_workflow_sha=signer_shas[0])
        # Empty client configuration: no credential helper, no cached login, no fallback.
        self.save(operation="anonymous_pull")
        with tempfile.TemporaryDirectory(prefix="release-anonymous-") as config:
            # A blank auth entry also disables Docker's automatic OS credential-store detection.
            write_json(Path(config) / "config.json", {"auths": {"ghcr.io": {}}})
            run("docker", "--config", config, "pull", "--platform", "linux/amd64", self.state["pinned_image"])
        self.save(operation="public_runtime")
        image_id = inspect_image(self.state["pinned_image"], self.state)
        smoke(self.workflow, image_id)
        remote_tag(self.workflow, self.state)
        package_id, digest = Registry(os.environ.get("GH_TOKEN", "")).lookup(self.state["version"])
        require(package_id == self.state["package_id"] and digest == self.state["digest"],
                "Completion package identity/digest conflict")
        self.check_release()
        self.save(stage="public_runtime_acceptance_complete", validation="passed", operation="complete")

    def retain(self):
        require(self.state.get("validation") == "passed"
                and self.state.get("stage") == "public_runtime_acceptance_complete", "Image acceptance required")
        self.check_release()
        remote_tag(self.workflow, self.state)
        api = Releases(os.environ.get("GH_TOKEN", ""))
        asset = self.check_evidence() if self.state.get("evidence_id") else api.evidence(self.state["release_id"])
        if asset is None:
            require(self.state["release_draft"], "Published release evidence cannot be backfilled")
            require(not self.state.get("evidence_upload_attempted"),
                    "Evidence upload outcome uncertain; use fresh discovery without replacement")
            body = (json.dumps(evidence_document(self.state, self.record.parent), indent=2) + "\n").encode()
            require(len(body) <= MAX_EVIDENCE, "Durable evidence too large")
            validate_evidence(body, self.state)
            self.check_release()
            remote_tag(self.workflow, self.state)
            self.save(operation="evidence_upload", evidence_upload_attempted=True)
            api.json("POST", f"{RELEASES}/{self.state['release_id']}/assets?name={EVIDENCE}",
                     body, expected=201, host="uploads.github.com")
            asset = api.evidence(self.state["release_id"])
            require(asset is not None and asset[1] == body, "Uploaded evidence readback mismatch")
        validate_evidence(asset[1], self.state)
        if not self.state["release_draft"]:
            self.check_evidence()
        self.pin_evidence(*asset)
        self.save(evidence_retained=True, operation="evidence_retained")

    def finalize(self):
        require(self.state.get("validation") == "passed" and self.state.get("evidence_retained") is True
                and self.state.get("stage") == "public_runtime_acceptance_complete", "Acceptance/evidence required")
        self.check_release()
        remote_tag(self.workflow, self.state)
        package_id, digest = Registry(os.environ.get("GH_TOKEN", "")).lookup(self.state["version"])
        require(package_id == self.state["package_id"] and digest == self.state["digest"],
                "Publication package identity/digest conflict")
        self.check_evidence()
        if not self.state["release_draft"]:
            self.save(publication="already_published_read_only", operation="complete")
            return
        require(not self.state.get("publication_attempted"), "Publication outcome uncertain; use fresh discovery")
        self.check_release()
        remote_tag(self.workflow, self.state)
        self.save(publication_attempted=True, publication="outcome_uncertain", operation="github_publication")
        api = Releases(os.environ.get("GH_TOKEN", ""))
        row, _ = api.json("PATCH", f"{RELEASES}/{self.state['release_id']}", b'{"draft":false}')
        row = release_identity(row, self.state)
        require(not row["draft"], "Publication response remains a draft")
        observed = release_identity(api.release(self.state["release_id"]), self.state)
        require(not observed["draft"] and observed["immutable"] == row["immutable"], "Publication readback mismatch")
        remote_tag(self.workflow, self.state)
        self.save(release_draft=False, immutable=observed["immutable"], publication="published", operation="complete")

    def report(self):
        if os.environ.get("RELEASE_JOB_STATUS") != "success":
            self.save(validation="incomplete", next_action=(
                "Positive-access investigation only; never reopen creation after any possible push"
                if self.state.get("push_attempted") or self.state.get("digest") else
                "Inspect failure; recovery requires positive package access. If still absent, preserve proof "
                "of no push and request a narrowly reviewed creation adjustment; ordinary reruns cannot create"))
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a") as stream:
                stream.write("## Adapter release record\n\n```json\n" + self.record.read_text() + "```\n")
        print(self.record.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("resolve", "publish", "accept", "retain", "finalize", "report"))
    parser.add_argument("--workflow", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--record", required=True)
    args = parser.parse_args()
    release = Release(args.workflow, args.source, args.record)
    try:
        getattr(release, args.phase)()
    except (ReleaseError, OSError, KeyError, TypeError, ValueError) as error:
        # Never serialize raw exceptions or API bodies; fields may contain credentials.
        message = str(error) if isinstance(error, ReleaseError) else "Invalid response or local execution failure"
        release.save(error=message, validation="incomplete")
        print(message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
