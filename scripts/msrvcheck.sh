#!/bin/bash
# rustup toolchain uninstall 1.80-x86_64-unknown-linux-gnu
# rustup toolchain install 1.82-x86_64-unknown-linux-gnu
# rustup +1.82-x86_64-unknown-linux-gnu target add aarch64-unknown-linux-gnu x86_64-unknown-freebsd
set -Ceuxo pipefail
toolchain=1.88-x86_64-unknown-linux-gnu
cargo +${toolchain} clippy --target=x86_64-unknown-linux-gnu --features=avif
cargo +${toolchain} clippy --target=aarch64-unknown-linux-gnu
cargo +${toolchain} clippy --target=x86_64-unknown-freebsd
