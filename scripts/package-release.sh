#!/bin/sh
set -eu
version=${1:?usage: package-release.sh VERSION TARGET}
target=${2:?usage: package-release.sh VERSION TARGET}
case "$version" in *[!0-9A-Za-z.+-]*|'') exit 2;; esac
case "$target" in x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;; *) echo 'Unsupported release target' >&2; exit 2;; esac
binary="target/$target/release/aic"
if [ -n "${AIC_RELEASE_RUNNER:-}" ]; then
  "$AIC_RELEASE_RUNNER" "$binary" --help >/dev/null
else
  "$binary" --help >/dev/null
fi
mkdir -p dist
archive="aic-$version-$target.tar.gz"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT HUP INT TERM
cp "$binary" "$staging/aic"
chmod 755 "$staging/aic"
tar -czf "dist/$archive" -C "$staging" aic
(cd dist && sha256sum "$archive" > "$archive.sha256" && sha256sum -c "$archive.sha256")
