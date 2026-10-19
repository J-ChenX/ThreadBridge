"""Inspect real APK signing, alignment and Android metadata using the pinned SDK."""
import hashlib
import re
import subprocess


def inspect(apk, sdk):
    tools = sdk / 'build-tools' / '35.0.0'
    certificates = subprocess.check_output(
        [tools / 'apksigner', 'verify', '--print-certs', apk], text=True
    )
    signers = re.findall(r'Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]{64})\b', certificates)
    if len(signers) != 1:
        raise ValueError('Expected exactly one verified APK signing certificate')
    subprocess.run([tools / 'zipalign', '-c', '-P', '16', '4', apk], check=True)
    metadata = subprocess.check_output([tools / 'aapt', 'dump', 'badging', apk], text=True)
    package = re.search(r"package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'", metadata)
    if not package:
        raise ValueError('Missing APK package/version metadata')
    return dict(package=package[1], version_code=int(package[2]), version=package[3],
                debuggable='application-debuggable' in metadata.splitlines(),
                signer_sha256=signers[0].lower(), sha256=hashlib.sha256(apk.read_bytes()).hexdigest())
