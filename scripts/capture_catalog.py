#!/usr/bin/env python3
"""Compatibility launcher for existing notify configuration; capture runs in Rust."""
import os
from pathlib import Path
import sys

binary = os.environ.get("THREADBRIDGE_BINARY", str(Path(__file__).resolve().parents[1] / "target/release/threadbridge"))
os.execv(binary, [binary, "capture", *sys.argv[1:]])
