#!/bin/bash
set -euo pipefail

if [[ "$(head -2 Cargo.toml)" != '[package]
name = "multibg-wayland"' ]]; then
    echo 'Not in crate root'
    exit 1
fi

version=$(cargo pkgid | cut -d '#' -f2)
crate="target/package/multibg-wayland-$version.crate"
sum=$(sha256sum "$crate" | cut -d ' ' -f1)

if [[ PKGBUILD -nt "$crate" ]]; then
    echo 'Nothing to do'
    exit 1
fi

sed -e "s/@pkgver@/$version/" -e "s/@sha256sum@/$sum/" < PKGBUILD.in > PKGBUILD
