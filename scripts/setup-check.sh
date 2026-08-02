#!/usr/bin/env sh
# setup-check.sh — checks a contributor's machine has what Canary needs,
# before they run their first `cargo build`. Deliberately dependency-free
# (POSIX sh, no Cargo/Rust required to run this itself) — see
# docs/development/build-system.md and scripts/README.md.

set -eu

ok=0
warn=0
fail=0

pass() { printf '  [ok]   %s\n' "$1"; ok=$((ok + 1)); }
warning() { printf '  [warn] %s\n' "$1"; warn=$((warn + 1)); }
fail_check() { printf '  [fail] %s\n' "$1"; fail=$((fail + 1)); }

echo "Canary Engine — environment check"
echo "=================================="

# --- git -------------------------------------------------------------
if command -v git >/dev/null 2>&1; then
    pass "git found ($(git --version))"
else
    fail_check "git not found — required to clone/contribute"
fi

# --- rustc / cargo -----------------------------------------------------
if command -v rustc >/dev/null 2>&1; then
    pass "rustc found ($(rustc --version))"
else
    fail_check "rustc not found — install via https://rustup.rs (or your package manager)"
fi

if command -v cargo >/dev/null 2>&1; then
    pass "cargo found ($(cargo --version))"
else
    fail_check "cargo not found — install via https://rustup.rs (or your package manager)"
fi

# --- rustup (optional but recommended, for rust-toolchain.toml) --------
if command -v rustup >/dev/null 2>&1; then
    pass "rustup found — rust-toolchain.toml will be honored automatically"
else
    warning "rustup not found — rust-toolchain.toml components/targets (rustfmt, clippy, wasm32-wasip2) won't be installed automatically; install them manually if your rustc/cargo didn't come from rustup"
fi

# --- wasm32-wasip2 target (only meaningful if rustup is present) -------
if command -v rustup >/dev/null 2>&1; then
    if rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2; then
        pass "wasm32-wasip2 target installed"
    else
        warning "wasm32-wasip2 target not installed — run: rustup target add wasm32-wasip2"
    fi
fi

echo "=================================="
echo "ok: $ok   warn: $warn   fail: $fail"

if [ "$fail" -gt 0 ]; then
    echo "Fix the [fail] items above before running 'cargo build --workspace'."
    exit 1
fi

echo "Looks good — try: cargo build --workspace && cargo test --workspace"
