use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct DepNode {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub direct_deps: Vec<String>,
    pub has_build_script: bool,
    pub src_dir: PathBuf,
}

pub struct BuildScriptOutput {
    pub out_dir: PathBuf,
    pub link_libs: Vec<String>,
    pub link_search: Vec<String>,
    pub cfg_flags: Vec<String>,
    pub env_vars: Vec<(String, String)>,
    pub extra_flags: Vec<String>,
}

pub struct PackageMeta {
    pub authors: Vec<String>,
    pub description: String,
}

pub struct NativeBuilder {
    pub rustc_path: PathBuf,
    pub verbose: bool,
}

fn target_os(triple: &str) -> &'static str {
    if triple.contains("windows") { "windows" }
    else if triple.contains("linux")   { "linux"   }
    else if triple.contains("darwin")  { "macos"   }
    else if triple.contains("freebsd") { "freebsd" }
    else { "unknown" }
}

fn target_arch(triple: &str) -> &'static str {
    if triple.starts_with("x86_64")  { "x86_64"  }
    else if triple.starts_with("aarch64") { "aarch64" }
    else if triple.starts_with("i686") || triple.starts_with("i586") { "x86" }
    else { "unknown" }
}

fn target_family(triple: &str) -> &'static str {
    if triple.contains("windows") { "windows" } else { "unix" }
}

impl NativeBuilder {
    pub fn new(verbose: bool) -> Result<Self> {
        let rustc = Self::find_in_path("rustc").ok_or_else(|| {
            anyhow::anyhow!("rustc not found in PATH — install Rust from https://rustup.rs")
        })?;
        if verbose {
            eprintln!("compiler: using rustc at {}", rustc.display());
        }
        Ok(Self { rustc_path: rustc, verbose })
    }

    pub fn rustc_version() -> Result<String> {
        let out = std::process::Command::new("rustc")
            .arg("--version")
            .output()
            .context("failed to run rustc --version")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("rustc --version failed: {stderr}");
        }
        let s = String::from_utf8(out.stdout).context("rustc output not utf8")?;
        s.split_whitespace()
            .nth(1)
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("unexpected rustc --version output: {s}"))
    }

    pub fn target_triple() -> Result<String> {
        let out = std::process::Command::new("rustc")
            .arg("-vV")
            .output()
            .context("failed to run rustc -vV")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("rustc -vV failed: {stderr}");
        }
        let s = String::from_utf8(out.stdout).context("rustc output not utf8")?;
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("host: ") {
                return Ok(rest.trim().to_string());
            }
        }
        anyhow::bail!("could not detect host triple from `rustc -vV`")
    }

    pub fn normalize_crate_name(name: &str) -> String {
        name.replace('-', "_")
    }

    fn find_in_path(name: &str) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
            #[cfg(windows)]
            {
                let exe = dir.join(format!("{name}.exe"));
                if exe.exists() {
                    return Some(exe);
                }
            }
        }
        None
    }

    pub fn download_source(name: &str, version: &str, cksum: Option<&str>) -> Result<PathBuf> {
        let src_dir = Self::ignite_src_dir()?.join(format!("{name}-{version}"));
        if src_dir.exists() {
            return Ok(src_dir);
        }

        let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
        let bytes = ureq::get(&url)
            .header("User-Agent", concat!("cargo-ignite/", env!("CARGO_PKG_VERSION")))
            .call()
            .context("failed to download crate tarball")?
            .into_body()
            .read_to_vec()
            .context("failed to read tarball body")?;

        if let Some(expected) = cksum {
            Self::verify_cksum(&bytes, expected)
                .with_context(|| format!("checksum mismatch for {name}-{version}"))?;
        }

        let parent = Self::ignite_src_dir()?;
        std::fs::create_dir_all(&parent)?;
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        archive.unpack(&parent).context("failed to extract crate tarball")?;

        Self::save_to_cargo_registry(name, version, &bytes);
        Ok(src_dir)
    }

    fn verify_cksum(bytes: &[u8], expected_hex: &str) -> Result<()> {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(bytes);
        let computed: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        if computed != expected_hex {
            anyhow::bail!("expected {expected_hex}, got {computed}");
        }
        Ok(())
    }

    pub fn read_edition(src_dir: &Path) -> String {
        let toml_path = src_dir.join("Cargo.toml");
        let Ok(content) = std::fs::read_to_string(&toml_path) else {
            return "2021".to_string();
        };
        let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
            return "2021".to_string();
        };
        doc.get("package")
            .and_then(|p| p.get("edition"))
            .and_then(|e| e.as_str())
            .unwrap_or("2021")
            .to_string()
    }

    fn ignite_src_dir() -> Result<PathBuf> {
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").context("USERPROFILE not set")?;
        #[cfg(not(windows))]
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".cargo-construct").join("src"))
    }

    fn save_to_cargo_registry(name: &str, version: &str, bytes: &[u8]) {
        let Some(index_cache) = crate::crates::CratesAPI::index_cache_dir() else { return; };
        let Some(index_dir) = index_cache.parent() else { return; };
        let Some(registry_dir) = index_dir.parent().and_then(|p| p.parent()) else { return; };
        let Some(dir_name) = index_dir.file_name() else { return; };
        let crate_cache = registry_dir.join("cache").join(dir_name);
        let _ = std::fs::create_dir_all(&crate_cache);
        let _ = std::fs::write(crate_cache.join(format!("{name}-{version}.crate")), bytes);
    }

    pub fn read_package_meta(src_dir: &Path) -> PackageMeta {
        let toml_path = src_dir.join("Cargo.toml");
        let Ok(content) = std::fs::read_to_string(&toml_path) else {
            return PackageMeta { authors: vec![], description: String::new() };
        };
        let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
            return PackageMeta { authors: vec![], description: String::new() };
        };
        let authors = doc.get("package")
            .and_then(|p| p.get("authors"))
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let description = doc.get("package")
            .and_then(|p| p.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string();
        PackageMeta { authors, description }
    }

    pub fn parse_build_script_output(stdout: &str, out_dir: &Path) -> BuildScriptOutput {
        let mut output = BuildScriptOutput {
            out_dir: out_dir.to_path_buf(),
            link_libs: Vec::new(),
            link_search: Vec::new(),
            cfg_flags: Vec::new(),
            env_vars: Vec::new(),
            extra_flags: Vec::new(),
        };
        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("cargo:rustc-link-lib=") {
                output.link_libs.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("cargo:rustc-link-search=") {
                output.link_search.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("cargo:rustc-cfg=") {
                output.cfg_flags.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("cargo:rustc-env=") {
                if let Some((k, v)) = rest.split_once('=') {
                    output.env_vars.push((k.to_string(), v.to_string()));
                }
            } else if let Some(rest) = line.strip_prefix("cargo:rustc-flags=") {
                output.extra_flags.extend(rest.split_whitespace().map(String::from));
            }
        }
        output
    }

    pub fn compile_build_script(&self, node: &DepNode, tmp_dir: &Path) -> Result<PathBuf> {
        let build_rs = node.src_dir.join("build.rs");
        let normalized = Self::normalize_crate_name(&node.name);

        #[cfg(not(windows))]
        let out_bin = tmp_dir.join(format!("build-{normalized}"));
        #[cfg(windows)]
        let out_bin = tmp_dir.join(format!("build-{normalized}.exe"));

        let output = std::process::Command::new(&self.rustc_path)
            .arg("--edition").arg(&node.edition)
            .arg("--crate-type").arg("bin")
            .arg("--crate-name").arg(format!("build_{normalized}"))
            .arg("-o").arg(&out_bin)
            .arg(&build_rs)
            .output()
            .context("failed to spawn rustc for build script")?;

        if !output.status.success() {
            anyhow::bail!(
                "build script compile failed for {}:\n{}",
                node.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(out_bin)
    }

    pub fn run_build_script(bin_path: &Path, node: &DepNode, out_dir: &Path) -> Result<BuildScriptOutput> {
        std::fs::create_dir_all(out_dir).context("failed to create OUT_DIR")?;

        let host = Self::target_triple().unwrap_or_default();
        let num_jobs = rayon::current_num_threads().to_string();
        let meta = Self::read_package_meta(&node.src_dir);

        let output = std::process::Command::new(bin_path)
            .current_dir(&node.src_dir)
            .env("CARGO_MANIFEST_DIR", &node.src_dir)
            .env("OUT_DIR",            out_dir)
            .env("CARGO_PKG_NAME",     &node.name)
            .env("CARGO_PKG_VERSION",  &node.version)
            .env("CARGO_PKG_AUTHORS",  meta.authors.join(":"))
            .env("CARGO_PKG_DESCRIPTION", meta.description)
            .env("HOST",    &host)
            .env("TARGET",  &host)
            .env("OPT_LEVEL", "3")
            .env("PROFILE",   "release")
            .env("NUM_JOBS",  &num_jobs)
            .env("RUSTC",     "rustc")
            .env("CARGO_CFG_TARGET_OS",     target_os(&host))
            .env("CARGO_CFG_TARGET_ARCH",   target_arch(&host))
            .env("CARGO_CFG_TARGET_FAMILY", target_family(&host))
            .output()
            .context("failed to run build script")?;

        if !output.status.success() {
            anyhow::bail!(
                "build script failed for {}:\n{}",
                node.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_build_script_output(&stdout, out_dir))
    }

    pub fn run_build_scripts(
        &self,
        levels: &[Vec<String>],
        nodes: &std::collections::HashMap<String, DepNode>,
        tmp_dir: &Path,
        diag: &crate::progress::DiagnosticCollector,
    ) -> Result<std::collections::HashMap<String, BuildScriptOutput>> {
        use rayon::prelude::*;
        use crate::progress::{Diagnostic, Severity};

        let rustc_path = self.rustc_path.clone();
        let verbose = self.verbose;
        let mut outputs: std::collections::HashMap<String, BuildScriptOutput> = std::collections::HashMap::new();

        for level in levels {
            let bs_names: Vec<&str> = level.iter()
                .filter_map(|n| {
                    let node = nodes.get(n.as_str())?;
                    if node.has_build_script { Some(n.as_str()) } else { None }
                })
                .collect();

            let results: Vec<(String, Result<BuildScriptOutput>)> = bs_names
                .into_par_iter()
                .map(|name| {
                    let result = (|| -> Result<BuildScriptOutput> {
                        let node = nodes.get(name)
                            .ok_or_else(|| anyhow::anyhow!("node '{}' missing", name))?;
                        let out_dir = tmp_dir.join(format!("out-{}", NativeBuilder::normalize_crate_name(name)));
                        let temp = NativeBuilder { rustc_path: rustc_path.clone(), verbose };
                        let bin = temp.compile_build_script(node, tmp_dir)?;
                        NativeBuilder::run_build_script(&bin, node, &out_dir)
                    })();
                    (name.to_string(), result)
                })
                .collect();

            for (name, result) in results {
                match result {
                    Ok(bs_out) => { outputs.insert(name, bs_out); }
                    Err(e) => {
                        diag.push(Diagnostic {
                            severity: Severity::Error,
                            crate_name: name,
                            message: e.to_string(),
                        });
                    }
                }
            }
        }

        Ok(outputs)
    }

    pub fn build_dep_tree(
        api: &crate::crates::CratesAPI,
        name: &str,
        version: &str,
        _features: &[String],
        diag: &crate::progress::DiagnosticCollector,
    ) -> Result<std::collections::HashMap<String, DepNode>> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let mut nodes: HashMap<String, DepNode> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Option<String>)> = VecDeque::new();
        queue.push_back((name.to_string(), Some(version.to_string())));

        while let Some((dep_name, dep_version)) = queue.pop_front() {
            if visited.contains(&dep_name) { continue; }
            visited.insert(dep_name.clone());

            let entry = match api.get(&dep_name, dep_version.as_deref()) {
                Some(entry) => entry,
                None => {
                    diag.push(crate::progress::Diagnostic {
                        severity: crate::progress::Severity::Warning,
                        crate_name: dep_name.clone(),
                        message: "could not resolve dependency (skipping)".into(),
                    });
                    continue;
                }
            };

            let src_dir = Self::download_source(&dep_name, &entry.vers, Some(&entry.cksum))?;
            let edition = Self::read_edition(&src_dir);
            let has_build_script = src_dir.join("build.rs").exists();

            let direct_deps: Vec<String> = entry.deps.iter()
                .filter(|d| d.kind == "normal" && !d.optional)
                .map(|d| d.name.clone())
                .collect();

            for d in &direct_deps {
                if !visited.contains(d) {
                    queue.push_back((d.clone(), None));
                }
            }

            nodes.insert(dep_name.clone(), DepNode {
                name: dep_name,
                version: entry.vers,
                edition,
                direct_deps,
                has_build_script,
                src_dir,
            });
        }

        Ok(nodes)
    }

    pub fn topo_sort(nodes: &std::collections::HashMap<String, DepNode>) -> Vec<Vec<String>> {
        use std::collections::HashMap;

        let mut in_degree: HashMap<String, usize> = nodes.keys().map(|k| (k.clone(), 0)).collect();
        let mut dependents: HashMap<String, Vec<String>> = nodes.keys().map(|k| (k.clone(), vec![])).collect();

        for (name, node) in nodes {
            for dep in &node.direct_deps {
                if nodes.contains_key(dep) {
                    *in_degree.get_mut(name).unwrap() += 1;
                    dependents.entry(dep.clone()).or_default().push(name.clone());
                }
            }
        }

        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut ready: Vec<String> = in_degree.iter()
            .filter(|&(_, v)| *v == 0)
            .map(|(k, _)| k.clone())
            .collect();
        ready.sort();

        while !ready.is_empty() {
            levels.push(ready.clone());
            let mut next: Vec<String> = Vec::new();
            for name in &ready {
                for dependent in dependents.get(name).unwrap_or(&vec![]) {
                    let d = in_degree.get_mut(dependent).unwrap();
                    *d -= 1;
                    if *d == 0 { next.push(dependent.clone()); }
                }
            }
            next.sort();
            ready = next;
        }

        levels
    }

    pub fn compile_lib(
        &self,
        node: &DepNode,
        externs: &std::collections::HashMap<String, PathBuf>,
        out_dir: &Path,
        opt_level: u32,
        bs_out: Option<&BuildScriptOutput>,
    ) -> Result<PathBuf> {
        let src_lib = node.src_dir.join("src").join("lib.rs");

        let mut cmd = std::process::Command::new(&self.rustc_path);
        cmd.arg("--edition").arg(&node.edition)
           .arg("--crate-type").arg("rlib")
           .arg("--crate-name").arg(Self::normalize_crate_name(&node.name))
           .arg(format!("-Copt-level={opt_level}"))
           .arg("--out-dir").arg(out_dir);

        for (dep_name, rlib_path) in externs {
            cmd.arg("--extern").arg(format!(
                "{}={}",
                Self::normalize_crate_name(dep_name),
                rlib_path.display()
            ));
        }

        if let Some(bs) = bs_out {
            for cfg in &bs.cfg_flags {
                cmd.arg("--cfg").arg(cfg);
            }
            for flag in &bs.extra_flags {
                cmd.arg(flag);
            }
            cmd.env("OUT_DIR", &bs.out_dir);
            for (k, v) in &bs.env_vars {
                cmd.env(k, v);
            }
        }

        cmd.arg(&src_lib);

        let output = cmd.output().context("failed to spawn rustc")?;
        if !output.status.success() {
            anyhow::bail!(
                "rustc failed for {}:\n{}",
                node.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let lib_prefix = format!("lib{}", Self::normalize_crate_name(&node.name));
        std::fs::read_dir(out_dir)?
            .filter_map(|e| e.ok())
            .find(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with(&lib_prefix) && n.ends_with(".rlib")
            })
            .map(|e| e.path())
            .ok_or_else(|| anyhow::anyhow!("rlib not found for {} in {}", node.name, out_dir.display()))
    }

    pub fn compile_bin(
        &self,
        node: &DepNode,
        externs: &std::collections::HashMap<String, PathBuf>,
        out_dir: &Path,
        all_build_outputs: &std::collections::HashMap<String, BuildScriptOutput>,
    ) -> Result<PathBuf> {
        let src_main = node.src_dir.join("src").join("main.rs");

        let mut cmd = std::process::Command::new(&self.rustc_path);
        cmd.arg("--edition").arg(&node.edition)
           .arg("--crate-type").arg("bin")
           .arg("--crate-name").arg(Self::normalize_crate_name(&node.name))
           .arg("-Copt-level=3")
           .arg("--out-dir").arg(out_dir);

        for (dep_name, rlib_path) in externs {
            cmd.arg("--extern").arg(format!(
                "{}={}",
                Self::normalize_crate_name(dep_name),
                rlib_path.display()
            ));
        }

        if let Some(bs) = all_build_outputs.get(&node.name) {
            for cfg in &bs.cfg_flags { cmd.arg("--cfg").arg(cfg); }
            cmd.env("OUT_DIR", &bs.out_dir);
            for (k, v) in &bs.env_vars { cmd.env(k, v); }
        }

        for bs in all_build_outputs.values() {
            for lib  in &bs.link_libs   { cmd.arg("-l").arg(lib);  }
            for path in &bs.link_search { cmd.arg("-L").arg(path); }
        }

        cmd.arg(&src_main);

        let output = cmd.output().context("failed to spawn rustc")?;
        if !output.status.success() {
            anyhow::bail!(
                "rustc failed for {}:\n{}",
                node.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        #[cfg(windows)]
        let bin = out_dir.join(format!("{}.exe", node.name));
        #[cfg(not(windows))]
        let bin = out_dir.join(&node.name);

        Ok(bin)
    }

    pub fn compile_all(
        &self,
        levels: &[Vec<String>],
        nodes: &std::collections::HashMap<String, DepNode>,
        out_dir: &Path,
        opt_level: u32,
        build_outputs: &std::collections::HashMap<String, BuildScriptOutput>,
        progress: &std::sync::Arc<crate::progress::CompileProgress>,
        diag: &crate::progress::DiagnosticCollector,
    ) -> Result<std::collections::HashMap<String, PathBuf>> {
        use rayon::prelude::*;
        use crate::progress::{Diagnostic, Severity};

        let rustc_path = self.rustc_path.clone();
        let verbose = self.verbose;
        let mut compiled: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();

        for level in levels {
            let level_work: Vec<(&str, std::collections::HashMap<String, PathBuf>)> = level
                .iter()
                .filter_map(|name| {
                    let node = nodes.get(name.as_str())?;
                    let externs = node.direct_deps.iter()
                        .filter_map(|d| compiled.get(d).map(|p| (d.clone(), p.clone())))
                        .collect();
                    Some((name.as_str(), externs))
                })
                .collect();

            let results: Vec<(String, Result<PathBuf>)> = level_work
                .into_par_iter()
                .map(|(name, externs)| {
                    let result = (|| -> Result<PathBuf> {
                        let node = nodes.get(name)
                            .ok_or_else(|| anyhow::anyhow!("node '{}' missing from dependency tree", name))?;
                        let bs_out = build_outputs.get(name);
                        let temp = NativeBuilder { rustc_path: rustc_path.clone(), verbose };
                        temp.compile_lib(node, &externs, out_dir, opt_level, bs_out)
                    })();
                    (name.to_string(), result)
                })
                .collect();

            for (name, result) in results {
                match result {
                    Ok(path) => {
                        progress.increment();
                        compiled.insert(name, path);
                    }
                    Err(e) => {
                        diag.push(Diagnostic {
                            severity: Severity::Error,
                            crate_name: name,
                            message: e.to_string(),
                        });
                    }
                }
            }
        }

        Ok(compiled)
    }

    pub fn precompile_lib(
        &self,
        name: &str,
        version: &str,
        features: &[String],
        fp: &str,
        cache: &crate::cache::Cache,
        progress: &std::sync::Arc<crate::progress::CompileProgress>,
        diag: &crate::progress::DiagnosticCollector,
    ) -> Result<()> {
        let api = crate::crates::CratesAPI::new();
        let nodes = Self::build_dep_tree(&api, name, version, features, diag)?;
        let levels = Self::topo_sort(&nodes);

        progress.set_total(nodes.len());

        let tmp_out = std::env::temp_dir().join(format!("ignite-lib-{fp}"));
        std::fs::create_dir_all(&tmp_out)?;

        let build_outputs = self.run_build_scripts(&levels, &nodes, &tmp_out, diag)?;
        let compiled = self.compile_all(&levels, &nodes, &tmp_out, 0, &build_outputs, progress, diag)?;

        if compiled.is_empty() {
            anyhow::bail!("no crates compiled — all may have failed or have unresolvable build scripts");
        }

        let meta = crate::cache::CacheMeta {
            crate_name: name.to_string(),
            version: version.to_string(),
            features: features.join(","),
            rustc: Self::rustc_version().unwrap_or_default(),
            target: Self::target_triple().unwrap_or_default(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        cache.store(fp, &tmp_out, &meta)?;
        let _ = std::fs::remove_dir_all(&tmp_out);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn install_bin(
        &self,
        name: &str,
        version: &str,
        _cksum: Option<&str>,
        features: &[String],
        fp: &str,
        cache: &crate::cache::Cache,
        progress: &std::sync::Arc<crate::progress::CompileProgress>,
        diag: &crate::progress::DiagnosticCollector,
    ) -> Result<PathBuf> {
        let api = crate::crates::CratesAPI::new();
        let nodes = Self::build_dep_tree(&api, name, version, features, diag)?;
        let levels = Self::topo_sort(&nodes);

        progress.set_total(nodes.len());

        let tmp_out = std::env::temp_dir().join(format!("ignite-bin-{fp}"));
        std::fs::create_dir_all(&tmp_out)?;

        let build_outputs = self.run_build_scripts(&levels, &nodes, &tmp_out, diag)?;

        let dep_levels: Vec<Vec<String>> = levels.iter()
            .filter(|l| !l.contains(&name.to_string()))
            .cloned()
            .collect();
        let compiled = self.compile_all(&dep_levels, &nodes, &tmp_out, 3, &build_outputs, progress, diag)?;

        let root_node = nodes.get(name)
            .ok_or_else(|| anyhow::anyhow!("root node {} not found in dep tree", name))?;
        let _bin_path = self.compile_bin(root_node, &compiled, &tmp_out, &build_outputs)?;
        progress.increment();

        let meta = crate::cache::CacheMeta {
            crate_name: name.to_string(),
            version: version.to_string(),
            features: features.join(","),
            rustc: Self::rustc_version().unwrap_or_default(),
            target: Self::target_triple().unwrap_or_default(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        cache.store(fp, &tmp_out, &meta)?;
        let _ = std::fs::remove_dir_all(&tmp_out);

        let artifacts_dir = cache.lookup(&fp)
            .ok_or_else(|| anyhow::anyhow!("cache lookup failed immediately after store for {}", fp))?;
        #[cfg(windows)]
        let cached_bin = artifacts_dir.join(format!("{name}.exe"));
        #[cfg(not(windows))]
        let cached_bin = artifacts_dir.join(name);
        Ok(cached_bin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_builder_new() {
        let b = NativeBuilder::new(false).expect("rustc must be in PATH for tests");
        assert!(b.rustc_path.exists());
        assert!(!b.verbose);
    }

    #[test]
    fn test_rustc_version_format() {
        let v = NativeBuilder::rustc_version().expect("rustc must be in PATH for tests");
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 3, "version should be major.minor.patch, got: {v}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "each part must be numeric");
    }

    #[test]
    fn test_target_triple_nonempty() {
        let t = NativeBuilder::target_triple().expect("rustc must be in PATH for tests");
        assert!(!t.is_empty());
        assert!(t.contains('-'), "target triple should contain hyphens, got: {t}");
    }

    #[test]
    fn test_crate_name_normalize() {
        assert_eq!(NativeBuilder::normalize_crate_name("serde-derive"), "serde_derive");
        assert_eq!(NativeBuilder::normalize_crate_name("tokio"), "tokio");
    }

    #[test]
    fn test_dep_node_has_build_script_detection() {
        let dir = std::env::temp_dir().join("ignite-test-build-rs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!dir.join("build.rs").exists());
        std::fs::write(dir.join("build.rs"), b"fn main() {}").unwrap();
        assert!(dir.join("build.rs").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_topo_sort_leaves_first() {
        use std::collections::HashMap;
        let nodes: HashMap<String, DepNode> = [
            ("a".to_string(), DepNode { name: "a".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec!["b".to_string(), "c".to_string()], has_build_script: false, src_dir: PathBuf::from("/fake/a") }),
            ("b".to_string(), DepNode { name: "b".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec![], has_build_script: false, src_dir: PathBuf::from("/fake/b") }),
            ("c".to_string(), DepNode { name: "c".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec![], has_build_script: false, src_dir: PathBuf::from("/fake/c") }),
        ].into_iter().collect();

        let levels = NativeBuilder::topo_sort(&nodes);
        assert_eq!(levels.len(), 2);
        assert!(levels[0].contains(&"b".to_string()));
        assert!(levels[0].contains(&"c".to_string()));
        assert_eq!(levels[1], vec!["a".to_string()]);
    }

    #[test]
    fn test_topo_sort_chain() {
        use std::collections::HashMap;
        let nodes: HashMap<String, DepNode> = [
            ("a".to_string(), DepNode { name: "a".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec!["b".to_string()], has_build_script: false, src_dir: PathBuf::from("/fake/a") }),
            ("b".to_string(), DepNode { name: "b".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec!["c".to_string()], has_build_script: false, src_dir: PathBuf::from("/fake/c") }),
            ("c".to_string(), DepNode { name: "c".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec![], has_build_script: false, src_dir: PathBuf::from("/fake/c") }),
        ].into_iter().collect();

        let levels = NativeBuilder::topo_sort(&nodes);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec!["c".to_string()]);
        assert_eq!(levels[1], vec!["b".to_string()]);
        assert_eq!(levels[2], vec!["a".to_string()]);
    }

    #[test]
    fn test_parse_build_script_output_empty() {
        let bs = NativeBuilder::parse_build_script_output("", std::path::Path::new("/tmp/out"));
        assert!(bs.link_libs.is_empty());
        assert!(bs.cfg_flags.is_empty());
        assert!(bs.env_vars.is_empty());
    }

    #[test]
    fn test_parse_build_script_output_directives() {
        let stdout = "cargo:rustc-link-lib=static=foo\ncargo:rustc-cfg=feature=\"bar\"\ncargo:rustc-env=MY_VAR=hello\ncargo:rustc-link-search=native=/usr/lib\ncargo:rustc-flags=-L extra\n";
        let bs = NativeBuilder::parse_build_script_output(stdout, std::path::Path::new("/tmp/out"));
        assert_eq!(bs.link_libs,   vec!["static=foo"]);
        assert_eq!(bs.cfg_flags,   vec!["feature=\"bar\""]);
        assert_eq!(bs.env_vars,    vec![("MY_VAR".to_string(), "hello".to_string())]);
        assert_eq!(bs.link_search, vec!["native=/usr/lib"]);
        assert_eq!(bs.extra_flags, vec!["-L", "extra"]);
    }

    #[test]
    fn test_compile_and_run_simple_build_script() {
        if NativeBuilder::rustc_version().is_err() { return; }

        let builder = NativeBuilder::new(false).unwrap();
        let tmp = std::env::temp_dir().join("ignite-test-bs");
        std::fs::create_dir_all(&tmp).unwrap();

        std::fs::write(tmp.join("build.rs"), r#"fn main() { println!("cargo:rustc-cfg=my_test_feature"); }"#).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();

        let node = DepNode {
            name: "test-crate".to_string(),
            version: "0.1.0".to_string(),
            edition: "2021".to_string(),
            direct_deps: vec![],
            has_build_script: true,
            src_dir: tmp.clone(),
        };

        let bin = builder.compile_build_script(&node, &tmp).unwrap();
        assert!(bin.exists(), "compiled build script binary should exist");

        let out_dir = tmp.join("out");
        let bs_out = NativeBuilder::run_build_script(&bin, &node, &out_dir).unwrap();
        assert_eq!(bs_out.cfg_flags, vec!["my_test_feature"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
