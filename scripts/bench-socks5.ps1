$ErrorActionPreference = "Stop"

cargo build --release -p socks5-tokio -p socks5-compio -p socks5-bench
& "$PSScriptRoot\..\target\release\socks5-bench.exe" @args
