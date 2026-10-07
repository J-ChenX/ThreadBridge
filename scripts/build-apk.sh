#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${JAVA_HOME:?Set JAVA_HOME to JDK 17}"
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
: "${THREADBRIDGE_KEYSTORE:?Set THREADBRIDGE_KEYSTORE to the existing release keystore}"
: "${THREADBRIDGE_KEY_PASSWORD:?Set THREADBRIDGE_KEY_PASSWORD}"
export PATH="$JAVA_HOME/bin:$PATH"
printf 'sdk.dir=%s\n' "$ANDROID_HOME" > android/local.properties
android/gradlew -p android :app:testDebugUnitTest --no-daemon --no-parallel --console=plain
# Debug and release KSP both export the same Room schema; serialize them.
android/gradlew -p android :app:assembleRelease :app:lintRelease --no-daemon --no-parallel --console=plain
mkdir -p artifacts
release_version=$(python3 -c 'import json; print(json.load(open("android/app/build/outputs/apk/release/output-metadata.json"))["elements"][0]["versionName"])')
[[ "$release_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]] || { echo 'Invalid release version' >&2; exit 1; }
release_apk="artifacts/ThreadBridge-${release_version%-test}.apk"
"$ANDROID_HOME/build-tools/35.0.0/apksigner" verify --verbose android/app/build/outputs/apk/release/app-release.apk
"$ANDROID_HOME/build-tools/35.0.0/zipalign" -c -P 16 4 android/app/build/outputs/apk/release/app-release.apk
cp android/app/build/outputs/apk/release/app-release.apk "$release_apk"
(cd artifacts && sha256sum "${release_apk#artifacts/}" > "${release_apk#artifacts/}.sha256")
printf 'Verified APK: %s\n' "$release_apk"
