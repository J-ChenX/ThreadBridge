#!/usr/bin/env python3
"""Verify a signed APK and, optionally, its compatibility with an older install."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess


def inspect(apk, sdk):
    tools = sdk / "build-tools" / "35.0.0"
    certificates = subprocess.check_output(
        [tools / "apksigner", "verify", "--print-certs", apk], text=True
    )
    signer = re.search(r"Signer #1 certificate SHA-256 digest: (\w+)", certificates)
    if not signer:
        raise ValueError("Missing verified signing certificate")
    subprocess.run([tools / "zipalign", "-c", "-P", "16", "4", apk], check=True)
    metadata = subprocess.check_output([tools / "aapt", "dump", "badging", apk], text=True)
    package = re.search(r"package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'", metadata)
    if not package:
        raise ValueError("Missing package metadata")
    return dict(package=package[1], version_code=int(package[2]), version=package[3],
                signer_sha256=signer[1], sha256=hashlib.sha256(apk.read_bytes()).hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=pathlib.Path)
    parser.add_argument("--previous", type=pathlib.Path)
    parser.add_argument("--expected-version", help="Require the APK versionName to match the release tag")
    parser.add_argument("--signer-sha256", help="Require the original signing certificate SHA-256")
    parser.add_argument("--sdk", type=pathlib.Path, default=os.environ.get("ANDROID_HOME"))
    args = parser.parse_args()
    if args.sdk is None:
        parser.error("Set ANDROID_HOME or --sdk")
    current = inspect(args.apk, args.sdk)
    if current["package"] != "dev.threadbridge":
        raise ValueError("Release must use dev.threadbridge (no debug suffix)")
    if args.expected_version and current["version"] != args.expected_version:
        raise ValueError("APK versionName does not match the release tag")
    if args.signer_sha256 and current["signer_sha256"].lower() != args.signer_sha256.lower():
        raise ValueError("APK does not use the original signing certificate")
    if args.previous:
        prior = inspect(args.previous, args.sdk)
        if prior["package"] != current["package"] or prior["signer_sha256"] != current["signer_sha256"]:
            raise ValueError("Package or signing identity changed: cannot update existing installation")
        if prior["version_code"] >= current["version_code"]:
            raise ValueError("Upgrade requires a higher versionCode")
        current["upgrade_identity_verified"] = True
    print(json.dumps(current, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
