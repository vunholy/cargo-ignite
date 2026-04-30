use std::fs;
use std::path::PathBuf;

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hatch-integ-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
    dir
}

/// hatch add serde — should update Cargo.toml and not error
#[test]
#[ignore = "requires network"]
fn test_add_updates_cargo_toml() {
    let dir = fixture_project("add-serde");
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let api = cargo_hatch::crates::CratesAPI::new();
    let entry = api.get("serde", None).expect("should resolve serde");

    let mut manifest = cargo_hatch::manifest::Manifest::load(&dir).unwrap();
    manifest.upsert_dependency("serde", &entry.vers, &[]);
    manifest.save().unwrap();

    let content = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(content.contains("serde"), "Cargo.toml should contain serde");
    assert!(content.contains(&entry.vers), "Cargo.toml should contain version");

    std::env::set_current_dir(original_dir).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// hatch add serde --precompile twice: second call should be a cache hit
#[test]
#[ignore = "requires network and rustc"]
fn test_precompile_cache_hit_on_second_call() {
    use cargo_hatch::cache::{Cache, CacheKey, FeatureSet};
    use cargo_hatch::compiler::Compiler;

    let api = cargo_hatch::crates::CratesAPI::new();
    let entry = api.get("memchr", Some("2.7.4")).expect("should resolve memchr 2.7.4");

    let rustc_ver = Compiler::rustc_version().unwrap();
    let target = Compiler::target_triple().unwrap();
    let fs = FeatureSet::All;
    let key = CacheKey {
        rustc_version: &rustc_ver,
        crate_name: "memchr",
        crate_version: &entry.vers,
        target_triple: &target,
        features: &fs,
    };
    let fp = Cache::fingerprint(&key);
    let cache = Cache::new(false).unwrap();

    let was_cached_before = cache.lookup(&fp).is_some();
    if !was_cached_before {
        let compiler = Compiler::detect(false).unwrap();
        compiler.precompile_lib("memchr", &entry.vers, &[], &fp, &cache, false).unwrap();
    }

    let hit = cache.lookup(&fp);
    assert!(hit.is_some(), "second lookup should be a cache hit");
}

/// Build.rs detection: add a crate with build.rs without cargo-construct.
#[test]
#[ignore = "requires network"]
fn test_add_with_build_rs_updates_manifest() {
    let dir = fixture_project("add-build-rs");
    let api = cargo_hatch::crates::CratesAPI::new();
    let entry = api.get("cc", None).expect("should resolve cc");

    let mut manifest = cargo_hatch::manifest::Manifest::load(&dir).unwrap();
    manifest.upsert_dependency("cc", &entry.vers, &[]);
    manifest.save().unwrap();

    let content = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(content.contains("cc"), "Cargo.toml should have cc even for build-script crates");
    let _ = fs::remove_dir_all(&dir);
}
