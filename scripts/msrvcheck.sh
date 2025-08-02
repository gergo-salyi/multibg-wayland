#!/bin/bash
# rustup toolchain uninstall 1.80-x86_64-unknown-linux-gnu
# rustup toolchain install 1.82-x86_64-unknown-linux-gnu
# rustup +1.82-x86_64-unknown-linux-gnu target add aarch64-unknown-linux-gnu x86_64-unknown-freebsd
set -euxo pipefail
cargo +1.87-x86_64-unknown-linux-gnu check --target=x86_64-unknown-linux-gnu --features=avif
cargo +1.87-x86_64-unknown-linux-gnu check --target=aarch64-unknown-linux-gnu
cargo +1.87-x86_64-unknown-linux-gnu check --target=x86_64-unknown-freebsd
