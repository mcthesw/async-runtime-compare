#!/usr/bin/env bash
set -euo pipefail

cargo build --release -p socks5-tokio -p socks5-compio -p socks5-bench
"$(dirname "$0")/../target/release/socks5-bench" "$@"
