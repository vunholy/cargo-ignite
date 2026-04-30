# cargo-ignite

[![crates.io](https://img.shields.io/crates/v/cargo-ignite.svg)](https://crates.io/crates/cargo-ignite)

Fast dependency management for Rust. Reads the crates.io sparse index directly instead of going through cargo, so version resolution is near-instant.

```bash
$ ignite add serde tokio@1.44 serde_json
$ ignite install ripgrep
$ ignite remove serde_json
$ ignite fetch tokio --metadata
```

---

## Benchmarks

Measured with [hyperfine](https://github.com/sharkdp/hyperfine). All runs use a warm disk cache (index files and source tarballs already local). `--warmup 3 --runs 12` for `add`, `--warmup 3 --runs 15` for `fetch`/`info`.

Bench project: minimal `cargo new --bin` with no existing dependencies. Each run resets `Cargo.toml` to its initial state via hyperfine's `--prepare`.

### `ignite add` vs `cargo add` — Windows 11

System: AMD Ryzen 5 5500, WD Green SATA SSD, Windows 11 Pro, rustc 1.95.0

| Crate | `cargo add` (mean) | `ignite add` (mean) | Speedup |
|---|---|---|---|
| serde | 486 ms | 14.6 ms | **33×** |
| tokio | 514 ms | 16.0 ms | **32×** |
| axum | 574 ms | 20.6 ms | **28×** |
| clap | 544 ms | 12.0 ms | **45×** |
| reqwest | 576 ms | 22.0 ms | **26×** |
| diesel | 528 ms | 11.6 ms | **45×** |
| actix-web | 581 ms | 20.5 ms | **28×** |

### `ignite fetch` vs `cargo info` — Windows 11

| Command | Mean | Speedup |
|---|---|---|
| `cargo info serde` | 149 ms | — |
| `ignite fetch serde` | 13.0 ms | **11×** |

`cargo info` initializes cargo's full subsystem even for a read-only query. `ignite fetch` reads one file from disk and parses one JSON line.

---

### `ignite add` vs `cargo add` — Linux (WSL2, Fedora 42, kernel 6.6)

Same methodology. ignite binary compiled natively on Linux (GCC 15.2, x86_64).

| Crate | `cargo add` (mean) | `ignite add` (mean) | Speedup |
|---|---|---|---|
| serde | 418 ms | 16.0 ms | **26×** |
| tokio | 440 ms | 16.1 ms | **27×** |
| axum | 408 ms | 15.0 ms | **27×** |
| clap | 416 ms | 15.2 ms | **27×** |
| reqwest | 448 ms | 14.1 ms | **32×** |
| diesel | 400 ms | 15.1 ms | **27×** |
| actix-web | 520 ms | 15.2 ms | **34×** |

### `ignite fetch` vs `cargo info` — Linux

| Command | Mean | Speedup |
|---|---|---|
| `cargo info serde` | 80 ms | — |
| `ignite fetch serde` | 12.5 ms | **6×** |

`cargo info` is faster on Linux than Windows (no PE loader overhead), but ignite still wins by reading the local index cache directly with no cargo startup cost.

---

## Why it's faster

When you run `cargo add serde`, cargo:

1. Locates and parses your workspace manifest
2. Initializes its internal package registry and resolver
3. Loads your `Cargo.lock` and resolves dependency constraints
4. Validates feature compatibility across the full graph
5. Finally writes the new line to `Cargo.toml`

Steps 1-4 are the same regardless of whether you're adding one crate or ten. They exist because cargo is a general build system, not a manifest editor.

`ignite add` instead:

1. Reads the crates.io sparse index file directly from disk (the same file cargo caches at `~/.cargo/registry/index/`)
2. Parses the last stable, non-yanked version using SIMD-accelerated line scanning
3. Appends the entry to `[dependencies]` using `toml_edit` (preserves formatting and comments)
4. Downloads the source tarball to `~/.cargo-construct/src/` if not already cached

Steps 1-3 take about 12 ms. The source download (step 4) is skipped on cache hit.

### What ignite does not do

ignite does **not** run cargo's full semver resolver. It picks the latest stable version of each crate and resolves transitive dependencies to their own latest stable. This means:

- Version constraints declared by a crate's dependencies are read but not enforced across the full graph
- If two crates need conflicting versions of a shared dependency, ignite will not detect this — cargo will catch it on the next `cargo build`
- Features are not validated across the dependency graph

This is intentional. ignite's job is to get the right line into `Cargo.toml` fast. Cargo owns the full resolution step.

---

### Reproducing on Alpine Linux via QEMU

The Linux numbers above were collected inside WSL2 (Fedora 42 container, Linux 6.6 kernel). To reproduce on Alpine Linux with QEMU using the ISO at `C:\Users\dmsve\Downloads\alpine-standard-3.23.4-x86_64.iso`:

```powershell
# 1. Create a disk image
cd $env:TEMP
& "C:\Program Files\qemu\qemu-img.exe" create -f qcow2 alpine.qcow2 8G

# 2. Boot from ISO (text console)
& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
  -m 2048 -smp 4 `
  -cdrom "C:\Users\dmsve\Downloads\alpine-standard-3.23.4-x86_64.iso" `
  -drive file=alpine.qcow2,if=virtio `
  -netdev user,id=net0 -device virtio-net-pci,netdev=net0 `
  -nographic -serial stdio
```

Inside Alpine (login as `root`, no password):

```sh
# Install Alpine to disk
setup-alpine    # accept defaults; when asked for disk, enter 'vda', use 'sys' mode
reboot
```

```powershell
# 3. Boot the installed disk
& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
  -m 2048 -smp 4 `
  -drive file=alpine.qcow2,if=virtio `
  -netdev user,id=net0 -device virtio-net-pci,netdev=net0 `
  -nographic -serial stdio
```

```sh
# 4. Inside installed Alpine: install dependencies
apk add curl gcc musl-dev git

# 5. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# 6. Build and install ignite
git clone https://github.com/vunholy/cargo-ignite
cd cargo-ignite
cargo build --release
cp target/release/ignite ~/.cargo/bin/

# 7. Install hyperfine
cargo install hyperfine

# 8. Run benchmarks
cargo new --bin bench && cd bench && cp Cargo.toml Cargo.toml.orig

hyperfine --warmup 3 --runs 12 \
  --prepare "cp Cargo.toml.orig Cargo.toml" \
  "cargo add serde -q" \
  "ignite add serde"
```

---

## Commands

### `ignite add <crate[@version]> [crate2 ...] [--features f1,f2] [--precompile]`

Adds crates to `Cargo.toml`. Resolves to latest stable unless pinned with `@`.

```sh
ignite add serde
ignite add tokio@1.44.0 --features full
ignite add serde serde_json tokio
ignite add serde --precompile
```

`--precompile` compiles the full dependency tree with `rustc` and stores `.rlib` artifacts in `~/.cargo-construct/cache/`. The next `cargo build` in a project that shares crate + version + rustc + target can link against these cached artifacts directly.

Crates with a `build.rs` are skipped during precompile (they require cargo's build script runner). ignite prints a warning and continues.

### `ignite install <crate> [version] [--features f1,f2]`

Compiles and installs a binary crate to `~/.cargo/bin/`. Rejects library crates (no `src/main.rs`) with a clear error message. On a cache hit, the binary is copied in without recompiling.

```sh
ignite install ripgrep
ignite install bat 0.24.0
```

### `ignite remove <crate> [crate2 ...]`

Removes crates from `[dependencies]`. Partially succeeds: removes what it finds, prints "not found" for anything missing, and only errors if nothing matched at all.

```sh
ignite remove serde
ignite remove serde serde_json tokio
```

### `ignite fetch <crate> [version] [--metadata]`

Reads crate info from the local index without touching `Cargo.toml`.

```sh
ignite fetch tokio
ignite fetch serde 1.0.219 --metadata
```

### `ignite help`

Prints command reference.

---

## Cache architecture

### Index cache (read-only reuse of cargo's cache)

ignite reads crate metadata from `~/.cargo/registry/index/index.crates.io-*/cache/`. This is the same directory cargo writes when it fetches index data. ignite does not maintain a separate index copy.

Each file in that directory is a newline-delimited stream of JSON objects, one per published version. ignite scans these lines with [memchr](https://github.com/BurntSushi/memchr)'s `memmem::Finder` for SIMD-accelerated substring search, then parses the matched line with [simd-json](https://github.com/simd-lite/simd-json).

For "latest version" queries (no `@` pin), ignite checks whether the cached file is older than 24 hours and fetches a fresh copy if so. For pinned-version queries, it never refreshes — a specific version's content on crates.io is immutable.

### Source cache (`~/.cargo-construct/src/`)

When `ignite add` or `ignite install` downloads a source tarball, it extracts it to `~/.cargo-construct/src/<name>-<version>/` and also writes the raw `.crate` bytes into cargo's own registry cache (`~/.cargo/registry/cache/.../`) so that a subsequent `cargo build` finds it there without re-downloading.

The write to cargo's registry happens only after a successful extraction, to avoid leaving a corrupt `.crate` file in cargo's cache on a failed download.

### Artifact cache (`~/.cargo-construct/cache/<fingerprint>/`)

Used by `ignite add --precompile` and `ignite install`. The cache key is a BLAKE3 hash of:

```
rustc_version\x00crate_name\x00crate_version\x00target_triple\x00features
```

Null bytes separate fields to prevent length-extension collisions where two different inputs would otherwise hash to the same key (e.g. `"fo"+"obar"` vs `"foo"+"bar"`).

`features` is serialized as the sorted, comma-joined list of explicitly requested features, or the literal string `ALL` if `--precompile` was used without `--features`. This distinguishes "compile with all features" from "compile with none".

Two index files track the artifact store:

- `ignite-index.json` — flat `{fingerprint: entry_path}` map for O(1) lookup
- `lru.json` — `{fingerprint: unix_timestamp}` for LRU eviction

Both are written atomically (write to a `.tmp` file, then rename into place) so a process crash mid-write does not corrupt them.

Default eviction limit: 10 GB of artifact data. Metadata files do not count toward the limit.

---

## Installation

```sh
git clone https://github.com/vunholy/cargo-ignite
cd cargo-ignite
cargo install --path .
```

Produces two binaries, `cargo-ignite` and `ignite`, both pointing to the same binary. Either works.

---

## License

[PolyForm Noncommercial License 1.0.0](LICENSE) — free for personal, educational, and noncommercial open-source use.

Required Notice: Copyright vunholy (https://github.com/vunholy)
