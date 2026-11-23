#!/usr/bin/env bash
# Builds a release tarball in dist/ with the binary, install.sh, the icon and the README.
# Lints and tests first; stops on any warning or failure.
#
#   scripts/release.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
name="kaminoneko-$version-$(uname -m)-linux"
echo "== Kaminoneko $version"

cargo clippy --release --all-targets --quiet -- -D warnings
cargo test --release --quiet
cargo build --release --quiet

rm -rf "dist/$name" "dist/$name.tar.gz" "dist/$name.tar.gz.sha256"
mkdir -p "dist/$name"
install -m 755 target/release/kaminoneko scripts/install.sh "dist/$name/"
install -m 644 assets/kaminoneko.svg README.md "dist/$name/"
tar -C dist --owner=0 --group=0 -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")

echo "== dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
cat "dist/$name.tar.gz.sha256"
echo "Install: tar xzf $name.tar.gz && $name/install.sh"
