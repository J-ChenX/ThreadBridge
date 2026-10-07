#!/usr/bin/env python3
"""Immutable mock CLI for Rust tests; each symlink has isolated fixture data."""

import json
from pathlib import Path
import sys


# Do not resolve the symlink: configuration belongs to the calling test's directory.
fixture_dir = Path(sys.argv[0]).absolute().parent
with (fixture_dir / "fixture.json").open() as config_file:
    config = json.load(config_file)

if config["mode"] == "create":
    if "--version" in sys.argv:
        print("codex-cli fixture")
        sys.exit(0)
    if sys.argv[1] == "queue":
        assert sys.argv[sys.argv.index("--thread") + 1] == config["native_id"]
        assert sys.argv[-1] == "--message=phone starts here"
        with (fixture_dir / "calls").open("a") as calls:
            calls.write("queue\n")
        if config.get("queue_error", False):
            sys.exit(9)
        if config.get("reply", True):
            print(f"Queued message 00000000-0000-4000-8000-000000000004 for thread {config['native_id']}.")
        else:
            print("uncertain")
        sys.exit(0)
    assert sys.argv[1:] == ["app-server", "proxy"]
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        if request["method"] == "initialize":
            result = {}
        elif request["method"] == "thread/start":
            assert request["params"] == {"cwd": str(fixture_dir)}
            with (fixture_dir / "calls").open("a") as calls:
                calls.write("start\n")
            if config.get("lose_start", False):
                sys.exit(0)
            result = {"thread": {"id": config["native_id"]}}
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
