use anyhow::Result;
use std::sync::Arc;

use crate::{
    cache::{Cache, CacheKey, FeatureSet},
    commands::Command,
    compiler::NativeBuilder,
    crates::CratesAPI,
    manifest::Manifest,
    progress::{CompileProgress, DiagnosticCollector},
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
pub struct Add {
    crates: Vec<(String, Option<String>)>, // (name, version)
    features: Vec<String>,
    precompile: bool,
    verbose: bool,
}

impl Command for Add {
    fn feed(&mut self, arguments: Vec<String>) {
        let mut args = arguments.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--precompile" => self.precompile = true,
                "--verbose" | "-v" => self.verbose = true,
                "--features" | "-F" => {
                    if let Some(feats) = args.next() {
                        self.features = feats.split(',').map(String::from).collect();
                        self.features.sort();
                    }
                }
                s if !s.starts_with('-') => {
                    if let Some((name, ver)) = s.split_once('@') {
                        self.crates.push((name.to_string(), Some(ver.to_string())));
                    } else {
                        self.crates.push((s.to_string(), None));
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&self) -> Result<(), String> {
        self.run_inner().map_err(|e| e.to_string())
    }
}

impl Add {
    fn run_inner(&self) -> Result<()> {
        use a::*;

        if self.crates.is_empty() {
            anyhow::bail!("no crates specified — usage: ignite add <crate> [crate2 ...]");
        }

        let names_display = self.crates.iter()
            .map(|(n, _)| format!("{BY}{n}{R}"))
            .collect::<Vec<_>>()
            .join("  ");
        println!("\t{BC}add{R}  {names_display}");

        // 1. Resolve all versions under one spinner
        let sp = Spinner::new("resolving...");
        let api = CratesAPI::new();
        let mut resolved = Vec::new();
        for (name, version) in &self.crates {
            let entry = api
                .get(name, version.as_deref())
                .ok_or_else(|| anyhow::anyhow!(
                    "version not found for '{}' — try: ignite fetch {}", name, name
                ))?;
            resolved.push((name.clone(), entry));
        }
        sp.finish_lines(resolved.iter().map(|(name, entry)| {
            format!("\t  {G}{:<13}:{R} {Y}{}{R}", name, entry.vers)
        }));

        // 2. Update manifest once for all crates
        let sp = Spinner::new("updating manifest...");
        let cwd = std::env::current_dir()?;
        let mut manifest = Manifest::load(&cwd)?;
        for (name, entry) in &resolved {
            manifest.upsert_dependency(name, &entry.vers, &self.features);
        }
        manifest.save()?;
        let n = resolved.len();
        sp.finish_with(format!(
            "\t  {G}{:<13}:{R} {Y}{} {} added{R}",
            "manifest", n, if n == 1 { "crate" } else { "crates" }
        ));

        // 3. Fetch all sources
        let sp = Spinner::new("fetching sources...");
        for (name, entry) in &resolved {
            NativeBuilder::download_source(name, &entry.vers, Some(&entry.cksum))?;
        }
        sp.finish_with(format!(
            "\t  {G}{:<13}:{R} {Y}{} {} fetched{R}",
            "source", n, if n == 1 { "crate" } else { "crates" }
        ));

        // 4. Optional precompile (per crate)
        if self.precompile {
            let rustc_ver = NativeBuilder::rustc_version()?;
            let target = NativeBuilder::target_triple()?;
            let cache = Cache::new(self.verbose)?;

            for (name, entry) in &resolved {
                let feature_set = if self.features.is_empty() {
                    FeatureSet::All
                } else {
                    FeatureSet::Explicit(self.features.clone())
                };
                let key = CacheKey {
                    rustc_version: &rustc_ver,
                    crate_name: name,
                    crate_version: &entry.vers,
                    target_triple: &target,
                    features: &feature_set,
                };
                let fp = Cache::fingerprint(&key);

                if cache.lookup(&fp).is_some() {
                    println!("\t  {G}{:<13}:{R} {BM}hit ✓{R}", name);
                    continue;
                }

                let features_for_compile = if self.features.is_empty() {
                    entry.features.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
                } else {
                    self.features.clone()
                };

                let label = name.clone();
                let progress = Arc::new(CompileProgress::new(0));
                let diag = DiagnosticCollector::new(self.verbose);
                let sp = Spinner::with_progress(format!("compiling {name}..."), Arc::clone(&progress));
                let builder = NativeBuilder::new(self.verbose)?;
                builder.precompile_lib(
                    name,
                    &entry.vers,
                    &features_for_compile,
                    &fp,
                    &cache,
                    &progress,
                    &diag,
                )?;
                cache.evict_if_needed()?;
                sp.finish_with(format!("\t  {G}{:<13}:{R} {Y}compiled{R}", label));
                diag.drain_pretty();
            }
        }

        Ok(())
    }
}
