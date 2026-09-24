#!/usr/bin/env python3
"""Exercise session interoperability between native Codex and a CapEx binary."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlsplit

NATIVE_CODEX = Path(
    "/Users/cole/.local/share/mise/installs/node/22.22.0/lib/node_modules/"
    "@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/"
    "aarch64-apple-darwin/bin/codex"
)
NATIVE_VERSION = "codex-cli 0.156.1"
NATIVE_SHA256 = "0196e89fe5a7598f816ee54232c3d7c26d75e502ab5cfe2c9240e81d90f7255a"
AUTO_COMPACT_PROMPT = "You are performing a CONTEXT CHECKPOINT COMPACTION."

# This mirrors the closed RolloutItem tag set in codex-rs/history. The pinned
# native CLI must remain able to parse each session without a CapEx-only tag.
ROLLOUT_TYPES = frozenset(
    {
        "session_meta",
        "response_item",
        "inter_agent_communication",
        "inter_agent_communication_metadata",
        "compacted",
        "turn_context",
        "token_usage_record",
        "world_state",
        "security_risk_score",
        "retained_context",
        "event_msg",
        "realtime_item",
    }
)


class CompatibilityError(RuntimeError):
    """A compatibility check failed with a user-facing explanation."""


@contextmanager
def evidence_directory(keep_temp: bool):
    root = Path(tempfile.mkdtemp(prefix="capex-binary-compat-"))
    try:
        yield root
    except BaseException as error:
        manifest_path = root / "run-manifest.json"
        if manifest_path.is_file():
            try:
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                if isinstance(manifest, dict):
                    manifest["run_status"] = "failed"
                    manifest["failure"] = f"{type(error).__name__}: {error}"
                    write_manifest(manifest_path, manifest)
            except (OSError, json.JSONDecodeError, TypeError):
                pass
        if keep_temp:
            print(f"EVIDENCE_DIR={root}", file=sys.stderr)
        else:
            shutil.rmtree(root)
        raise
    else:
        if keep_temp:
            print(f"EVIDENCE_DIR={root}")
        else:
            shutil.rmtree(root)


@dataclass(frozen=True)
class ExpectedRequest:
    stage: str
    prompt_marker: str
    compaction: bool = False
    input_tokens: int = 1


@dataclass(frozen=True)
class CapturedRequest:
    stage: str
    body: dict[str, Any]


@dataclass(frozen=True)
class CliStage:
    label: str
    prompt_suffix: str
    binary: str
    resume_from: str | None
    create_as: str | None
    requests: tuple[tuple[str, bool, int, tuple[str, ...]], ...]
    capability_markers: tuple[str, ...] | None
    expected_grants: dict[str, str] | None
    fork_from: str | None = None
    fork_through: str | None = None


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def send_json(handler: BaseHTTPRequestHandler, status: int, payload: Any) -> None:
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    handler.send_response(status)
    handler.send_header("content-type", "application/json")
    handler.send_header("content-length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def sse_response(response_id: str, text: str, input_tokens: int) -> bytes:
    events = [
        {"type": "response.created", "response": {"id": response_id}},
        {
            "type": "response.output_item.done",
            "item": {
                "type": "message",
                "role": "assistant",
                "id": f"msg-{response_id}",
                "content": [{"type": "output_text", "text": text}],
            },
        },
        {
            "type": "response.completed",
            "response": {
                "id": response_id,
                "usage": {
                    "input_tokens": input_tokens,
                    "input_tokens_details": None,
                    "output_tokens": 1,
                    "output_tokens_details": None,
                    "total_tokens": input_tokens + 1,
                },
            },
        },
    ]
    return b"".join(
        f"event: {event['type']}\ndata: {json.dumps(event, separators=(',', ':'))}\n\n".encode()
        for event in events
    )


class ResponsesServer:
    """Small local Responses SSE server with ordered requests and body capture."""

    def __init__(
        self, expected: list[ExpectedRequest], prefix: str, evidence_dir: Path
    ) -> None:
        self.expected = expected
        self.prefix = prefix
        self.evidence_dir = evidence_dir
        self.requests: list[CapturedRequest] = []
        self.failures: list[str] = []
        self.lock = threading.Lock()
        self.httpd: ThreadingHTTPServer | None = None
        self.thread: threading.Thread | None = None

    def __enter__(self) -> ResponsesServer:  # noqa: PYI034 - keep Python 3.10 compatibility
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: object) -> None:
                return None

            def do_GET(self) -> None:
                request_path = urlsplit(self.path).path
                if request_path.endswith("/models"):
                    send_json(
                        self,
                        200,
                        {
                            "object": "list",
                            "data": [
                                {
                                    "id": "capex-compat-mock",
                                    "object": "model",
                                    "created": 0,
                                    "owned_by": "openai",
                                }
                            ],
                        },
                    )
                    return
                send_json(self, 404, {"error": f"unexpected GET {request_path}"})

            def do_POST(self) -> None:
                request_path = urlsplit(self.path).path
                if request_path.endswith("/analytics/codex/turn-costs"):
                    send_json(self, 404, {"error": "cost lookup disabled in harness"})
                    return
                if not request_path.endswith("/responses"):
                    send_json(self, 404, {"error": f"unexpected POST {request_path}"})
                    return

                try:
                    length = int(self.headers.get("content-length", "0"))
                    body = json.loads(self.rfile.read(length))
                    serialized = json.dumps(body, ensure_ascii=False)
                    with owner.lock:
                        index = len(owner.requests)
                        expected = (
                            owner.expected[index]
                            if index < len(owner.expected)
                            else None
                        )
                        capture_path = (
                            owner.evidence_dir / f"request-{index + 1:02d}.json"
                        )
                        capture_path.write_text(
                            json.dumps(
                                {
                                    "stage": expected.stage if expected else None,
                                    "compaction": (
                                        expected.compaction if expected else None
                                    ),
                                    "body": body,
                                },
                                ensure_ascii=False,
                                indent=2,
                            ),
                            encoding="utf-8",
                        )
                        if index >= len(owner.expected):
                            raise CompatibilityError(
                                f"unexpected extra model request #{index + 1}"
                            )
                        if (
                            not expected.compaction
                            and expected.prompt_marker not in serialized
                        ):
                            raise CompatibilityError(
                                f"request #{index + 1} was expected during "
                                f"{expected.stage}, but its body lacked the current prompt marker"
                            )
                        is_compaction = AUTO_COMPACT_PROMPT in serialized
                        if is_compaction != expected.compaction:
                            kind = "compaction" if is_compaction else "ordinary"
                            expected_kind = (
                                "compaction" if expected.compaction else "ordinary"
                            )
                            raise CompatibilityError(
                                f"request #{index + 1} for {expected.stage} was {kind}; "
                                f"expected {expected_kind}"
                            )
                        owner.requests.append(CapturedRequest(expected.stage, body))
                    response = sse_response(
                        f"compat-{index + 1}",
                        f"mock reply for {expected.stage}",
                        expected.input_tokens,
                    )
                    self.send_response(200)
                    self.send_header("content-type", "text/event-stream")
                    self.send_header("content-length", str(len(response)))
                    self.send_header("connection", "close")
                    self.end_headers()
                    self.wfile.write(response)
                    self.wfile.flush()
                    self.close_connection = True
                except (CompatibilityError, json.JSONDecodeError, ValueError) as error:
                    with owner.lock:
                        owner.failures.append(str(error))
                    send_json(self, 500, {"error": str(error)})

        try:
            self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        except OSError as error:
            raise CompatibilityError(
                f"could not start local mock server: {error}"
            ) from error
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_exc: object) -> None:
        if self.httpd is not None:
            self.httpd.shutdown()
            self.httpd.server_close()
        if self.thread is not None:
            self.thread.join(timeout=2)

    @property
    def url(self) -> str:
        if self.httpd is None:
            raise CompatibilityError("mock server is not running")
        host, port = self.httpd.server_address
        return f"http://{host}:{port}"

    def verify_complete(self) -> None:
        if self.failures:
            raise CompatibilityError("mock server failure: " + "; ".join(self.failures))
        if len(self.requests) != len(self.expected):
            missing = self.expected[len(self.requests) :]
            stages = ", ".join(item.stage for item in missing)
            raise CompatibilityError(
                f"mock server received {len(self.requests)} of {len(self.expected)} "
                f"expected model requests; missing: {stages}"
            )


def check_native_binary(path: Path) -> None:
    if not path.is_file() or not os.access(path, os.X_OK):
        raise CompatibilityError(
            f"native Codex executable is missing or not executable: {path}"
        )
    version = subprocess.run(
        [str(path), "--version"],
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    if version.returncode != 0 or version.stdout.strip() != NATIVE_VERSION:
        raise CompatibilityError(
            f"native binary version mismatch: expected {NATIVE_VERSION!r}, "
            f"got {version.stdout.strip()!r} (stderr: {version.stderr.strip()})"
        )
    actual_hash = sha256_file(path)
    if actual_hash != NATIVE_SHA256:
        raise CompatibilityError(
            f"native binary SHA-256 mismatch: expected {NATIVE_SHA256}, got {actual_hash}"
        )


def check_patched_binary(path: Path, native_path: Path) -> str:
    if not path.is_file() or not os.access(path, os.X_OK):
        raise CompatibilityError(
            f"patched Codex executable is missing or not executable: {path}"
        )
    if path.resolve() == native_path.resolve() or sha256_file(path) == NATIVE_SHA256:
        raise CompatibilityError(
            "patched binary resolves to the pinned native executable"
        )
    return sha256_file(path)


def child_environment(home: Path, capex_enabled: bool) -> dict[str, str]:
    environment = os.environ.copy()
    for key in (
        "CODEX_SQLITE_HOME",
        "CODEX_AUTHAPI_BASE_URL",
        "CODEX_ACCESS_TOKEN",
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "CAPEX_CAPABILITY_MODE",
        "CAPEX_INITIAL_CAPABILITIES",
        "CAPEX_ALWAYS_EXCLUDE_TAGS",
    ):
        environment.pop(key, None)
    environment["HOME"] = str(home.parent)
    environment["CODEX_HOME"] = str(home)
    if capex_enabled:
        environment["CAPEX_CAPABILITY_MODE"] = "1"
        environment["CAPEX_INITIAL_CAPABILITIES"] = "compat"
    return environment


def write_config(codex_home: Path, workspace: Path, mock_url: str) -> None:
    trusted_project = json.dumps(str(workspace))
    config = f'''model = "capex-compat-mock"
model_provider = "capex_compat"
model_context_window = 400000
model_auto_compact_token_limit = 200000
approval_policy = "never"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
analytics.enabled = false
check_for_update_on_startup = false

[features]
hooks = true

[model_providers.capex_compat]
name = "CapEx binary compatibility mock"
base_url = "{mock_url}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0

[projects.{trusted_project}]
trust_level = "trusted"
'''
    (codex_home / "config.toml").write_text(config, encoding="utf-8")


def write_capability_grant_hook(codex_home: Path, trigger_marker: str) -> None:
    script_path = codex_home / "capex_grant_hook.py"
    event_log = json.dumps(str(codex_home / "hook-events.jsonl"))
    trigger = json.dumps(trigger_marker)
    script = f"""import json
import sys

event = json.load(sys.stdin)
if {trigger} in event.get("prompt", ""):
    output = {{"capex": {{"grant": ["hooked"]}}}}
else:
    output = {{}}
with open({event_log}, "a", encoding="utf-8") as log:
    log.write(json.dumps({{"event": event, "output": output}}) + "\\n")
print(json.dumps(output))
"""
    script_path.write_text(script, encoding="utf-8")
    hooks = {
        "hooks": {
            "UserPromptSubmit": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": f"python3 {shlex.quote(str(script_path))}",
                        }
                    ]
                }
            ]
        }
    }
    (codex_home / "hooks.json").write_text(
        json.dumps(hooks, separators=(",", ":")), encoding="utf-8"
    )


def run_cli(
    binary: Path,
    environment: dict[str, str],
    workspace: Path,
    stage: str,
    prompt: str,
    timeout: int,
    server: ResponsesServer,
    bypass_hook_trust: bool,
    evidence_dir: Path,
    session_id: str | None = None,
) -> str:
    command = [str(binary), "exec"]
    if session_id is not None:
        command.append("resume")
        if bypass_hook_trust:
            command.append("--dangerously-bypass-hook-trust")
        command.extend(["--json", "--skip-git-repo-check", session_id, prompt])
    else:
        if bypass_hook_trust:
            command.append("--dangerously-bypass-hook-trust")
        command.extend(
            ["--json", "--skip-git-repo-check", "--sandbox", "read-only", prompt]
        )
    stage_slug = "".join(
        character.lower() if character.isalnum() else "_" for character in stage
    ).strip("_")
    try:
        result = subprocess.run(
            command,
            cwd=workspace,
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", errors="replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        (evidence_dir / f"{stage_slug}.stdout").write_text(stdout, encoding="utf-8")
        (evidence_dir / f"{stage_slug}.stderr").write_text(stderr, encoding="utf-8")
        raise CompatibilityError(
            f"{stage}: CLI timed out after {timeout}s\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ) from error
    (evidence_dir / f"{stage_slug}.stdout").write_text(result.stdout, encoding="utf-8")
    (evidence_dir / f"{stage_slug}.stderr").write_text(result.stderr, encoding="utf-8")
    if result.returncode != 0:
        mock_failures = (
            "\nmock server: " + "; ".join(server.failures) if server.failures else ""
        )
        raise CompatibilityError(
            f"{stage}: CLI exited {result.returncode}\n"
            f"command: {command!r}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            f"{mock_failures}"
        )

    events: list[dict[str, Any]] = []
    for line_number, line in enumerate(result.stdout.splitlines(), start=1):
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            raise CompatibilityError(
                f"{stage}: --json output line {line_number} is not JSON: {line!r}"
            ) from error
        if isinstance(event, dict):
            events.append(event)
    thread_events = [event for event in events if event.get("type") == "thread.started"]
    if not thread_events or not isinstance(thread_events[0].get("thread_id"), str):
        raise CompatibilityError(
            f"{stage}: CLI output did not include thread.started with a thread_id; "
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    actual_id = thread_events[0]["thread_id"]
    if session_id is not None and actual_id != session_id:
        raise CompatibilityError(
            f"{stage}: resumed {session_id}, but CLI reported thread {actual_id}"
        )
    return actual_id


def run_app_server_fork(
    binary: Path,
    environment: dict[str, str],
    workspace: Path,
    stage: str,
    evidence_dir: Path,
    parent_thread_id: str,
    last_turn_id: str,
    timeout: int,
) -> dict[str, Any]:
    """Create a cutoff fork over the app-server's stable thread/fork API."""
    stage_slug = "".join(
        character.lower() if character.isalnum() else "_" for character in stage
    ).strip("_")
    stdout_path = evidence_dir / f"{stage_slug}.app-server.stdout"
    stderr_path = evidence_dir / f"{stage_slug}.app-server.stderr"
    result_path = evidence_dir / f"{stage_slug}.app-server.result.json"
    messages: queue.Queue[dict[str, Any] | None] = queue.Queue()
    process: subprocess.Popen[str] | None = None
    reader: threading.Thread | None = None
    result: dict[str, Any] | None = None
    failure: Exception | None = None

    stdout_capture = stdout_path.open("w", encoding="utf-8")
    stderr_capture = stderr_path.open("w", encoding="utf-8")
    try:
        process = subprocess.Popen(
            [str(binary), "app-server", "--listen", "stdio://"],
            cwd=workspace,
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr_capture,
            text=True,
            bufsize=1,
        )
        assert process.stdin is not None and process.stdout is not None

        def pump_messages() -> None:
            assert process is not None and process.stdout is not None
            for line in process.stdout:
                stdout_capture.write(line)
                stdout_capture.flush()
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    message = {"_unparsed_line": line.rstrip("\n")}
                if isinstance(message, dict):
                    messages.put(message)
            messages.put(None)

        reader = threading.Thread(target=pump_messages, daemon=True)
        reader.start()

        def request(
            request_id: int, method: str, params: dict[str, Any]
        ) -> dict[str, Any]:
            assert process is not None and process.stdin is not None
            process.stdin.write(
                json.dumps(
                    {"id": request_id, "method": method, "params": params},
                    separators=(",", ":"),
                )
                + "\n"
            )
            process.stdin.flush()
            deadline = time.monotonic() + timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise CompatibilityError(
                        f"{stage}: timed out waiting for app-server {method} response"
                    )
                try:
                    message = messages.get(timeout=remaining)
                except queue.Empty as error:
                    raise CompatibilityError(
                        f"{stage}: timed out waiting for app-server {method} response"
                    ) from error
                if message is None:
                    raise CompatibilityError(
                        f"{stage}: app-server exited before {method} response"
                    )
                if message.get("id") != request_id:
                    continue
                if "error" in message:
                    raise CompatibilityError(
                        f"{stage}: app-server {method} failed: {message['error']}"
                    )
                response_result = message.get("result")
                if not isinstance(response_result, dict):
                    raise CompatibilityError(
                        f"{stage}: app-server {method} returned no result object"
                    )
                return response_result

        request(
            1,
            "initialize",
            {"clientInfo": {"name": "capex-binary-compat", "version": "1"}},
        )
        process.stdin.write('{"method":"initialized"}\n')
        process.stdin.flush()
        result = request(
            2,
            "thread/fork",
            {
                "threadId": parent_thread_id,
                "lastTurnId": last_turn_id,
                "excludeTurns": True,
            },
        )
        thread = result.get("thread")
        if not isinstance(thread, dict) or not isinstance(thread.get("id"), str):
            raise CompatibilityError(f"{stage}: thread/fork response omitted thread.id")
        result_path.write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
    except (CompatibilityError, OSError, subprocess.SubprocessError) as error:
        failure = error
    finally:
        if process is not None:
            if process.poll() is None:
                try:
                    process.send_signal(signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            if process.stdin is not None:
                process.stdin.close()
        if reader is not None:
            reader.join(timeout=2)
        if process is not None and process.stdout is not None:
            process.stdout.close()
        stdout_capture.close()
        stderr_capture.close()

    if failure is not None:
        stdout = stdout_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        stderr = stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        raise CompatibilityError(
            f"{failure}\napp-server stdout (tail):\n{stdout}\n"
            f"app-server stderr (tail):\n{stderr}"
        ) from failure
    if result is None:
        raise CompatibilityError(f"{stage}: app-server fork returned no result")
    return result


def rollout_session_meta(records: list[dict[str, Any]], stage: str) -> dict[str, Any]:
    for record in records:
        if record.get("type") == "session_meta":
            payload = record.get("payload")
            if isinstance(payload, dict):
                return payload
    raise CompatibilityError(f"{stage}: rollout has no session_meta record")


def turn_id_for_user_prompt(
    records: list[dict[str, Any]], prompt_marker: str, stage: str
) -> tuple[str, int]:
    matching_turn_ids: set[str] = set()
    for record in records:
        payload = record.get("payload")
        if (
            record.get("type") != "response_item"
            or not isinstance(payload, dict)
            or payload.get("type") != "message"
            or payload.get("role") != "user"
            or prompt_marker
            not in json.dumps(payload.get("content", []), ensure_ascii=False)
        ):
            continue
        metadata = payload.get("internal_chat_message_metadata_passthrough")
        if isinstance(metadata, dict) and isinstance(metadata.get("turn_id"), str):
            matching_turn_ids.add(metadata["turn_id"])
    if len(matching_turn_ids) != 1:
        raise CompatibilityError(
            f"{stage}: expected one persisted user turn containing {prompt_marker!r}, "
            f"found {len(matching_turn_ids)}"
        )
    turn_id = next(iter(matching_turn_ids))
    for record in records:
        payload = record.get("payload")
        if (
            record.get("type") == "event_msg"
            and isinstance(payload, dict)
            and payload.get("type") in ("task_complete", "turn_complete")
            and payload.get("turn_id") == turn_id
        ):
            ordinal = record.get("ordinal")
            if isinstance(ordinal, int) and ordinal >= 0:
                return turn_id, ordinal
    raise CompatibilityError(
        f"{stage}: turn {turn_id} has no persisted completion ordinal"
    )


def validate_copied_fork(
    records: list[dict[str, Any]],
    parent_thread_id: str,
    child_thread_id: str,
    last_turn_id: str,
    end_ordinal: int,
    required_markers: tuple[str, ...],
    excluded_markers: tuple[str, ...],
) -> dict[str, Any]:
    metadata = rollout_session_meta(records, "self-contained copied fork child")
    history_base = metadata.get("history_base")
    if metadata.get("id") != child_thread_id:
        raise CompatibilityError(
            "fork child session_meta.id does not match its thread ID"
        )
    if metadata.get("forked_from_id") != parent_thread_id:
        raise CompatibilityError(
            "fork child session_meta.forked_from_id does not identify the source thread"
        )
    if history_base is not None:
        raise CompatibilityError(
            "fork child is not self-contained: session_meta.history_base is present"
        )
    if metadata.get("forked_from_ordinal_exclusive") != end_ordinal + 1:
        raise CompatibilityError(
            "fork child session_meta.forked_from_ordinal_exclusive does not end "
            "immediately after its selected turn"
        )
    child_text = json.dumps(records, ensure_ascii=False)
    for marker in required_markers:
        if marker not in child_text:
            raise CompatibilityError(
                f"self-contained fork child omitted pre-cutoff content marker {marker!r}"
            )
    for marker in excluded_markers:
        if marker in child_text:
            raise CompatibilityError(
                f"self-contained fork child contains post-cutoff content marker {marker!r}"
            )
    return {
        "parent_thread_id": parent_thread_id,
        "child_thread_id": child_thread_id,
        "last_turn_id": last_turn_id,
        "forked_from_ordinal_exclusive": end_ordinal + 1,
        "history_base": None,
        "persistence": "self-contained-copied",
        "required_prefix_markers": list(required_markers),
        "excluded_post_cutoff_markers": list(excluded_markers),
    }


def find_rollout(codex_home: Path, thread_id: str) -> tuple[Path, list[dict[str, Any]]]:
    sessions = codex_home / "sessions"
    if not sessions.is_dir():
        raise CompatibilityError(f"session directory was not created: {sessions}")
    for path in sessions.rglob("*.jsonl"):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        if not lines:
            continue
        try:
            header = json.loads(lines[0])
        except json.JSONDecodeError:
            continue
        payload = header.get("payload") if isinstance(header, dict) else None
        if (
            isinstance(header, dict)
            and header.get("type") == "session_meta"
            and isinstance(payload, dict)
            and payload.get("id") == thread_id
        ):
            records = []
            for index, line in enumerate(lines, start=1):
                if not line.strip():
                    continue
                try:
                    record = json.loads(line)
                except json.JSONDecodeError as error:
                    raise CompatibilityError(
                        f"rollout {path} line {index} is invalid JSON: {error}"
                    ) from error
                validate_rollout_record(path, index, record)
                records.append(record)
            return path, records
    raise CompatibilityError(f"no JSONL rollout found for thread {thread_id}")


def validate_rollout_record(path: Path, line_number: int, record: Any) -> None:
    if not isinstance(record, dict):
        raise CompatibilityError(
            f"rollout {path} line {line_number} is not a JSON object"
        )
    record_type = record.get("type")
    if not isinstance(record_type, str) or record_type not in ROLLOUT_TYPES:
        raise CompatibilityError(
            f"rollout {path} line {line_number} has unknown native record type {record_type!r}"
        )
    unexpected = set(record) - {
        "timestamp",
        "ordinal",
        "type",
        "payload",
        "metadata",
    }
    if unexpected:
        raise CompatibilityError(
            f"rollout {path} line {line_number} has unexpected top-level keys: "
            f"{sorted(unexpected)}"
        )
    if not isinstance(record.get("timestamp"), str):
        raise CompatibilityError(
            f"rollout {path} line {line_number} has no string timestamp"
        )
    ordinal = record.get("ordinal")
    if ordinal is not None and (
        isinstance(ordinal, bool) or not isinstance(ordinal, int) or ordinal < 0
    ):
        raise CompatibilityError(
            f"rollout {path} line {line_number} has an invalid ordinal {ordinal!r}"
        )
    if not isinstance(record.get("payload"), dict):
        raise CompatibilityError(
            f"rollout {path} line {line_number} has no object payload"
        )
    if "metadata" in record and record_type != "response_item":
        raise CompatibilityError(
            f"rollout {path} line {line_number} attaches metadata to {record_type}"
        )


def verify_shared_index(codex_home: Path, thread_id: str, rollout_path: Path) -> None:
    db_path = codex_home / "state_5.sqlite"
    if not db_path.is_file():
        raise CompatibilityError(
            f"shared state index database was not created: {db_path}"
        )
    uri = f"file:{quote(str(db_path.resolve()), safe='/')}?mode=ro"
    database: sqlite3.Connection | None = None
    try:
        database = sqlite3.connect(uri, uri=True, timeout=5)
        row = database.execute(
            "SELECT rollout_path FROM threads WHERE id = ?", (thread_id,)
        ).fetchone()
    except sqlite3.Error as error:
        raise CompatibilityError(
            f"could not read shared thread index {db_path}: {error}"
        ) from error
    finally:
        if database is not None:
            database.close()
    if row is None or not isinstance(row[0], str):
        raise CompatibilityError(
            f"thread {thread_id} is missing from shared SQLite index"
        )
    indexed_path = Path(row[0])
    if not indexed_path.is_absolute():
        indexed_path = codex_home / indexed_path
    if indexed_path.resolve() != rollout_path.resolve():
        raise CompatibilityError(
            f"SQLite index points thread {thread_id} at {indexed_path}, "
            f"but the rollout is {rollout_path}"
        )


def verify_capability_sidecar(
    codex_home: Path, thread_id: str, expected_grants: dict[str, str]
) -> tuple[Path, str]:
    sidecar = codex_home / "capex" / f"{thread_id}.json"
    if not sidecar.is_file():
        raise CompatibilityError(
            f"patched binary did not create its sidecar: {sidecar}"
        )
    try:
        contents = json.loads(sidecar.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CompatibilityError(
            f"could not read capability sidecar {sidecar}: {error}"
        ) from error
    if not isinstance(contents, dict):
        raise CompatibilityError(f"sidecar root is not an object: {sidecar}")
    grants = contents.get("grants")
    if contents.get("version") != 1 or not isinstance(grants, list):
        raise CompatibilityError(f"sidecar has an unsupported schema: {sidecar}")
    actual = {
        grant.get("id"): str(grant.get("instructions", ""))
        for grant in grants
        if isinstance(grant, dict) and isinstance(grant.get("id"), str)
    }
    if set(actual) != set(expected_grants):
        raise CompatibilityError(
            f"{sidecar}: expected grants {sorted(expected_grants)}, found {sorted(actual)}"
        )
    for grant_id, marker in expected_grants.items():
        if marker not in actual[grant_id]:
            raise CompatibilityError(
                f"{sidecar}: {grant_id} instructions do not contain the expected marker"
            )
    return sidecar, sha256_file(sidecar)


def assert_capability_context(
    request: CapturedRequest,
    markers: tuple[str, ...],
    all_markers: tuple[str, ...],
    stage: str,
) -> None:
    developer_messages = []
    inputs = request.body.get("input")
    if isinstance(inputs, list):
        for item in inputs:
            if not isinstance(item, dict) or item.get("role") != "developer":
                continue
            content = item.get("content")
            if isinstance(content, str):
                text = content
            elif isinstance(content, list):
                text = "\n".join(
                    part["text"]
                    for part in content
                    if isinstance(part, dict) and isinstance(part.get("text"), str)
                )
            else:
                text = ""
            developer_messages.append(text)

    for marker in all_markers:
        matching_blocks = [text for text in developer_messages if marker in text]
        expected_count = 1 if marker in markers else 0
        actual_count = sum(text.count(marker) for text in matching_blocks)
        if len(matching_blocks) != expected_count or actual_count != expected_count:
            raise CompatibilityError(
                f"{stage}: expected exactly {expected_count} active developer input block(s) "
                f"for capability marker {marker!r}; found {len(matching_blocks)} blocks "
                f"with {actual_count} marker occurrence(s)"
            )


def effective_rollout_records(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Return the latest compacted replacement history plus records appended after it."""
    compacted_index = next(
        (
            index
            for index in range(len(records) - 1, -1, -1)
            if records[index].get("type") == "compacted"
        ),
        None,
    )
    if compacted_index is None:
        return records
    payload = records[compacted_index].get("payload")
    replacement_history = (
        payload.get("replacement_history") if isinstance(payload, dict) else None
    )
    if not isinstance(replacement_history, list) or not all(
        isinstance(item, dict) for item in replacement_history
    ):
        raise CompatibilityError(
            "latest compacted rollout record has no valid replacement_history array"
        )
    compacted_items = [
        {"type": "response_item", "payload": item} for item in replacement_history
    ]
    return compacted_items + records[compacted_index + 1 :]


def count_capability_items(records: list[dict[str, Any]], marker: str) -> int:
    return sum(
        1
        for record in effective_rollout_records(records)
        if record.get("type") == "response_item"
        and record.get("payload", {}).get("type") == "message"
        and record.get("payload", {}).get("role") == "developer"
        and marker in json.dumps(record.get("payload", {}), ensure_ascii=False)
    )


def run_compatibility(
    native: Path,
    patched: Path,
    timeout: int,
    keep_temp: bool,
    patched_hash: str,
    patched_revision: str | None,
) -> None:
    prefix = f"CAPEX_BINARY_COMPAT_{uuid.uuid4().hex}"
    capability_marker = f"{prefix}:initial-capability-instructions"
    hook_marker = f"{prefix}:hook-granted-capability-instructions"
    all_capability_markers = (capability_marker, hook_marker)
    initial_grants = {"compat": capability_marker}
    hook_grants = {**initial_grants, "hooked": hook_marker}
    # A response over the configured 200k threshold arms compaction for the
    # next user turn. The compact response deliberately omits the capability
    # marker so the following request proves the patched CLI restores it.
    stages = [
        CliStage(
            "native creates session",
            "native-create",
            "native",
            None,
            "first",
            (("native-create", False, 70_000, ()),),
            (),
            None,
        ),
        CliStage(
            "CapEx resumes native session with initial guidance",
            "patched-capability-prime",
            "patched",
            "first",
            None,
            (("patched-capability-prime", False, 70_000, (capability_marker,)),),
            (capability_marker,),
            initial_grants,
        ),
        CliStage(
            "CapEx resumes native session and hook-grants guidance",
            "patched-high-on-native",
            "patched",
            "first",
            None,
            (
                (
                    "patched-high-on-native",
                    False,
                    330_000,
                    (capability_marker, hook_marker),
                ),
            ),
            (capability_marker, hook_marker),
            hook_grants,
        ),
        CliStage(
            "native cold-resumes self-contained copied fork at grant cutoff",
            "native-fork-child",
            "native",
            "fork-child",
            None,
            (("native-fork-child", False, 120, ()),),
            (),
            None,
            fork_from="first",
            fork_through="patched-capability-prime",
        ),
        CliStage(
            "CapEx resumes native-written copied fork child",
            "fork-child-capex-resume",
            "patched",
            "fork-child",
            None,
            (
                (
                    "fork-child-capex-resume",
                    False,
                    120,
                    (capability_marker,),
                ),
            ),
            (capability_marker,),
            initial_grants,
        ),
        CliStage(
            "native compacts CapEx-written session and sends follow-up",
            "native-trigger-compaction",
            "native",
            "first",
            None,
            (
                (
                    "native-auto-compact",
                    True,
                    200,
                    (capability_marker, hook_marker),
                ),
                ("native-post-compaction-follow-up", False, 120, ()),
            ),
            None,
            None,
        ),
        CliStage(
            "CapEx reopens native-compacted session",
            "patched-reopen-native-compaction",
            "patched",
            "first",
            None,
            (
                (
                    "patched-reopen-native-compaction",
                    False,
                    120,
                    (capability_marker, hook_marker),
                ),
            ),
            (capability_marker, hook_marker),
            hook_grants,
        ),
        CliStage(
            "CapEx creates session",
            "patched-create",
            "patched",
            None,
            "second",
            (("patched-create", False, 70_000, (capability_marker,)),),
            (capability_marker,),
            initial_grants,
        ),
        CliStage(
            "native resumes CapEx session above compaction threshold",
            "native-high-on-capex",
            "native",
            "second",
            None,
            (("native-high-on-capex", False, 330_000, (capability_marker,)),),
            (capability_marker,),
            None,
        ),
        CliStage(
            "CapEx compacts native-written session and sends follow-up",
            "patched-trigger-compaction",
            "patched",
            "second",
            None,
            (
                ("patched-auto-compact", True, 200, (capability_marker,)),
                (
                    "patched-post-compaction-follow-up",
                    False,
                    120,
                    (capability_marker,),
                ),
            ),
            (capability_marker,),
            initial_grants,
        ),
        CliStage(
            "native reopens CapEx-compacted session",
            "native-reopen-capex-compaction",
            "native",
            "second",
            None,
            (
                (
                    "native-reopen-capex-compaction",
                    False,
                    120,
                    (capability_marker,),
                ),
            ),
            (capability_marker,),
            None,
        ),
    ]
    request_specs = [
        ExpectedRequest(name, f"{prefix}:{stage.prompt_suffix}", compact, tokens)
        for stage in stages
        for name, compact, tokens, _markers in stage.requests
    ]

    with evidence_directory(keep_temp) as root:
        evidence_dir = root / "process"
        evidence_dir.mkdir()
        request_evidence_dir = root / "requests"
        request_evidence_dir.mkdir()
        codex_home = root / "codex-home"
        workspace = root / "workspace"
        manifest_path = root / "run-manifest.json"
        manifest: dict[str, Any] = {
            "run_status": "running",
            "current_stage": None,
            "native": {
                "path": str(native.resolve()),
                "version": NATIVE_VERSION,
                "sha256": NATIVE_SHA256,
            },
            "patched": {
                "path": str(patched.resolve()),
                "sha256": patched_hash,
                "build_revision": patched_revision,
            },
            "completed_stages": [],
            "forks": [],
        }
        write_manifest(manifest_path, manifest)
        codex_home.mkdir()
        workspace.mkdir()
        subprocess.run(
            ["git", "init", "--quiet"],
            cwd=workspace,
            check=True,
            capture_output=True,
            text=True,
        )
        capability_dir = workspace / ".agents" / "capabilities" / "compat"
        capability_dir.mkdir(parents=True)
        (capability_dir / "CAPABILITY.md").write_text(
            f"---\ntags: [compatibility]\n---\n{capability_marker}\n",
            encoding="utf-8",
        )
        hook_capability_dir = workspace / ".agents" / "capabilities" / "hooked"
        hook_capability_dir.mkdir(parents=True)
        (hook_capability_dir / "CAPABILITY.md").write_text(
            f"---\ntags: [compatibility]\n---\n{hook_marker}\n",
            encoding="utf-8",
        )
        write_capability_grant_hook(codex_home, f"{prefix}:patched-high-on-native")

        with ResponsesServer(request_specs, prefix, request_evidence_dir) as server:
            write_config(codex_home, workspace, server.url)
            native_environment = child_environment(codex_home, capex_enabled=False)
            patched_environment = child_environment(codex_home, capex_enabled=True)
            thread_ids: dict[str, str] = {}
            sidecar_hashes: dict[str, str] = {}
            sidecar_grants: dict[str, dict[str, str]] = {}
            turn_ids_by_prompt: dict[str, tuple[str, int]] = {}

            for step in stages:
                stage = step.label
                manifest["current_stage"] = stage
                write_manifest(manifest_path, manifest)
                prompt_marker = f"{prefix}:{step.prompt_suffix}"
                prompt = (
                    f"Reply with a short confirmation. Test marker: {prompt_marker}"
                )
                fork_details = None
                fork_manifest_entry = None
                if step.fork_from is not None:
                    if step.fork_through is None:
                        raise CompatibilityError(
                            f"{stage}: fork cutoff turn is unspecified"
                        )
                    parent_thread_id = thread_ids[step.fork_from]
                    last_turn_id, cutoff_ordinal = turn_ids_by_prompt[step.fork_through]
                    fork_result = run_app_server_fork(
                        patched,
                        patched_environment,
                        workspace,
                        stage,
                        evidence_dir,
                        parent_thread_id,
                        last_turn_id,
                        timeout,
                    )
                    forked_thread = fork_result.get("thread")
                    if not isinstance(forked_thread, dict) or not isinstance(
                        forked_thread.get("id"), str
                    ):
                        raise CompatibilityError(
                            f"{stage}: app-server fork response omitted thread.id"
                        )
                    fork_child_id = forked_thread["id"]
                    fork_child_path, fork_child_records = find_rollout(
                        codex_home, fork_child_id
                    )
                    verify_shared_index(codex_home, fork_child_id, fork_child_path)
                    fork_details = validate_copied_fork(
                        fork_child_records,
                        parent_thread_id,
                        fork_child_id,
                        last_turn_id,
                        cutoff_ordinal,
                        (
                            f"{prefix}:native-create",
                            f"{prefix}:patched-capability-prime",
                        ),
                        (
                            f"{prefix}:patched-high-on-native",
                            capability_marker,
                            hook_marker,
                        ),
                    )
                    verify_capability_sidecar(codex_home, fork_child_id, initial_grants)
                    sidecar_path = codex_home / "capex" / f"{fork_child_id}.json"
                    sidecar_hashes[fork_child_id] = sha256_file(sidecar_path)
                    sidecar_grants[fork_child_id] = initial_grants
                    thread_ids["fork-child"] = fork_child_id
                    fork_manifest_entry = {
                        **fork_details,
                        "stage": stage,
                        "status": "created",
                        "rollout_path": str(fork_child_path),
                        "capability_grants": sorted(initial_grants),
                    }
                    manifest["forks"].append(fork_manifest_entry)
                    write_manifest(manifest_path, manifest)
                session_id = (
                    None if step.resume_from is None else thread_ids[step.resume_from]
                )
                capex_enabled = step.binary == "patched"
                binary = patched if capex_enabled else native
                environment = (
                    patched_environment if capex_enabled else native_environment
                )
                request_start = len(server.requests)
                thread_id = run_cli(
                    binary,
                    environment,
                    workspace,
                    stage,
                    prompt,
                    timeout,
                    server,
                    capex_enabled,
                    evidence_dir,
                    session_id,
                )
                if step.create_as is not None:
                    thread_ids[step.create_as] = thread_id
                elif thread_id != session_id:
                    raise CompatibilityError(
                        f"{stage}: session UUID changed while resuming"
                    )

                requests = server.requests[request_start:]
                if len(requests) != len(step.requests):
                    raise CompatibilityError(
                        f"{stage}: expected {len(step.requests)} outbound request(s), "
                        f"captured {len(requests)}"
                    )
                if tuple(request.stage for request in requests) != tuple(
                    spec[0] for spec in step.requests
                ):
                    raise CompatibilityError(
                        f"{stage}: captured outbound requests in unexpected order: "
                        f"{[request.stage for request in requests]}"
                    )
                for request, spec in zip(requests, step.requests, strict=True):
                    assert_capability_context(
                        request,
                        spec[3],
                        all_capability_markers,
                        f"{stage} / {request.stage}",
                    )
                expect_compaction = any(spec[1] for spec in step.requests)
                if expect_compaction:
                    compact_text = json.dumps(requests[0].body, ensure_ascii=False)
                    follow_up_text = json.dumps(requests[1].body, ensure_ascii=False)
                    if AUTO_COMPACT_PROMPT not in compact_text:
                        raise CompatibilityError(
                            f"{stage}: first captured request was not the compaction request"
                        )
                    if AUTO_COMPACT_PROMPT in follow_up_text:
                        raise CompatibilityError(
                            f"{stage}: immediate post-compaction request repeated the compaction prompt"
                        )

                rollout_path, records = find_rollout(codex_home, thread_id)
                verify_shared_index(codex_home, thread_id, rollout_path)
                if step.prompt_suffix == "patched-capability-prime":
                    turn_ids_by_prompt[step.prompt_suffix] = turn_id_for_user_prompt(
                        records, prompt_marker, stage
                    )
                if step.capability_markers is not None:
                    for marker in all_capability_markers:
                        actual_count = count_capability_items(records, marker)
                        expected_count = int(marker in step.capability_markers)
                        if actual_count != expected_count:
                            raise CompatibilityError(
                                f"{stage}: expected {expected_count} effective ordinary "
                                f"developer response item(s) for {marker!r}, found {actual_count}"
                            )
                if expect_compaction and not any(
                    record.get("type") == "compacted" for record in records
                ):
                    raise CompatibilityError(
                        f"{stage}: outbound compaction succeeded but no native compacted record was persisted"
                    )

                if capex_enabled:
                    sidecar, digest = verify_capability_sidecar(
                        codex_home,
                        thread_id,
                        step.expected_grants or {},
                    )
                    previous_digest = sidecar_hashes.get(thread_id)
                    if previous_digest is None:
                        sidecar_hashes[thread_id] = digest
                        sidecar_grants[thread_id] = step.expected_grants or {}
                    elif digest != previous_digest and (
                        step.expected_grants or {}
                    ) == sidecar_grants.get(thread_id):
                        raise CompatibilityError(
                            f"{stage}: a resumed capability sidecar changed unexpectedly: {sidecar}"
                        )
                    else:
                        sidecar_hashes[thread_id] = digest
                        sidecar_grants[thread_id] = step.expected_grants or {}
                elif thread_id in sidecar_hashes:
                    sidecar = codex_home / "capex" / f"{thread_id}.json"
                    if (
                        not sidecar.is_file()
                        or sha256_file(sidecar) != sidecar_hashes[thread_id]
                    ):
                        raise CompatibilityError(
                            f"{stage}: native Codex changed or removed the CapEx sidecar"
                        )

                if fork_details is not None:
                    request_text = json.dumps(requests[0].body, ensure_ascii=False)
                    for inherited_marker in (
                        f"{prefix}:native-create",
                        f"{prefix}:patched-capability-prime",
                        prompt_marker,
                    ):
                        if inherited_marker not in request_text:
                            raise CompatibilityError(
                                f"{stage}: fork-child request omitted expected active history "
                                f"marker {inherited_marker!r}"
                            )
                    excluded_marker = f"{prefix}:patched-high-on-native"
                    if excluded_marker in request_text:
                        raise CompatibilityError(
                            f"{stage}: fork-child request contains post-cutoff marker "
                            f"{excluded_marker!r}"
                        )
                    child_rollout_text = json.dumps(records, ensure_ascii=False)
                    if prompt_marker not in child_rollout_text:
                        raise CompatibilityError(
                            f"{stage}: child JSONL did not persist its native continuation"
                        )
                    validate_copied_fork(
                        records,
                        fork_details["parent_thread_id"],
                        thread_id,
                        fork_details["last_turn_id"],
                        fork_details["forked_from_ordinal_exclusive"] - 1,
                        tuple(fork_details["required_prefix_markers"]),
                        tuple(fork_details["excluded_post_cutoff_markers"]),
                    )
                    if hook_marker in request_text or capability_marker in request_text:
                        raise CompatibilityError(
                            f"{stage}: native fork continuation contains CapEx capability instructions"
                        )
                    fork_manifest_entry["native_resume_status"] = "passed"
                    fork_manifest_entry["native_resume_stage"] = stage

                if step.binary == "patched" and step.resume_from == "fork-child":
                    fork_manifest_entry = next(
                        (
                            fork
                            for fork in reversed(manifest["forks"])
                            if fork.get("child_thread_id") == thread_id
                        ),
                        None,
                    )
                    if fork_manifest_entry is None:
                        raise CompatibilityError(
                            f"{stage}: no fork manifest entry for the resumed child"
                        )
                    if fork_manifest_entry.get("native_resume_status") != "passed":
                        raise CompatibilityError(
                            f"{stage}: native cold-resume of the fork child was not recorded"
                        )
                    inherited_markers = tuple(
                        fork_manifest_entry["required_prefix_markers"]
                    ) + (f"{prefix}:native-fork-child", prompt_marker)
                    request_text = json.dumps(requests[0].body, ensure_ascii=False)
                    for marker in inherited_markers:
                        if marker not in request_text:
                            raise CompatibilityError(
                                f"{stage}: CapEx fork-child request omitted retained context "
                                f"marker {marker!r}"
                            )
                    for marker in (
                        f"{prefix}:patched-high-on-native",
                        hook_marker,
                    ):
                        if marker in request_text:
                            raise CompatibilityError(
                                f"{stage}: CapEx fork-child request contains post-cutoff context "
                                f"marker {marker!r}"
                            )
                    validate_copied_fork(
                        records,
                        fork_manifest_entry["parent_thread_id"],
                        thread_id,
                        fork_manifest_entry["last_turn_id"],
                        fork_manifest_entry["forked_from_ordinal_exclusive"] - 1,
                        inherited_markers,
                        (
                            f"{prefix}:patched-high-on-native",
                            hook_marker,
                        ),
                    )
                    fork_manifest_entry["status"] = "passed"
                    fork_manifest_entry["capex_resume_stage"] = stage
                    fork_manifest_entry["capex_resume_status"] = "passed"

                manifest["completed_stages"].append(
                    {
                        "stage": stage,
                        "status": "passed",
                        "thread_id": thread_id,
                        "rollout_path": str(rollout_path),
                        "fork": fork_details,
                        "requests": [
                            {
                                "stage": request.stage,
                                "compaction": request_spec[1],
                                "capability_markers": list(request_spec[3]),
                            }
                            for request, request_spec in zip(
                                requests, step.requests, strict=True
                            )
                        ],
                    }
                )
                manifest["current_stage"] = None
                write_manifest(manifest_path, manifest)

            server.verify_complete()
            manifest["run_status"] = "passed"
            write_manifest(manifest_path, manifest)

    print(
        "PASS: each binary created a session the other resumed, appended to, and compacted across"
    )
    print("PASS: patched capability guidance stayed in ordinary response_item records")
    print(
        "PASS: native Codex preserved CapEx sidecars and both binaries shared the SQLite thread index"
    )
    print(
        "PASS: both compaction requests were followed by an immediate captured model request"
    )
    print(
        "PASS: CapEx capability instructions were present after compaction and in shared rollouts"
    )
    print(
        "PASS: a lastTurnId self-contained copied fork retained pre-cutoff content, "
        "excluded later grants, and both binaries cold-resumed and continued its child"
    )
    print(
        "UNTESTED: other fork boundaries and tagged skill-catalog exposure after grants"
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Check two-way session interoperability between pinned native Codex 0.156.1 "
            "and a CapEx binary using one temporary CODEX_HOME and a local Responses API mock. "
            "Both binaries are required to compact and resume the shared session format. "
            "A focused self-contained copied fork cutoff is covered, including native and "
            "CapEx cold-resume; other fork boundaries and "
            "tagged skill-catalog behavior are not."
        )
    )
    parser.add_argument(
        "--patched-bin",
        required=True,
        type=Path,
        help="path to the patched Codex binary to test",
    )
    parser.add_argument(
        "--expected-patched-sha256",
        required=True,
        help="required SHA-256 pin for the patched Codex executable",
    )
    parser.add_argument(
        "--native-bin",
        type=Path,
        default=NATIVE_CODEX,
        help=f"pinned native Codex 0.156.1 executable (default: {NATIVE_CODEX})",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=45,
        help="per-CLI-process timeout in seconds (default: 45)",
    )
    parser.add_argument(
        "--patched-revision",
        default=os.environ.get("CAPEX_BUILD_REVISION"),
        help="optional source revision/build identifier for the patched binary; defaults to CAPEX_BUILD_REVISION",
    )
    parser.add_argument(
        "--keep-temp",
        action="store_true",
        help="preserve temporary CODEX_HOME, workspace, process logs, and captured requests",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    # Stage subprocesses run from a temporary workspace, not the caller's cwd.
    args.native_bin = args.native_bin.resolve()
    args.patched_bin = args.patched_bin.resolve()
    try:
        if args.timeout <= 0:
            raise CompatibilityError("--timeout must be positive")
        expected_hash = args.expected_patched_sha256.lower()
        if len(expected_hash) != 64 or any(
            digit not in "0123456789abcdef" for digit in expected_hash
        ):
            raise CompatibilityError(
                "--expected-patched-sha256 must contain exactly 64 hexadecimal characters"
            )
        check_native_binary(args.native_bin)
        patched_hash = check_patched_binary(args.patched_bin, args.native_bin)
        if patched_hash != expected_hash:
            raise CompatibilityError(
                f"patched binary SHA-256 mismatch: expected {expected_hash}, got {patched_hash}"
            )
        print(f"PATCHED_BINARY={args.patched_bin.resolve()}")
        print(f"PATCHED_SHA256={patched_hash}")
        print(
            "PATCHED_BUILD_REVISION="
            + (args.patched_revision or "not supplied; SHA-256 identifies the binary")
        )
        run_compatibility(
            args.native_bin,
            args.patched_bin,
            args.timeout,
            args.keep_temp,
            patched_hash,
            args.patched_revision,
        )
    except (CompatibilityError, OSError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
