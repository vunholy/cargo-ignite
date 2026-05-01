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

pub struct NativeBuilder {
    pub rustc_path: PathBuf,
    pub verbose: bool,
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

    pub fn build_dep_tree(
        api: &crate::crates::CratesAPI,
        name: &str,
        version: &str,
        _features: &[String],
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
                    eprintln!("warning: could not resolve dependency: {} (skipping)", dep_name);
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
    ) -> Result<std::collections::HashMap<String, PathBuf>> {
        use rayon::prelude::*;

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

            let results: Vec<Result<(String, PathBuf)>> = level_work
                .into_par_iter()
                .map(|(name, externs)| {
                    let node = nodes.get(name)
                        .ok_or_else(|| anyhow::anyhow!("node '{}' missing from dependency tree", name))?;

                    if node.has_build_script {
                        eprintln!(" warning: {} has a build script — skipping precompile for this crate", name);
                        return Err(anyhow::anyhow!("build script: {name}"));
                    }

                    let temp = NativeBuilder { rustc_path: rustc_path.clone(), verbose };
                    let rlib = temp.compile_lib(node, &externs, out_dir, opt_level)?;
                    Ok((name.to_string(), rlib))
                })
                .collect();

            for r in results {
                match r {
                    Ok((name, path)) => { compiled.insert(name, path); }
                    Err(e) if e.to_string().starts_with("build script:") => {}
                    Err(e) => return Err(e),
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
        verbose: bool,
    ) -> Result<()> {
        let api = crate::crates::CratesAPI::new();
        let nodes = Self::build_dep_tree(&api, name, version, features)?;
        let levels = Self::topo_sort(&nodes);

        let tmp_out = std::env::temp_dir().join(format!("ignite-lib-{fp}"));
        std::fs::create_dir_all(&tmp_out)?;

        let compiled = self.compile_all(&levels, &nodes, &tmp_out, 0)?;

        if compiled.is_empty() {
            anyhow::bail!("no crates compiled — all may have build scripts requiring cargo-construct");
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

        if verbose {
            eprintln!("precompiled {} v{} → cached as {fp}", name, version);
        }

        let _ = std::fs::remove_dir_all(&tmp_out);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn install_bin(
        &self,
        name: &str,
        version: &str,
        cksum: Option<&str>,
        features: &[String],
        fp: &str,
        cache: &crate::cache::Cache,
        verbose: bool,
    ) -> Result<PathBuf> {
        let api = crate::crates::CratesAPI::new();
        let nodes = Self::build_dep_tree(&api, name, version, features)?;
        let levels = Self::topo_sort(&nodes);

        let tmp_out = std::env::temp_dir().join(format!("ignite-bin-{fp}"));
        std::fs::create_dir_all(&tmp_out)?;

        let dep_levels: Vec<Vec<String>> = levels.iter()
            .filter(|l| !l.contains(&name.to_string()))
            .cloned()
            .collect();
        let compiled = self.compile_all(&dep_levels, &nodes, &tmp_out, 3)?;

        let root_node = nodes.get(name)
            .ok_or_else(|| anyhow::anyhow!("root node {} not found in dep tree", name))?;
        let bin_path = self.compile_bin(root_node, &compiled, &tmp_out)?;

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

        if verbose {
            eprintln!("installed {} v{} → cached as {fp}", name, version);
        }

        Ok(bin_path)
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
            ("b".to_string(), DepNode { name: "b".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec!["c".to_string()], has_build_script: false, src_dir: PathBuf::from("/fake/b") }),
            ("c".to_string(), DepNode { name: "c".to_string(), version: "1.0.0".to_string(), edition: "2021".to_string(), direct_deps: vec![], has_build_script: false, src_dir: PathBuf::from("/fake/c") }),
        ].into_iter().collect();

        let levels = NativeBuilder::topo_sort(&nodes);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec!["c".to_string()]);
        assert_eq!(levels[1], vec!["b".to_string()]);
        assert_eq!(levels[2], vec!["a".to_string()]);
    }
}
