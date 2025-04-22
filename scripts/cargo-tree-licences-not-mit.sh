#!/bin/bash
set -euo pipefail
cargo tree --prefix none --format '{p} {l}' | grep --invert-match -e 'MIT'
