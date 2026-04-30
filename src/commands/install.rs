use anyhow::Result;
use cargo_toml::Manifest;
use std::path::PathBuf;

use crate::{
    cache::{Cache, CacheKey, FeatureSet},
    commands::Command,
    compiler::Compiler,
    crates::CratesAPI,
    spinner::Spinner,
};

mod a {
    pub const R: &str = "\x1b[0m";
    pub const G: &str = "\x1b[37m";
    pub const Y: &str = "\x1b[33m";
    pub const BC: &str = "\x1b[1;36m";
    pub const BY: &str = "\x1b[1;33m";
    pub const BM: &str = "\x1b[1;35m";
}

#[derive(Default)]
pub struct Install {
    name: String,
    version: Option<String>,
    features: Vec<String>,
    verbose: bool,
}

impl Command for Install {
    fn feed(&mut self, arguments: Vec<String>) {
        let mut args = arguments.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--verbose" | "-v" => self.verbose = true,
                "--features" | "-F" => {
                    if let Some(feats) = args.next() {
                        self.features = feats.split(',').map(String::from).collect();
                        self.features.sort();
                    }
                }
                _ if self.name.is_empty() => self.name = arg,
                _ if self.version.is_none() => self.version = Some(arg),
                _ => {}
            }
        }
    }

    fn run(&self) -> Result<(), String> {
        self.run_inner().map_err(|e| e.to_string())
    }
}

impl Install {
    fn run_inner(&self) -> Result<()> {
        use a::*;

        println!("\t{BC}install{R}  {BY}{}{R}", self.name);

        // 1. Resolve version
        let sp = Spinner::new("resolving...");
        let api = CratesAPI::new();
        let entry = api
            .get(&self.name, self.version.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "version not found for '{}' — try: ignite fetch {}",
                    self.name,
                    self.name
                )
            })?;
        sp.finish_with(format!("\t  {G}version      :{R} {Y}{}{R}", entry.vers));

        // 2. Verify this is a binary crate (download source — fast if already cached)
        let src_dir = Compiler::download_source(&self.name, &entry.vers, Some(&entry.cksum))?;
        let manifest_path = src_dir.join("Cargo.toml");
        let manifest = Manifest::from_path(&manifest_path)?;

        let has_bin = !manifest.bin.is_empty()
            || manifest
                .package
                .as_ref()
                .map(|p| p.default_run.is_some())
                .unwrap_or(false)
            || src_dir.join("src/bin").exists();

        if !has_bin {
            anyhow::bail!(
                "'{}' is a library crate — use `ignite add {}` to add it as a dependency",
                self.name,
                self.name
            );
        }

        // 3. Fingerprint + cache lookup
        let rustc_ver = Compiler::rustc_version()?;
        let target = Compiler::target_triple()?;
        let feature_set = if self.features.is_empty() {
            FeatureSet::All
        } else {
            FeatureSet::Explicit(self.features.clone())
        };
        let key = CacheKey {
            rustc_version: &rustc_ver,
            crate_name: &self.name,
            crate_version: &entry.vers,
            target_triple: &target,
            features: &feature_set,
        };
        let fp = Cache::fingerprint(&key);
        let cache = Cache::new(self.verbose)?;

        #[cfg(windows)]
        let bin_name = format!("{}.exe", self.name);
        #[cfg(not(windows))]
        let bin_name = self.name.clone();

        let cargo_bin = cargo_bin_dir()?;

        // Fast path: cache hit
        if let Some(artifacts_dir) = cache.lookup(&fp) {
            let bin_src = artifacts_dir.join(&bin_name);
            if bin_src.exists() {
                println!("\t  {G}cache        :{R} {BM}hit ✓{R}");
                let sp = Spinner::new("installing...");
                std::fs::copy(&bin_src, cargo_bin.join(&bin_name))?;
                sp.finish_with(format!(
                    "\t  {G}installed    :{R} {Y}{}{R}",
                    cargo_bin.join(&bin_name).display()
                ));
                return Ok(());
            }
        }

        // Cache miss: compile
        println!("\t  {G}cache        :{R} {Y}miss → compiling{R}");

        let features_for_compile = if self.features.is_empty() {
            entry
                .features
                .iter()
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>()
        } else {
            self.features.clone()
        };

        let sp = Spinner::new("compiling...");
        let compiler = Compiler::detect(self.verbose)?;
        let bin_path = compiler.install_bin(
            &self.name,
            &entry.vers,
            Some(&entry.cksum),
            &features_for_compile,
            &fp,
            &cache,
            self.verbose,
        )?;

        std::fs::create_dir_all(&cargo_bin)?;
        std::fs::copy(&bin_path, cargo_bin.join(&bin_name))?;
        cache.evict_if_needed()?;
        sp.finish_with(format!(
            "\t  {G}installed    :{R} {Y}{}{R}",
            cargo_bin.join(&bin_name).display()
        ));

        Ok(())
    }
}

fn cargo_bin_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CARGO_HOME") {
        return Ok(PathBuf::from(p).join("bin"));
    }
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").map_err(|_| anyhow::anyhow!("USERPROFILE not set"))?;
    #[cfg(not(windows))]
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".cargo").join("bin"))
}
