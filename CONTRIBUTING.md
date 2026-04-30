# Contributing

Thanks for taking the time. A few things to know before you send a PR.

## What this project is

cargo-hatch is a focused tool — fast dependency management for Rust. Before adding a feature, open an issue first so we can agree on whether it fits the scope. Bug fixes and performance improvements are always welcome without prior discussion.

## Setup

```sh
git clone https://github.com/vunholy/cargo-hatch
cd cargo-hatch
cargo build
cargo test
```

Tests are self-contained and do not require network access (the ones that do are gated behind `api.get()` calls that fall through to the local index cache).

## Guidelines

**Keep it fast.** The whole point of this tool is speed. If your change adds latency to the common path, benchmark it with hyperfine before opening a PR.

**No unnecessary dependencies.** Every new crate is a compile-time and supply-chain cost. If the standard library or an already-present dependency can do the job, use that.

**No comments explaining what the code does.** Only write a comment if the *why* is non-obvious — a hidden constraint, a subtle invariant, a workaround for a specific bug. Names should speak for themselves.

**Tests for non-trivial logic.** Cache fingerprinting, manifest editing, index parsing, and LRU eviction all have tests. New logic in those areas should too.

## Submitting a PR

- One logical change per PR
- Include a short description of what the change does and why
- Run `cargo test` and `cargo clippy` before pushing
- If the change affects performance, include hyperfine output

## Reporting bugs

Open an issue with the command you ran, the output you got, and the output you expected. Include your OS, rustc version, and cargo version.
