#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# != 1 ]]; then
  echo 'Usage: scripts/build-apk.sh /path/to/previous-published.apk' >&2
  exit 2
fi
# Fail on a stale version before touching build configuration or outputs.
release_name=$(python3 scripts/release_policy.py --field artifactName)
: "${JAVA_HOME:?Set JAVA_HOME to JDK 17}"
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
: "${THREADBRIDGE_KEYSTORE:?Set THREADBRIDGE_KEYSTORE to the existing release keystore}"
: "${THREADBRIDGE_KEY_PASSWORD:?Set THREADBRIDGE_KEY_PASSWORD}"
export PATH="$JAVA_HOME/bin:$PATH"
python3 scripts/verify-release.py --preflight --previous "$1" --sdk "$ANDROID_HOME"
release_apk="artifacts/$release_name"
release_report="${release_apk%.apk}.verification.json"
for release_output in "$release_apk" "$release_apk.sha256" "$release_report"; do
  [[ ! -e "$release_output" ]] || { echo "Refusing to overwrite $release_output" >&2; exit 1; }
done
printf 'sdk.dir=%s\n' "$ANDROID_HOME" > android/local.properties
android/gradlew -p android :app:testDebugUnitTest --no-daemon --no-parallel --console=plain
# Debug and release KSP both export the same Room schema; serialize them.
android/gradlew -p android :app:assembleRelease :app:lintRelease --no-daemon --no-parallel --console=plain
mkdir -p artifacts
release_stage=$(mktemp -d artifacts/.release-staging.XXXXXX)
trap 'rm -rf -- "$release_stage"' EXIT
cp android/app/build/outputs/apk/release/app-release.apk "$release_stage/$release_name"
python3 scripts/verify-release.py "$release_stage/$release_name" --previous "$1" --sdk "$ANDROID_HOME" --output-report "$release_stage/report.json"
(cd "$release_stage" && sha256sum "$release_name" > checksum.sha256)
# Hard links create each output exclusively: an existing APK is never replaced.
ln "$release_stage/$release_name" "$release_apk"
ln "$release_stage/checksum.sha256" "$release_apk.sha256"
ln "$release_stage/report.json" "$release_report"
printf 'Verified release APK: %s\nVerification report: %s\n' "$release_apk" "$release_report"
