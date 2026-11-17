#!/bin/bash
# Builds dist/apexdesk-<version>-android.apk: crates/apex-android (the
# program as a NativeActivity's library) for 64- and 32-bit ARM, packed and signed
# with the SDK's build tools, with the one Java class that puts it full
# screen (java/). No Gradle.
#
# Needs: the Android SDK (build-tools, a platform) and NDK, a JDK (javac), and
#   rustup target add aarch64-linux-android armv7-linux-androideabi
# ANDROID_HOME (default ~/android-sdk) and ANDROID_NDK_HOME (default the
# newest NDK inside the SDK) say where they are.
# APEXDESK_ANDROID_TARGETS: Rust targets to include (default
# aarch64-linux-android and armv7-linux-androideabi, the latter for devices
# with a 32-bit Android; x86_64-linux-android adds the emulator's).
# APEXDESK_ANDROID_KEYSTORE / APEXDESK_ANDROID_KEY_PASS: the key that signs.
# Without them the build stops: the SDK's debug key
# (~/.android/debug.keystore, password "android") signs only when asked for
# with --debug or APEXDESK_ANDROID_DEBUG_KEY=1, for copies to try out (an
# app signed with it can't be updated by one signed with the real key).
set -euo pipefail
cd "$(dirname "$0")/../.."

debug_key=${APEXDESK_ANDROID_DEBUG_KEY:-0}
for arg in "$@"; do
    case "$arg" in
        --debug) debug_key=1 ;;
        *) echo "usage: packaging/android/apk.sh [--debug]" >&2; exit 2 ;;
    esac
done
if [ -n "${APEXDESK_ANDROID_KEYSTORE:-}" ]; then
    keystore=$APEXDESK_ANDROID_KEYSTORE
    if [ -z "${APEXDESK_ANDROID_KEY_PASS:-}" ]; then
        echo "APEXDESK_ANDROID_KEY_PASS is not set (the password of $keystore)" >&2
        exit 1
    fi
elif [ "$debug_key" = 1 ]; then
    keystore=$HOME/.android/debug.keystore
    export APEXDESK_ANDROID_KEY_PASS=android
else
    echo "No signing key: set APEXDESK_ANDROID_KEYSTORE and APEXDESK_ANDROID_KEY_PASS," >&2
    echo "or pass --debug to sign a test copy with the SDK's debug key." >&2
    exit 1
fi
here=packaging/android

sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android-sdk}}
ndk=${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}
tools=$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)
jar=$(ls -d "$sdk"/platforms/android-* | sort -V | tail -1)/android.jar
llvm=$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin
api=26

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IFS=. read -r major minor patch <<< "$version"
code=$((major * 10000 + minor * 100 + patch))

work=target/android-apk
rm -rf "$work"
mkdir -p "$work" dist

for target in ${APEXDESK_ANDROID_TARGETS:-aarch64-linux-android armv7-linux-androideabi}; do
    # The NDK names its 32-bit ARM compiler armv7a-, not armv7-.
    clang=$target
    case $target in
        aarch64-linux-android) abi=arm64-v8a ;;
        armv7-linux-androideabi) abi=armeabi-v7a clang=armv7a-linux-androideabi ;;
        x86_64-linux-android) abi=x86_64 ;;
        *) echo "unknown target $target" >&2; exit 1 ;;
    esac
    t=${target//-/_}
    export "CC_$t=$llvm/$clang$api-clang" "CXX_$t=$llvm/$clang$api-clang++" "AR_$t=$llvm/llvm-ar"
    export "CARGO_TARGET_${t^^}_LINKER=$llvm/$clang$api-clang"
    cargo build --release --locked --target "$target" -p apex-android
    mkdir -p "$work/lib/$abi"
    cp "target/$target/release/libapexdesk.so" "$work/lib/$abi/"
done

mkdir -p "$work/classes"
javac -source 8 -target 8 -Xlint:-options -classpath "$jar" -d "$work/classes" $(find "$here/java" -name '*.java')
"$tools/d8" --min-api "$api" --lib "$jar" --output "$work" $(find "$work/classes" -name '*.class')

"$tools/aapt2" compile --dir "$here/res" -o "$work/res.zip"
"$tools/aapt2" link -o "$work/base.apk" -I "$jar" --manifest "$here/AndroidManifest.xml" \
    --version-code "$code" --version-name "$version" --replace-version -R "$work/res.zip" --auto-add-overlay
(cd "$work" && zip -qr base.apk lib classes.dex)
"$tools/zipalign" -f -p 4 "$work/base.apk" "$work/aligned.apk"
out=dist/apexdesk-$version-android.apk
# The password from the environment, not the command line (where any
# process can read it).
"$tools/apksigner" sign --ks "$keystore" \
    --ks-pass env:APEXDESK_ANDROID_KEY_PASS --out "$out" "$work/aligned.apk"
# Which key signed it, so a release made with the debug key shows.
if [ "$debug_key" = 1 ] && [ -z "${APEXDESK_ANDROID_KEYSTORE:-}" ]; then
    echo "Signed with the DEBUG key ($keystore): not for release." >&2
else
    echo "Signed with $keystore" >&2
fi
"$tools/apksigner" verify --print-certs "$out" | grep -m2 -E 'certificate (DN|SHA-256 digest)' >&2 || true
scripts/sha256.sh "$out"
echo "$out"
