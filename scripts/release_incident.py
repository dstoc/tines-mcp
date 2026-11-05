"""Offline checks for tines-mcp/40. No CLI, I/O, authorization or production caller.

Validated fixtures are not permission to recover. Enabling execution requires a
separate review, original-byte access and a permitted exact-draft publication path.
"""

import hashlib
import io
import re
import stat
import zipfile

from release import ReleaseError, SHA, read_json, require

SOURCE = "bf97287109eaab863d26ed82307d31db7c3c6a71"
REPO = "dstoc/tines-mcp"
RUN = 42010011054
JOB = 127186429900
WORKFLOW = 407725245
RELEASE = 435049346
ARTIFACT = 13406950249
ARCHIVE_SIZE = 819
ARCHIVE_DIGEST = "sha256:31c9fa47eb29b8dcfbef51358a45d206359fce40f21e8b5226177e1f14362356"
# Deliberately no executable opportunity. Only offline fixtures replace this pin.
REVIEWED_RUN_NUMBER = None
MAX_RECORD = 64 * 1024
V1_FIELDS = {"schema_version", "route", "http_status", "body_bytes_observed", "json_result", "json_type",
             "link_state", "json_object", "message_exact", "documentation_string", "documentation_prefix", "link_absent"}


def validate_snapshot(value):
    """Read historical diagnostics without upgrading or inventing lost predicates."""
    require(isinstance(value, dict), "Missing original metadata snapshot")
    version = value.get("schema_version")
    fields = V1_FIELDS | ({"message_string", "message_nonempty"} if version == 2 else set())
    require(type(version) is int and version in (1, 2) and set(value) == fields,
            "Unknown original metadata snapshot schema")
    require(all(value.get(k) == v and type(value.get(k)) is type(v) for k, v in dict(
        route="package_metadata", http_status=404, json_result="parsed", json_type="object",
        link_state="absent", json_object=True, message_exact=False, documentation_string=True,
        documentation_prefix=True, link_absent=True).items()), "Original predicate observations conflict")
    require(type(value.get("body_bytes_observed")) is int
            and 0 < value["body_bytes_observed"] <= 8 * 1024 * 1024, "Invalid original body size")
    if version == 2:
        require(all(type(value.get(k)) is bool for k in ("message_string", "message_nonempty"))
                and (value["message_string"] or not value["message_nonempty"]), "Invalid message observations")


def validate_original(run, attempts, job, metadata, archive):
    """Check supplied original bytes and API observations; never fetch alternatives."""
    expected = dict(id=RUN, workflow_id=WORKFLOW, run_number=18, run_attempt=1, head_sha=SOURCE,
                    event="push", head_branch="main", status="completed", conclusion="failure")
    require(isinstance(run, dict) and all(run.get(k) == v and type(run.get(k)) is type(v)
                                         for k, v in expected.items())
            and isinstance(run.get("repository"), dict) and run["repository"].get("full_name") == REPO,
            "Original run mismatch")
    # The caller must have completed normal authenticated attempt enumeration.
    require(isinstance(attempts, list) and len(attempts) == 1 and isinstance(attempts[0], dict)
            and attempts[0] == run and all(type(attempts[0].get(k)) is type(v) for k, v in expected.items()),
            "Original run has unknown or later attempts")
    require(isinstance(job, dict) and all(job.get(k) == v and type(job.get(k)) is type(v)
        for k, v in dict(id=JOB, run_id=RUN, run_attempt=1, head_sha=SOURCE,
                        status="completed", conclusion="failure").items()), "Original job mismatch")
    require(isinstance(metadata, dict) and type(metadata.get("id")) is int and metadata["id"] == ARTIFACT
            and metadata.get("name") == "adapter-release-diagnostics-42010011054-1"
            and metadata.get("expired") is False and type(metadata.get("size_in_bytes")) is int
            and metadata["size_in_bytes"] == ARCHIVE_SIZE and metadata.get("digest") == ARCHIVE_DIGEST,
            "Original artifact metadata mismatch")
    association = metadata.get("workflow_run")
    require(isinstance(association, dict) and all(association.get(k) == v and type(association.get(k)) is type(v)
        for k, v in dict(id=RUN, head_sha=SOURCE, head_branch="main", repository_id=1435671871,
                        head_repository_id=1435671871).items()), "Original artifact run mismatch")
    require(isinstance(archive, bytes) and len(archive) == ARCHIVE_SIZE
            and "sha256:" + hashlib.sha256(archive).hexdigest() == ARCHIVE_DIGEST,
            "Original archive bytes unavailable or changed")
    try:
        with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
            entries = zipped.infolist()
            require(1 <= len(entries) <= 8 and len({i.filename for i in entries}) == len(entries),
                    "Invalid original ZIP entry count or duplicates")
            require(sum(i.file_size for i in entries) <= MAX_RECORD, "Original ZIP decompression limit")
            for entry in entries:
                name = entry.filename
                require(name and not name.startswith("/") and "\\" not in name
                        and all(part not in ("", ".", "..") for part in name.rstrip("/").split("/"))
                        and not stat.S_ISLNK(entry.external_attr >> 16) and not entry.flag_bits & 1,
                        "Unsafe original ZIP entry")
            records = [i for i in entries if i.filename == "record.json"]
            require(len(records) == 1, "Missing original record.json")
            with zipped.open(records[0]) as stream:
                body = stream.read(MAX_RECORD + 1)
            require(len(body) <= MAX_RECORD, "Original record decompression limit")
    except (zipfile.BadZipFile, RuntimeError, OSError, NotImplementedError, EOFError):
        raise ReleaseError("Invalid original diagnostics archive") from None
    record = read_json(body)
    require(isinstance(record, dict) and all(record.get(k) == v and type(record.get(k)) is type(v)
        for k, v in dict(tag="v0.1.2", version="0.1.2", source_sha=SOURCE, workflow_sha=SOURCE,
                        release_id=RELEASE, run_id=str(RUN), run_attempt="1", push_attempted=False,
                        stage="no_push_attempted", validation="incomplete", operation="registry_authority").items())
        and record.get("run_url") == f"https://github.com/{REPO}/actions/runs/{RUN}"
        and not any(record.get(k) is not None for k in (
            "digest", "pinned_image", "original_workflow_sha", "package_id",
            "package_observation", "prepush_package_observation", "publication"))
        and record.get("creation_eligible") is True
        and record.get("release_draft") is True
        and not any(record.get(k) for k in ("publication_attempted", "evidence_upload_attempted", "evidence_retained", "reused")),
        "Original no-push record incomplete or contradictory")
    snapshots = record.get("package_responses")
    require(isinstance(snapshots, dict) and set(snapshots) == {"initial_authority"},
            "Original authority failure evidence missing or contradictory")
    validate_snapshot(snapshots["initial_authority"])
    # Return only fixed coordinates. Never retain arbitrary archive content.
    return dict(incident_run_id=RUN, artifact_id=ARTIFACT, archive_digest=ARCHIVE_DIGEST,
                release_id=RELEASE, source_sha=SOURCE)


def validate_opportunity(env, current, release):
    """Fixture-only identity check. No current run pin means no admitted opportunity."""
    require(type(REVIEWED_RUN_NUMBER) is int and REVIEWED_RUN_NUMBER > 18,
            "No reviewed incident run opportunity")
    require(isinstance(env, dict) and all(env.get(k) == v for k, v in dict(
        GITHUB_REPOSITORY=REPO, GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main",
        GITHUB_RUN_ATTEMPT="1", GITHUB_RUN_NUMBER=str(REVIEWED_RUN_NUMBER), RELEASE_INCIDENT=str(RUN),
        RELEASE_TAG="v0.1.2", RELEASE_SOURCE=SOURCE).items())
        and isinstance(env.get("GITHUB_SHA"), str) and re.fullmatch(SHA, env["GITHUB_SHA"])
        and isinstance(env.get("GITHUB_RUN_ID"), str)
        and re.fullmatch(r"[1-9][0-9]*", env["GITHUB_RUN_ID"])
        and env["GITHUB_RUN_ID"] != str(RUN), "Incident dispatch identity mismatch")
    require(isinstance(current, dict) and all(current.get(k) == v and type(current.get(k)) is type(v)
        for k, v in dict(id=int(env["GITHUB_RUN_ID"]), workflow_id=WORKFLOW,
                        run_number=REVIEWED_RUN_NUMBER, run_attempt=1, event="workflow_dispatch",
                        head_branch="main", head_sha=env["GITHUB_SHA"]).items())
        and isinstance(current.get("repository"), dict) and current["repository"].get("full_name") == REPO,
        "Executing run API identity mismatch")
    require(isinstance(release, dict) and type(release.get("id")) is int and release["id"] == RELEASE
            and release.get("tag_name") == "v0.1.2" and release.get("target_commitish") == SOURCE
            and release.get("draft") is True and release.get("prerelease") is False
            and release.get("immutable") is False and "published_at" in release and release["published_at"] is None
            and release.get("assets") == [], "Incident draft identity/state mismatch")


def validate_workflow_comparison(fixed, reviewed, default, *, workflow_sha, default_sha, current_default_sha):
    """Compare commit-bound complete path/blob maps, including deletions.

    Equal workflow trees are necessary for the retained-source GITHUB_TOKEN path;
    this check does not establish token authority or owner publication consent.
    """
    require(all(isinstance(sha, str) and re.fullmatch(SHA, sha)
                for sha in (workflow_sha, default_sha, current_default_sha))
            and default_sha == current_default_sha, "Stale or missing default workflow comparison")
    for observation, commit in ((fixed, SOURCE), (reviewed, workflow_sha), (default, default_sha)):
        require(isinstance(observation, dict) and observation.get("commit") == commit,
                "Workflow comparison commit mismatch or denied read")
        tree = observation.get("files")
        require(isinstance(tree, dict) and bool(tree) and all(
            isinstance(path, str) and path.startswith(".github/workflows/")
            and isinstance(blob, str) and re.fullmatch(SHA, blob) for path, blob in tree.items()),
            "Missing or denied complete workflow tree")
    require(fixed["files"] == default["files"] == reviewed["files"],
            "Retained-source GITHUB_TOKEN publication authority blocked")
