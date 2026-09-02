#!/bin/bash
set -euo pipefail
dir="/tmp/depdiff-$(date --utc '+%s%N')"
mkdir "$dir"
cp -r . "$dir/crate"
cd "$dir/crate"
cargo vendor --locked "$dir/new" >/dev/null 2>&1
git checkout HEAD^ >/dev/null 2>&1
cargo vendor --locked "$dir/old" >/dev/null 2>&1
cd "$dir"
set +e
diff -dru0 --unidirectional-new-file \
    --exclude=Cargo.lock \
    --exclude=.cargo-checksum.json \
    --exclude=.cargo_vcs_info.json \
    old new \
| sed \
    -e 's/^diff.*/+--------------------------------------------------------------------------------/' \
    -e '/^Only in .*/d' \
    -e '/^@@.*/d' \
    -e '/^-.*/d' \
    -e 's/^+//' \
> "$dir/diff.txt"
if (( $? > 1 )); then
    echo 'diff failed'
    exit 1
fi
set -e
rm -rf "$dir/crate" "$dir/new" "$dir/old"
echo "$dir/diff.txt"
