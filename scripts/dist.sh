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
cargo build --release --locked --target "$target_triple"

binary_path="${dist_dir}/${pkgname}-${version}-${target_triple}"
checksums="${dist_dir}/SHA256SUMS"

cp "target/${target_triple}/release/${pkgname}" "$binary_path"

(
    cd "$dist_dir"
    sha256sum "${pkgname}-${version}-${target_triple}" > "SHA256SUMS"
)

cat <<EOF
Created release artifacts:
  ${binary_path}
  ${checksums}

Verify checksums with: (cd ${dist_dir} && sha256sum -c SHA256SUMS)
EOF
