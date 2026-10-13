#!/bin/sh
set -eu

repo=${AIC_REPOSITORY:-hedibertosilva/aic}
install_dir=${AIC_INSTALL_DIR:-"$HOME/.local/bin"}
requested_version=${1:-}

case "$(uname -s)" in
  Linux) ;;
  *)
    echo "aic releases currently support Linux only." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64|amd64) ;;
  *)
    echo "The current release publishes a Linux x86_64 binary only." >&2
    exit 1
    ;;
esac

if [ -n "$requested_version" ]; then
  version=${requested_version#v}
else
  release_url=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
    "https://github.com/$repo/releases/latest")
  tag=${release_url##*/}
  version=${tag#v}
fi

case "$version" in
  ''|*[!0-9A-Za-z.+-]*)
    echo "Invalid version: $version" >&2
    exit 1
    ;;
esac

asset="aic"
base_url="https://github.com/$repo/releases/download/$version"
tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

echo "Downloading aic v$version..."
curl -fsSL --retry 3 -o "$tmp_dir/$asset" "$base_url/$asset"
chmod 755 "$tmp_dir/$asset"
"$tmp_dir/$asset" --version >/dev/null
mkdir -p "$install_dir"
install -m 755 "$tmp_dir/$asset" "$install_dir/aic"

echo "Installed aic v$version at $install_dir/aic"
case ":${PATH:-}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH if it is not already there." ;;
esac
