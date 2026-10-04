#!/usr/bin/env python3
"""Run isolated regressions using only synthetic data and local mock processes."""
import argparse
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument('--unit-only', action='store_true')
    scope.add_argument('--integration-only', action='store_true')
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/threadbridge')
    args = parser.parse_args()
    env = os.environ.copy()
    env['PYTHONPATH'] = str(ROOT / 'tests/integration')
    env['PYTHONDONTWRITEBYTECODE'] = '1'

    def run(*command):
        subprocess.run([sys.executable, *map(str, command)], cwd=ROOT, env=env, check=True)

    if not args.integration_only:
        subprocess.run(['cargo', 'test', '--workspace', '--locked'], cwd=ROOT, env=env, check=True)
    if not args.unit_only:
        binary = args.binary.resolve()
        if not binary.is_file():
            parser.error('Build the release binary first: cargo build --release --locked')
        cases = [
            ('smoke.py', []),
            ('test_capture_sync.py', []),
            ('test_capture_bridge.py', []),
            ('test_capture_bridge.py', ['--all']),
            ('test_saved_inbox.py', []),
            ('test_saved_inbox.py', ['--all']),
            ('test_resume_phone.py', []),
            ('test_resume_phone.py', ['--worker']),
            ('test_conversation_sync.py', []),
            ('test_capture_correlation.py', []),
            ('test_subagent_cleanup.py', []),
            ('test_capture_health_scope.py', []),
            ('test_archived_cleanup.py', []),
            ('test_visible_history.py', []),
            ('test_android_migrations.py', []),
        ]
        for name, extra in cases:
            print(f'Running {name} {" ".join(str(x) for x in extra if str(x).startswith("--"))}', flush=True)
            run(ROOT / 'tests/integration' / name, binary, *extra)


if __name__ == '__main__':
    main()
