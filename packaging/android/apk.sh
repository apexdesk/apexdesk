#!/bin/bash
# Builds dist/apexdesk-<version>-android.apk: crates/apex-android (the
# program as a NativeActivity's library) for 64-bit ARM, packed and signed
# with the SDK's build tools, with the one Java class that puts it full
# screen (java/). No Gradle.
#
# Needs: the Android SDK (build-tools, a platform) and NDK, a JDK (javac), and
#   rustup target add aarch64-linux-android
# ANDROID_HOME (default ~/android-sdk) and ANDROID_NDK_HOME (default the
# newest NDK inside the SDK) say where they are.
# APEXDESK_ANDROID_TARGETS: Rust targets to include (default
# aarch64-linux-android; x86_64-linux-android adds the emulator's).
# APEXDESK_ANDROID_KEYSTORE / APEXDESK_ANDROID_KEY_PASS: the key that signs
# (default the SDK's debug key, ~/.android/debug.keystore).
set -euo pipefail
cd "$(dirname "$0")/../.."
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

for target in ${APEXDESK_ANDROID_TARGETS:-aarch64-linux-android}; do
    case $target in
        aarch64-linux-android) abi=arm64-v8a ;;
        x86_64-linux-android) abi=x86_64 ;;
        *) echo "unknown target $target" >&2; exit 1 ;;
    esac
    t=${target//-/_}
    export "CC_$t=$llvm/$target$api-clang" "CXX_$t=$llvm/$target$api-clang++" "AR_$t=$llvm/llvm-ar"
    export "CARGO_TARGET_${t^^}_LINKER=$llvm/$target$api-clang"
    cargo build --release --target "$target" -p apex-android
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
export APEXDESK_ANDROID_KEY_PASS=${APEXDESK_ANDROID_KEY_PASS:-android}
"$tools/apksigner" sign --ks "${APEXDESK_ANDROID_KEYSTORE:-$HOME/.android/debug.keystore}" \
    --ks-pass env:APEXDESK_ANDROID_KEY_PASS --out "$out" "$work/aligned.apk"
echo "$out"
