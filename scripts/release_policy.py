#!/usr/bin/env python3
"""Shared Android candidate and published-release policy; contains no signing secrets."""
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'android/release.properties'
KEYS = {'applicationId', 'versionName', 'versionCode', 'publishedVersionName',
        'publishedVersionCode', 'publishedApkSha256', 'signerSha256'}


def version(value):
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', value):
        raise ValueError('Release version must be MAJOR.MINOR.PATCH without suffixes')
    return tuple(map(int, value.split('.')))


def load_policy(path=POLICY):
    policy = {}
    for line in path.read_text(encoding='utf-8').splitlines():
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        key, separator, value = line.partition('=')
        key, value = key.strip(), value.strip()
        if not separator or key in policy:
            raise ValueError('Invalid or duplicate release policy entry')
        policy[key] = value
    if set(policy) != KEYS:
        raise ValueError('Release policy has missing or unknown fields')
    if policy['applicationId'] != 'dev.threadbridge':
        raise ValueError('Release applicationId must remain dev.threadbridge')
    if version(policy['versionName']) <= version(policy['publishedVersionName']):
        raise ValueError('Candidate version must exceed the published version')
    for key in ('versionCode', 'publishedVersionCode'):
        if not re.fullmatch(r'[1-9]\d*', policy[key]) or int(policy[key]) > 2100000000:
            raise ValueError('Invalid Android versionCode')
        policy[key] = int(policy[key])
    if policy['versionCode'] <= policy['publishedVersionCode']:
        raise ValueError('Candidate versionCode must exceed the published versionCode')
    for key in ('publishedApkSha256', 'signerSha256'):
        if not re.fullmatch('[0-9a-f]{64}', policy[key]):
            raise ValueError('Invalid release identity digest')
    return policy


def artifact_name(policy):
    return f"ThreadBridge-{policy['versionName']}.apk"


def validate_previous(prior, policy):
    expected = {'package': policy['applicationId'], 'version': policy['publishedVersionName'],
                'version_code': policy['publishedVersionCode'],
                'sha256': policy['publishedApkSha256'], 'signer_sha256': policy['signerSha256'],
                'debuggable': False}
    if any(prior.get(key) != value for key, value in expected.items()):
        raise ValueError('Previous APK does not match the recorded published release identity')


def validate_current(current, policy, filename):
    if current['package'] != policy['applicationId'] or current['debuggable']:
        raise ValueError('Delivery requires a non-debuggable dev.threadbridge release APK')
    if current['version'] != policy['versionName'] or current['version_code'] != policy['versionCode']:
        raise ValueError('APK version does not match android/release.properties')
    if current['signer_sha256'] != policy['signerSha256']:
        raise ValueError('APK signing identity differs from the published release')
    if filename != artifact_name(policy):
        raise ValueError('APK filename must exactly match its release version')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--field', choices=sorted(KEYS | {'artifactName'}))
    parser.add_argument('--check-debug-apk', type=Path)
    parser.add_argument('--sdk', type=Path)
    args = parser.parse_args()
    policy = load_policy()
    if args.check_debug_apk:
        if args.sdk is None:
            parser.error('--check-debug-apk requires --sdk')
        from apk_metadata import inspect
        meta = inspect(args.check_debug_apk, args.sdk)
        expected = {'package': policy['applicationId'] + '.debug',
                    'version': policy['versionName'] + '-debug',
                    'version_code': policy['versionCode'], 'debuggable': True}
        if any(meta[key] != value for key, value in expected.items()):
            raise ValueError('Debug build metadata drifted from the shared version policy')
        print(json.dumps({'debug_build_valid': True, 'delivery_approved': False, **meta}, indent=2))
    elif args.field:
        print(artifact_name(policy) if args.field == 'artifactName' else policy[args.field])
    else:
        print(json.dumps(policy, indent=2))


if __name__ == '__main__':
    main()
