#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
MANIFEST="${ROOT_DIR}/packaging/java-tools.env"

fail() {
  printf 'package-release: error: %s\n' "$*" >&2
  exit 1
}

[[ -f "$MANIFEST" ]] || fail "tool manifest is missing: $MANIFEST"
# The checked-in manifest contains only simple quoted assignments.
# shellcheck source=../packaging/java-tools.env
source "$MANIFEST"

[[ "$(uname -s)" == "Darwin" ]] || fail "macOS packaging is supported only on macOS (found $(uname -s))"

for command_name in cargo curl tar git shasum codesign; do
  command -v "$command_name" >/dev/null 2>&1 || fail "required command '$command_name' was not found in PATH"
done

if [[ -n "${JAVA_HOME:-}" ]]; then
  java_home="$JAVA_HOME"
elif [[ -x /usr/libexec/java_home ]]; then
  java_home="$(/usr/libexec/java_home -v '21+' 2>/dev/null || true)"
else
  java_home=""
fi
[[ -n "$java_home" && -x "$java_home/bin/java" ]] || fail "Java 21 or newer is required locally. Set JAVA_HOME to an installed JDK; this script does not install Java."

java_version_output="$("$java_home/bin/java" -version 2>&1)" || fail "could not run Java from JAVA_HOME=$java_home"
java_major="$(printf '%s\n' "$java_version_output" | sed -nE 's/.*version "([0-9]+)(\.[0-9]+)?.*/\1/p' | head -n 1)"
[[ "$java_major" =~ ^[0-9]+$ ]] || fail "could not determine Java version from JAVA_HOME=$java_home"
(( java_major >= 21 )) || fail "Java 21 or newer is required (found Java $java_major at $java_home)"

export JAVA_HOME="$java_home"
export PATH="$JAVA_HOME/bin:$PATH"

DIST_DIR="${ROOT_DIR}/dist"
STAGE_DIR=""
MIGRATED_DIST=""
PUBLISH_LOCK="${ROOT_DIR}/.dist-publish.lock"
PUBLISHED=0
LOCK_HELD=0
cleanup() {
  if [[ "$PUBLISHED" == 1 && -n "$STAGE_DIR" && -d "$STAGE_DIR" ]]; then rm -rf -- "$STAGE_DIR"; fi
  if [[ "$PUBLISHED" == 0 && -n "$STAGE_DIR" && -d "$STAGE_DIR" ]]; then
    printf 'package-release: staging retained for inspection: %s\n' "$STAGE_DIR" >&2
  fi
  if [[ -n "$MIGRATED_DIST" && -d "$MIGRATED_DIST" && ! -e "$DIST_DIR" && ! -L "$DIST_DIR" ]]; then
    mv "$MIGRATED_DIST" "$DIST_DIR" || true
  fi
  if [[ "$LOCK_HELD" == 1 ]]; then rmdir "$PUBLISH_LOCK" 2>/dev/null || true; fi
  rm -f "${ROOT_DIR}/.dist-link.$$"
}
trap cleanup EXIT

# Stage as a sibling so final directory moves stay on the same filesystem.
STAGE_DIR="$(mktemp -d "${ROOT_DIR}/.dist-stage.XXXXXX")"
JDT_ARCHIVE_PATH="${STAGE_DIR}/${JDTLS_ARCHIVE}"
JDT_EXTRACT_DIR="${STAGE_DIR}/jdtls-extract"
DEBUG_REPO_DIR="${STAGE_DIR}/java-debug-source"

printf 'Downloading Eclipse JDT LS %s...\n' "$JDTLS_VERSION"
curl --fail --location --silent --show-error \
  "${JDTLS_BASE_URL}/${JDTLS_ARCHIVE}" -o "$JDT_ARCHIVE_PATH"
curl --fail --location --silent --show-error \
  "$JDTLS_SHA256_URL" -o "${JDT_ARCHIVE_PATH}.sha256"

sidecar_hash="$(awk 'NR == 1 { print $1 }' "${JDT_ARCHIVE_PATH}.sha256")"
[[ "$sidecar_hash" =~ ^[[:xdigit:]]{64}$ ]] || fail "official JDT LS SHA-256 sidecar has an invalid format"
actual_hash="$(shasum -a 256 "$JDT_ARCHIVE_PATH" | awk '{ print $1 }')"
actual_hash="$(printf '%s' "$actual_hash" | tr '[:upper:]' '[:lower:]')"
sidecar_hash="$(printf '%s' "$sidecar_hash" | tr '[:upper:]' '[:lower:]')"
[[ "$actual_hash" == "$sidecar_hash" ]] || fail "JDT LS archive checksum mismatch (expected ${sidecar_hash}, got ${actual_hash}); refusing extraction"

mkdir -p "$JDT_EXTRACT_DIR"
tar -xzf "$JDT_ARCHIVE_PATH" -C "$JDT_EXTRACT_DIR"
[[ -x "${JDT_EXTRACT_DIR}/bin/jdtls" ]] || fail "JDT LS archive did not contain bin/jdtls"

printf 'Fetching Eclipse JDT LS license for %s...\n' "$JDTLS_VERSION"
JDTLS_LICENSE_PATH="${STAGE_DIR}/jdtls-LICENSE"
curl --fail --location --silent --show-error "$JDTLS_LICENSE_URL" -o "$JDTLS_LICENSE_PATH"
actual_license_hash="$(shasum -a 256 "$JDTLS_LICENSE_PATH" | awk '{ print $1 }')"
actual_license_hash="$(printf '%s' "$actual_license_hash" | tr '[:upper:]' '[:lower:]')"
expected_license_hash="$(printf '%s' "$JDTLS_LICENSE_SHA256" | tr '[:upper:]' '[:lower:]')"
[[ "$expected_license_hash" =~ ^[[:xdigit:]]{64}$ ]] || fail "pinned JDT LS license SHA-256 has an invalid format"
[[ "$actual_license_hash" == "$expected_license_hash" ]] || fail "JDT LS license checksum mismatch (expected ${expected_license_hash}, got ${actual_license_hash})"

printf 'Fetching Microsoft java-debug %s...\n' "$JAVA_DEBUG_VERSION"
git clone --quiet --depth 1 --branch "$JAVA_DEBUG_TAG" "$JAVA_DEBUG_REPOSITORY" "$DEBUG_REPO_DIR"
actual_commit="$(git -C "$DEBUG_REPO_DIR" rev-parse HEAD)"
[[ "$actual_commit" == "$JAVA_DEBUG_COMMIT_PREFIX"* ]] || fail "java-debug tag resolved to unexpected commit ${actual_commit} (expected prefix ${JAVA_DEBUG_COMMIT_PREFIX})"
[[ -x "${DEBUG_REPO_DIR}/mvnw" ]] || fail "java-debug checkout is missing its Maven Wrapper"
JAVA_DEBUG_TARGET_PATCH="${ROOT_DIR}/packaging/java-debug-target.patch"
git -C "$DEBUG_REPO_DIR" apply --check "$JAVA_DEBUG_TARGET_PATCH" || fail "java-debug target-platform compatibility patch does not apply to the pinned source"
git -C "$DEBUG_REPO_DIR" apply "$JAVA_DEBUG_TARGET_PATCH"

printf 'Building java-debug with local Java %s...\n' "$java_major"
(cd "$DEBUG_REPO_DIR" && MAVEN_OPTS="${MAVEN_OPTS:-} -Djdk.xml.maxGeneralEntitySizeLimit=1000000 -Djdk.xml.totalEntitySizeLimit=1000000" ./mvnw --batch-mode --no-transfer-progress -DskipTests package)
DEBUG_PLUGIN="${DEBUG_REPO_DIR}/com.microsoft.java.debug.plugin/target/com.microsoft.java.debug.plugin-${JAVA_DEBUG_VERSION}.jar"
[[ -f "$DEBUG_PLUGIN" ]] || fail "java-debug build did not produce expected plugin: $DEBUG_PLUGIN"

printf 'Building Intelli-gen...\n'
(cd "$ROOT_DIR" && cargo build --release)
APP_BINARY="${ROOT_DIR}/target/release/intelli-gen"
[[ -x "$APP_BINARY" ]] || fail "release executable was not produced at $APP_BINARY"

PACKAGE_DIR="${STAGE_DIR}/package"
APP_BUNDLE="${PACKAGE_DIR}/Intelli-gen.app"
CONTENTS_DIR="${APP_BUNDLE}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
APP_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT_DIR}/Cargo.toml")"
[[ -n "$APP_VERSION" ]] || fail "could not read Intelli-gen version from Cargo.toml"
mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}/jdtls" "${RESOURCES_DIR}/java-debug" "${RESOURCES_DIR}/licenses"
cp "$APP_BINARY" "${MACOS_DIR}/intelli-gen"
cp -R "${JDT_EXTRACT_DIR}/." "${RESOURCES_DIR}/jdtls/"
cp "$DEBUG_PLUGIN" "${RESOURCES_DIR}/java-debug/"

cat > "${CONTENTS_DIR}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>intelli-gen</string>
  <key>CFBundleIdentifier</key><string>dev.intelli-gen.workbench</string>
  <key>CFBundleName</key><string>Intelli-gen</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${APP_VERSION}</string>
  <key>CFBundleVersion</key><string>${APP_VERSION}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
ln -s "Intelli-gen.app/Contents/MacOS/intelli-gen" "${PACKAGE_DIR}/intelli-gen"

copy_upstream_notices() {
  local source_root="$1" destination="$2"
  local found=0 file relative
  while IFS= read -r -d '' file; do
    relative="${file#"${source_root}/"}"
    mkdir -p "${destination}/$(dirname "$relative")"
    cp "$file" "${destination}/${relative}"
    found=1
  done < <(find "$source_root" -type f \( -iname 'license' -o -iname 'license.*' -o -iname 'notice' -o -iname 'notice.*' \) -print0)
  (( found == 1 )) || fail "upstream license/notice files were not found under $source_root"
}
mkdir -p "${RESOURCES_DIR}/licenses/eclipse-jdtls"
cp "$JDTLS_LICENSE_PATH" "${RESOURCES_DIR}/licenses/eclipse-jdtls/LICENSE"
copy_upstream_notices "$DEBUG_REPO_DIR" "${RESOURCES_DIR}/licenses/microsoft-java-debug"
[[ -f "${DEBUG_REPO_DIR}/LICENSE.txt" ]] || fail "Microsoft java-debug LICENSE.txt is missing"
[[ -f "${DEBUG_REPO_DIR}/ThirdPartyNotices.txt" ]] || fail "Microsoft java-debug ThirdPartyNotices.txt is missing"
cp "${DEBUG_REPO_DIR}/LICENSE.txt" "${RESOURCES_DIR}/licenses/microsoft-java-debug/LICENSE.txt"
cp "${DEBUG_REPO_DIR}/ThirdPartyNotices.txt" "${RESOURCES_DIR}/licenses/microsoft-java-debug/ThirdPartyNotices.txt"

cat > "${RESOURCES_DIR}/licenses/README.txt" <<EOF
Upstream licensing and notice files are grouped by project and retain their
relative paths.
Eclipse JDT Language Server ${JDTLS_VERSION}: EPL-2.0.
Microsoft java-debug ${JAVA_DEBUG_VERSION}: EPL-1.0.
EOF

[[ -f "${RESOURCES_DIR}/jdtls/config_mac/config.ini" ]] || fail "staged JDT LS macOS configuration is missing"
find "${RESOURCES_DIR}/jdtls/plugins" -maxdepth 1 -name 'org.eclipse.equinox.launcher_*.jar' -print -quit | grep -q . || fail "staged JDT LS Equinox launcher JAR is missing"
[[ -f "${RESOURCES_DIR}/java-debug/com.microsoft.java.debug.plugin-${JAVA_DEBUG_VERSION}.jar" ]] || fail "staged Java Debug plugin is missing"
[[ -f "${RESOURCES_DIR}/licenses/microsoft-java-debug/LICENSE.txt" ]] || fail "staged Microsoft java-debug LICENSE.txt is missing"
[[ -f "${RESOURCES_DIR}/licenses/microsoft-java-debug/ThirdPartyNotices.txt" ]] || fail "staged Microsoft java-debug ThirdPartyNotices.txt is missing"
# Cargo's Mach-O executable has an ad-hoc signature; seal the completed bundle
# after every resource has been copied so the signature remains valid.
codesign --force --deep --sign - "$APP_BUNDLE"
codesign --verify --deep --strict "$APP_BUNDLE"

# Serialize publication; package builds may stage concurrently.
lock_attempt=0
until mkdir "$PUBLISH_LOCK" 2>/dev/null; do
  (( lock_attempt += 1 ))
  (( lock_attempt < 60 )) || fail "another package publisher holds $PUBLISH_LOCK; remove it only if no packager is running"
  sleep 1
done
LOCK_HELD=1
PACKAGE_VERSION="intelli-gen-${JDTLS_VERSION}-${JAVA_DEBUG_VERSION}-$(date +%Y%m%d%H%M%S)-$$"
PACKAGE_STORE="${ROOT_DIR}/.dist-packages"
mkdir -p "$PACKAGE_STORE"
VERSIONED_PACKAGE="${PACKAGE_STORE}/${PACKAGE_VERSION}"
mv "$PACKAGE_DIR" "$VERSIONED_PACKAGE"
rm -rf -- "$STAGE_DIR"
STAGE_DIR=""

# Preserve a preexisting real directory during the one-time link migration.
if [[ -e "$DIST_DIR" && ! -L "$DIST_DIR" ]]; then
  MIGRATED_DIST="${PACKAGE_STORE}/legacy-$(date +%Y%m%d%H%M%S)-$$"
  mv "$DIST_DIR" "$MIGRATED_DIST"
fi
TEMP_LINK="${ROOT_DIR}/.dist-link.$$"
ln -s ".dist-packages/${PACKAGE_VERSION}" "$TEMP_LINK"
mv -fh "$TEMP_LINK" "$DIST_DIR"
MIGRATED_DIST=""
PUBLISHED=1
rmdir "$PUBLISH_LOCK"
LOCK_HELD=0

printf 'Package ready: %s\n' "$DIST_DIR"
