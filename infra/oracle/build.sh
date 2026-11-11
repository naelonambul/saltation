#!/usr/bin/env bash
# Builds the Oracle Database 19c Enterprise Edition image used by Saltation.
# The installation zip is read from ORACLE_ZIP and copied only into a temporary
# build directory; it is never part of this repository.
set -euo pipefail

DOCKER_IMAGES_REPO="https://github.com/oracle/docker-images.git"
DOCKER_IMAGES_COMMIT="e10e8970b375fa8969a41469612de14c460f0166"
IMAGE_TAG="${IMAGE_TAG:-saltation/oracle-ee:19.19}"
ORACLE_ZIP="${ORACLE_ZIP:-$HOME/Library/Mobile Documents/com~apple~CloudDocs/Downloads/LINUX.ARM64_1919000_db_home.zip}"
EXPECTED_MD5="6c39043ad12e11bcdc505184631e11a2"

# Oracle's Dockerfile uses RUN --mount, which needs BuildKit.
export DOCKER_BUILDKIT=1
if ! docker buildx version >/dev/null 2>&1; then
  echo "build.sh: docker buildx (BuildKit) is required" >&2
  exit 2
fi

if [ ! -f "$ORACLE_ZIP" ]; then
  echo "build.sh: Oracle installation zip not found at ORACLE_ZIP=$ORACLE_ZIP" >&2
  exit 2
fi

md5_of() { if command -v md5sum >/dev/null; then md5sum "$1" | cut -d' ' -f1; else md5 -q "$1"; fi; }
actual="$(md5_of "$ORACLE_ZIP")"
if [ "$actual" != "$EXPECTED_MD5" ]; then
  echo "build.sh: checksum mismatch for $ORACLE_ZIP (got $actual, want $EXPECTED_MD5)" >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone --quiet --filter=blob:none "$DOCKER_IMAGES_REPO" "$work/docker-images"
git -C "$work/docker-images" checkout --quiet "$DOCKER_IMAGES_COMMIT"
dockerfiles="$work/docker-images/OracleDatabase/SingleInstance/dockerfiles"
cp "$ORACLE_ZIP" "$dockerfiles/19.3.0/LINUX.ARM64_1919000_db_home.zip"

# Upstream issue at the pinned commit: setupLinuxEnv.sh installs libxcrypt-compat,
# which exists only on Oracle Linux 9, but 19c on arm64 builds on Oracle Linux 8.
# The failed install skips oracle-database-preinstall-19c, so the oracle user is
# never created. Install libxcrypt-compat only when the package exists.
sed -i.orig 's/ openssl hostname libxcrypt-compat \&\& \\/ openssl hostname \&\& (yum -y install libxcrypt-compat || true) \&\& \\/' \
  "$dockerfiles/19.3.0/setupLinuxEnv.sh"
grep -q 'yum -y install libxcrypt-compat || true' "$dockerfiles/19.3.0/setupLinuxEnv.sh" \
  || { echo "build.sh: setupLinuxEnv.sh patch did not apply" >&2; exit 2; }

cd "$dockerfiles"
./buildContainerImage.sh -v 19.3.0 -e -t "$IMAGE_TAG"
echo "build.sh: built $IMAGE_TAG"
