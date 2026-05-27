#!/usr/bin/env bash
set -euo pipefail

pkgname="drmcru"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
target_triple="${TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
dist_dir="dist"

if [[ -z "$version" ]]; then
    echo "Could not read package version from Cargo.toml" >&2
    exit 1
fi

mkdir -p "$dist_dir"

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo package --allow-dirty --no-verify --offline
cargo build --release --locked

crate_path="target/package/${pkgname}-${version}.crate"
source_tarball="${dist_dir}/${pkgname}-${version}.tar.gz"
binary_path="${dist_dir}/${pkgname}-${version}-${target_triple}"
checksums="${dist_dir}/SHA256SUMS"

cp "$crate_path" "$source_tarball"
cp "target/release/${pkgname}" "$binary_path"

(
    cd "$dist_dir"
    sha256sum \
        "${pkgname}-${version}.tar.gz" \
        "${pkgname}-${version}-${target_triple}" \
        > "SHA256SUMS"
)

cat <<EOF
Created release artifacts:
  ${source_tarball}
  ${binary_path}
  ${checksums}

Verify checksums with: (cd ${dist_dir} && sha256sum -c SHA256SUMS)
EOF
