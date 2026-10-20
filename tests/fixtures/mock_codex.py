#!/usr/bin/env python3
"""Immutable mock CLI for Rust tests; each symlink has isolated fixture data."""

import json
from pathlib import Path
import sys


# Do not resolve the symlink: configuration belongs to the calling test's directory.
fixture_dir = Path(sys.argv[0]).absolute().parent
with (fixture_dir / "fixture.json").open() as config_file:
    config = json.load(config_file)

if config["mode"] == "steer":
    if len(sys.argv)>1 and sys.argv[1] == "queue":
        assert sys.argv[sys.argv.index("--thread") + 1] == config["native_id"]
        assert sys.argv[sys.argv.index("--message") + 1] == "explicit phone guidance"
        with (fixture_dir / "calls").open("a") as calls:
            calls.write("queue\n")
        print(f"Queued message {config['queue_id']} for thread {config['native_id']}.")
        sys.exit(0)
    if "--version" in sys.argv:
        print("codex-cli fixture")
        sys.exit(0)
    assert sys.argv[1:] == ["app-server", "proxy"]
    if config.get("no_socket"):
        sys.exit(1)
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method, params = request["method"], request["params"]
        native, turn, queued = config["native_id"], config["turn_id"], config["queue_id"]
        if method == "initialize":
            result = {}
        elif method == "thread/read":
            assert params == {"threadId": native, "includeTurns": False}
            result = {"thread": {"id": native, "status": {"type": "idle" if config.get("idle") else "active"}}}
        elif method == "thread/turns/list":
            assert params["threadId"] == native
            result = {"data": [{"id": "stale" if config.get("wrong_turn") else turn, "status": "inProgress"}]}
        elif method == "thread/queue/list":
            assert params["threadId"] == native
            result = {"data": [] if config.get("queue_missing") else [{"id": queued, "clientUserMessageId": "queued-client-id", "input": [{"type": "text", "text": "different" if config.get("queue_changed") else "explicit phone guidance"}]}], "nextCursor": None}
        elif method == "thread/queue/delete":
            assert params == {"threadId": native, "queuedSubmissionId": queued}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("delete\n")
            if config.get("lose_delete"):
                sys.exit(0)
            result = {"deleted": not config.get("delete_missing", False)}
        elif method == "turn/steer":
            assert params == {"threadId": native, "expectedTurnId": turn, "clientUserMessageId": "queued-client-id", "input": [{"type": "text", "text": "explicit phone guidance"}]}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("steer\n")
            if config.get("lose_steer"):
                sys.exit(0)
            if config.get("reject_steer"):
                print(json.dumps({"id": request["id"], "error": {"code": -32600, "message": "turn changed"}}), flush=True)
                continue
            result = {"turnId": turn}
        else:
            raise RuntimeError("unexpected steering method")
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
    sys.exit(0)

if config["mode"] == "create":
    if "--version" in sys.argv:
        print("codex-cli fixture")
        sys.exit(0)
    if sys.argv[1] == "queue":
        raise RuntimeError("first turn must start directly, never queue")
    if sys.argv[1:] == ["app-server", "proxy"] and config.get("no_socket", False):
        sys.exit(1)
    assert sys.argv[1:] == (["app-server", "--stdio"] if config.get("no_socket", False) else ["app-server", "proxy"])
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        if request["method"] == "initialize":
            if config.get("fail_initialize", False):
                print(json.dumps({"id": request["id"], "error": {"code": -32603, "message": "fixture unavailable"}}), flush=True)
                continue
            result = {}
        elif request["method"] == "thread/start":
            assert request["params"] == {"cwd": config.get("cwd", str(fixture_dir))}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("start\n")
            if config.get("lose_start", False):
                sys.exit(0)
            result = {"thread": {"id": config["native_id"]}}
        elif request["method"] == "thread/name/set":
            assert request["params"] == {"threadId": config["native_id"], "name": "phone starts here"}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("name\n")
            if config.get("lose_name", False):
                sys.exit(0)
            result = {}
        elif request["method"] == "turn/start":
            assert request["params"] == {"threadId": config["native_id"], "input": [{"type": "text", "text": "phone starts here"}]}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("turn\n")
            if not config.get("reply", True):
                sys.exit(0)
            if config.get("turn_error", False):
                print(json.dumps({"id": request["id"], "error": {"code": -32603, "message": "fixture unavailable"}}), flush=True)
                continue
            result = {"turn": {"id": "00000000-0000-4000-8000-000000000009"}}
        else:
            raise RuntimeError("unexpected creation method")
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
elif config["mode"] == "queue":
    with (fixture_dir / "calls").open("a") as calls:
        calls.write("call\n")
    assert sys.argv[1] == "queue"
    assert sys.argv[sys.argv.index("--thread") + 1] == config["native_id"]
    assert sys.argv[sys.argv.index("--message") + 1] == "explicit phone action"
    if config["reply"]:
        print(f"Queued message 00000000-0000-4000-8000-000000000004 for thread {config['native_id']}.")
    else:
        print("uncertain")
elif config["mode"] == "proxy":
    if "--version" in sys.argv:
        print("codex-cli fixture")
        sys.exit(0)
    assert sys.argv[1:] == ["app-server", "proxy"]
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request["method"]
        if method == "initialize":
            result = {}
        elif method == "thread/loaded/list":
            result = {"data": ["original"] if config["loaded"] else []}
        elif method == "thread/read":
            result = {"thread": {"id": "original", "status": {"type": "idle"}, "updatedAt": 1}}
        elif method == "thread/turns/list":
            result = {"data": [{"id": "prior", "status": "completed", "items": []}], "nextCursor": None}
        elif method == "turn/start":
            result = {"turn": {"id": "existing-task-turn"}}
        else:
            raise RuntimeError(f"unexpected method: {method}")
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
else:
    raise RuntimeError("unexpected fixture mode")
