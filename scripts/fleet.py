#!/usr/bin/env python3
"""Compatibility launcher for existing systemd paths; fleet logic runs in Rust."""
import os
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[1]
binary = os.environ.get("THREADBRIDGE_BINARY", str(root / "target/release/threadbridge"))
os.chdir(root)
os.execv(binary, [binary, "fleet", *sys.argv[1:]])
