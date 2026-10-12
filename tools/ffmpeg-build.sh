#!/usr/bin/env bash
# Only for builds with apex-app's `ffmpeg` feature (off by default: videos
# and sounds play through the system's own decoders, GStreamer, Media
# Foundation or AVFoundation, and no FFmpeg is shipped). Kept as a fallback.
#
# Builds the optional FFmpeg component that plays videos and sounds in
# presentations: a trimmed LGPL FFmpeg as shared libraries (decoders and
# demuxers only, no programs, no encoders, no network, nothing GPL or
# nonfree). ApexDesk loads it while running from components/ffmpeg/lib
# next to the program, or from APEXDESK_FFMPEG (the folder or "off").
#
# Usage: tools/ffmpeg-build.sh SOURCE_DIR OUT_DIR
#   SOURCE_DIR  an unpacked FFmpeg 7.1 release (ffmpeg-7.1.x)
#   OUT_DIR     where to build; the libraries end in OUT_DIR/inst/lib
# Nothing is installed in the system.
set -euo pipefail

src="$(cd "$1" && pwd)"
out="$2"
mkdir -p "$out/b"
out="$(cd "$out" && pwd)"
cd "$out/b"

decoders="h264,hevc,vp8,vp9,mpeg4,msmpeg4v3,vc1,wmv1,wmv2,wmv3,mjpeg,aac,aac_latm,mp3,mp3float,mp2,opus,vorbis,flac,wmav1,wmav2,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8,pcm_mulaw,pcm_alaw,adpcm_ms,adpcm_ima_wav"
demuxers="mov,matroska,avi,asf,wav,mp3,ogg,flac,aac"
parsers="h264,hevc,vp8,vp9,mpeg4video,vc1,aac,aac_latm,mpegaudio,opus,vorbis,flac"
# On macOS the libraries find each other next to themselves; on Windows
# (MSYS2) the DLLs go to inst/bin, where ApexDesk looks too.
extra=()
case "$(uname -s)" in
    Darwin) extra+=(--install-name-dir='@loader_path') ;;
esac

"$src/configure" --prefix="$out/inst" \
    --disable-everything --disable-programs --disable-doc --disable-avdevice \
    --disable-avfilter --disable-network --disable-autodetect --disable-x86asm \
    --disable-static --enable-shared --enable-small \
    --enable-swscale --enable-swresample \
    --enable-decoder="$decoders" --enable-demuxer="$demuxers" \
    --enable-parser="$parsers" --enable-protocol=file "${extra[@]}" >/dev/null
make -j"${JOBS:-6}" >/dev/null
make install >/dev/null
strip "$out"/inst/lib/*.so.*.*.* 2>/dev/null || true
ls -l "$out"/inst/lib/ | grep -E '\.(so|dylib|dll)' || true
