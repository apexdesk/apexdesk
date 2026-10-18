#!/usr/bin/env bash
# Installs the validators `apex-bench validate` uses into target/tools of
# the main checkout (nothing in the home directory):
#   dotnet/            .NET 8 SDK (official dotnet-install.sh; its own
#                      extraction failed here, so the tarball it names is
#                      unpacked with tar)
#   ooxml-validate/    this project built (Open XML SDK validator)
#   odfvalidator-*.jar ODF Toolkit validator from Maven Central (needs Java)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
common="$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
if [ -n "$common" ]; then TOOLS="$(dirname "$common")/target/tools"; else TOOLS="$HERE/../../target/tools"; fi
TOOLS="${APEX_BENCH_TOOLS:-$TOOLS}"
mkdir -p "$TOOLS"
export DOTNET_CLI_HOME="$TOOLS/dotnet-home" NUGET_PACKAGES="$TOOLS/nuget"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1

if [ ! -x "$TOOLS/dotnet/dotnet" ] || [ ! -s "$TOOLS/dotnet/dotnet" ]; then
    curl -fsSL -o "$TOOLS/dotnet-install.sh" https://dot.net/v1/dotnet-install.sh
    url="$(bash "$TOOLS/dotnet-install.sh" --channel 8.0 --install-dir "$TOOLS/dotnet" --no-path --dry-run 2>&1 | grep -o 'https://[^ ]*dotnet-sdk-[^ ]*\.tar\.gz' | head -1)"
    rm -rf "$TOOLS/dotnet" && mkdir -p "$TOOLS/dotnet"
    curl -fsSL "$url" | tar -xz -C "$TOOLS/dotnet"
fi

"$TOOLS/dotnet/dotnet" build -c Release -o "$TOOLS/ooxml-validate" \
    -p:BaseIntermediateOutputPath="$TOOLS/ooxml-validate-obj/" "$HERE/ooxml-validate.csproj"

JAR="odfvalidator-0.13.0.jar"
if [ ! -s "$TOOLS/$JAR" ]; then
    base="https://repo1.maven.org/maven2/org/odftoolkit/odfvalidator/0.13.0/odfvalidator-0.13.0-jar-with-dependencies.jar"
    curl -fsSL -o "$TOOLS/$JAR.part" "$base"
    echo "$(curl -fsSL "$base.sha1")  $TOOLS/$JAR.part" | sha1sum -c --quiet -
    mv "$TOOLS/$JAR.part" "$TOOLS/$JAR"
fi
echo "validators in $TOOLS" >&2
