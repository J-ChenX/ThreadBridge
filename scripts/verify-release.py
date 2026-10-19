#!/usr/bin/env python3
"""Mandatory delivery gate against the recorded published APK and candidate policy."""
import argparse
import json
import os
from pathlib import Path

from apk_metadata import inspect
from release_policy import load_policy, validate_current, validate_previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path, nargs='?')
    parser.add_argument('--previous', type=Path, required=True)
    parser.add_argument('--sdk', type=Path, default=os.environ.get('ANDROID_HOME'))
    parser.add_argument('--preflight', action='store_true', help='Validate policy and published baseline before building')
    parser.add_argument('--output-report', type=Path)
    args = parser.parse_args()
    if args.sdk is None:
        parser.error('Set ANDROID_HOME or --sdk')
    if args.preflight and (args.apk is not None or args.output_report is not None):
        parser.error('--preflight does not accept a candidate APK or delivery report')
    if not args.preflight and args.apk is None:
        parser.error('A versioned candidate APK is required')
    policy = load_policy()
    prior = inspect(args.previous, args.sdk)
    validate_previous(prior, policy)
    if args.preflight:
        print(json.dumps({'baseline_verified': True, 'delivery_approved': False,
                          'published_version': prior['version']}, indent=2))
        return
    current = inspect(args.apk, args.sdk)
    validate_current(current, policy, args.apk.name)
    report = {'delivery_approved': True, 'upgrade_identity_verified': True, **current,
              'previous_version': prior['version'], 'previous_version_code': prior['version_code'],
              'previous_sha256': prior['sha256']}
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + '\n'
    if args.output_report:
        # Refuse overwriting an earlier verification record.
        with args.output_report.open('x', encoding='utf-8') as output:
            output.write(encoded)
    print(encoded, end='')


if __name__ == '__main__':
    main()
