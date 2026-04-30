use anyhow::{Context, Result};
use blake3::Hasher;
use simd_json::base::{ValueAsObject, ValueAsScalar};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct CacheKey<'a> {
    pub rustc_version: &'a str,
    pub crate_name: &'a str,
    pub crate_version: &'a str,
    pub target_triple: &'a str,
    pub features: &'a FeatureSet,
}

pub enum FeatureSet {
    All,
    Explicit(Vec<String>),
}

pub struct CacheMeta {
    pub crate_name: String,
    pub version: String,
    pub features: String,
    pub rustc: String,
    pub target: String,
    pub cached_at: u64,
}

pub struct Cache {
    pub(crate) root: PathBuf,
    pub(crate) verbose: bool,
}

impl Cache {
    pub fn new(verbose: bool) -> Result<Self> {
        let root = Self::cache_root()?;
        std::fs::create_dir_all(root.join("cache"))
            .context("failed to create ~/.cargo-construct/cache")?;
        Ok(Self { root, verbose })
    }

    pub fn fingerprint(key: &CacheKey<'_>) -> String {
        // "ALL" is distinct from "" (no features); both would hash identically
        // with an empty string, producing cache collisions across feature sets.
        let features_str: String = match key.features {
            FeatureSet::All => "ALL".to_string(),
            FeatureSet::Explicit(v) => {
                let mut sorted = v.clone();
                sorted.sort();
                sorted.join(",")
            }
        };

        // Null bytes as field separators prevent length-extension collisions:
        // "foo"+"bar" and "fo"+"obar" must not produce the same hash.
        let mut hasher = Hasher::new();
        hasher.update(key.rustc_version.as_bytes());
        hasher.update(b"\x00");
        hasher.update(key.crate_name.as_bytes());
        hasher.update(b"\x00");
        hasher.update(key.crate_version.as_bytes());
        hasher.update(b"\x00");
        hasher.update(key.target_triple.as_bytes());
        hasher.update(b"\x00");
        hasher.update(features_str.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    pub(crate) fn cache_root() -> Result<PathBuf> {
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").context("USERPROFILE not set")?;
        #[cfg(not(windows))]
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".cargo-construct"))
    }

    /// Returns the artifacts directory path if there is a valid cache entry.
    pub fn lookup(&self, fp: &str) -> Option<PathBuf> {
        let index = self.read_index().ok()?;
        let entry_path_str = index.get(fp)?;
        let entry_path = PathBuf::from(entry_path_str);
        let artifacts = entry_path.join("artifacts");
        if artifacts.exists() {
            if self.verbose {
                eprintln!("cache hit: {fp}");
            }
            let _ = self.touch_lru(fp);
            Some(artifacts)
        } else {
            None
        }
    }

    /// Copy all files from `artifacts_src` into the cache and record in ignite-index.json.
    pub fn store(&self, fp: &str, artifacts_src: &std::path::Path, meta: &CacheMeta) -> Result<()> {
        let entry_dir = self.root.join("cache").join(fp);
        let artifacts_dst = entry_dir.join("artifacts");
        std::fs::create_dir_all(&artifacts_dst)?;

        for entry in std::fs::read_dir(artifacts_src)? {
            let entry = entry?;
            if entry.metadata()?.is_file() {
                std::fs::copy(entry.path(), artifacts_dst.join(entry.file_name()))?;
            }
        }

        let meta_json = format!(
            r#"{{"crate":"{}","version":"{}","features":"{}","rustc":"{}","target":"{}","cached_at":{}}}"#,
            meta.crate_name, meta.version, meta.features, meta.rustc, meta.target, meta.cached_at
        );
        std::fs::write(entry_dir.join("meta.json"), meta_json)?;

        let mut index = self.read_index().unwrap_or_default();
        index.insert(fp.to_string(), entry_dir.to_string_lossy().to_string());
        self.write_index(&index)?;

        let _ = self.touch_lru(fp);
        Ok(())
    }

    pub fn evict_if_needed(&self) -> Result<()> {
        self.evict_if_needed_with_limit(10 * 1024 * 1024 * 1024)
    }

    pub fn evict_if_needed_with_limit(&self, max_bytes: u64) -> Result<()> {
        let cache_dir = self.root.join("cache");
        if !cache_dir.exists() {
            return Ok(());
        }
        // Measure only artifact sizes (not metadata) against the limit
        let artifact_size = |fp: &str| -> u64 {
            dir_size(&cache_dir.join(fp).join("artifacts")).unwrap_or(0)
        };
        let lru = self.read_lru().unwrap_or_default();
        let mut entries: Vec<(String, u64)> = lru.into_iter().collect();
        entries.sort_by_key(|(_, ts)| *ts); // oldest first

        // Compute total artifact size across all cached entries
        let total: u64 = entries.iter().map(|(fp, _)| artifact_size(fp)).sum();
        if total <= max_bytes {
            return Ok(());
        }

        let mut freed = 0u64;
        let need = total.saturating_sub(max_bytes);
        for (fp, _) in &entries {
            if freed >= need {
                break;
            }
            let entry_artifact_size = artifact_size(fp);
            let entry_dir = cache_dir.join(fp);
            if std::fs::remove_dir_all(&entry_dir).is_ok() {
                freed += entry_artifact_size;
                if let Ok(mut idx) = self.read_index() {
                    idx.remove(fp);
                    let _ = self.write_index(&idx);
                }
            }
        }
        Ok(())
    }

    fn read_index(&self) -> Result<HashMap<String, String>> {
        let path = self.root.join("ignite-index.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let mut bytes = std::fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(HashMap::new());
        }
        let value = simd_json::to_borrowed_value(&mut bytes)
            .context("failed to parse ignite-index.json")?;
        let obj = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("ignite-index.json is not a JSON object"))?;
        Ok(obj
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
            .collect())
    }

    fn write_index(&self, index: &HashMap<String, String>) -> Result<()> {
        let mut buf = String::from("{");
        let mut first = true;
        for (k, v) in index {
            if !first {
                buf.push(',');
            }
            buf.push_str(&format!(r#""{}":"{}""#, k, v.replace('\\', "\\\\")));
            first = false;
        }
        buf.push('}');
        let path = self.root.join("ignite-index.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn touch_lru(&self, fp: &str) -> Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.set_lru_timestamp(fp, ts)
    }

    // pub(crate) so tests can inject deterministic timestamps without real syscalls.
    pub(crate) fn set_lru_timestamp(&self, fp: &str, ts: u64) -> Result<()> {
        let mut lru = self.read_lru().unwrap_or_default();
        lru.insert(fp.to_string(), ts);
        self.write_lru(&lru)
    }

    fn read_lru(&self) -> Result<HashMap<String, u64>> {
        let path = self.root.join("lru.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let mut bytes = std::fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(HashMap::new());
        }
        let value = simd_json::to_borrowed_value(&mut bytes)
            .context("failed to parse lru.json")?;
        let obj = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("lru.json is not a JSON object"))?;
        Ok(obj
            .iter()
            .filter_map(|(k, v)| v.as_u64().map(|n| (k.to_string(), n)))
            .collect())
    }

    fn write_lru(&self, lru: &HashMap<String, u64>) -> Result<()> {
        let mut buf = String::from("{");
        let mut first = true;
        for (k, v) in lru {
            if !first {
                buf.push(',');
            }
            buf.push_str(&format!(r#""{}":{}"#, k, v));
            first = false;
        }
        buf.push('}');
        let path = self.root.join("lru.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

fn dir_size(path: &std::path::Path) -> Result<u64> {
    let mut size = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            size += meta.len();
        } else if meta.is_dir() {
            size += dir_size(&entry.path()).unwrap_or(0);
        }
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_stability() {
        let fs = FeatureSet::Explicit(vec!["derive".to_string(), "std".to_string()]);
        let key = CacheKey {
            rustc_version: "1.87.0",
            crate_name: "serde",
            crate_version: "1.0.219",
            target_triple: "x86_64-pc-windows-msvc",
            features: &fs,
        };
        let fp1 = Cache::fingerprint(&key);
        let fp2 = Cache::fingerprint(&key);
        assert_eq!(fp1, fp2, "same inputs must produce same fingerprint");
        assert_eq!(fp1.len(), 64, "blake3 hex is 64 chars");
    }

    #[test]
    fn test_fingerprint_features_differ() {
        let fs_all = FeatureSet::All;
        let fs_none = FeatureSet::Explicit(vec![]);
        let fs_derive = FeatureSet::Explicit(vec!["derive".to_string()]);

        let make = |fs: &FeatureSet| Cache::fingerprint(&CacheKey {
            rustc_version: "1.87.0",
            crate_name: "serde",
            crate_version: "1.0.219",
            target_triple: "x86_64-unknown-linux-gnu",
            features: fs,
        });

        let fp_all = make(&fs_all);
        let fp_none = make(&fs_none);
        let fp_derive = make(&fs_derive);

        assert_ne!(fp_all, fp_none);
        assert_ne!(fp_all, fp_derive);
        assert_ne!(fp_none, fp_derive);
    }

    #[test]
    fn test_fingerprint_features_sorted() {
        let fs_ab = FeatureSet::Explicit(vec!["a".to_string(), "b".to_string()]);
        let fs_ba = FeatureSet::Explicit(vec!["b".to_string(), "a".to_string()]);
        assert_eq!(
            Cache::fingerprint(&CacheKey {
                rustc_version: "1.87.0", crate_name: "x", crate_version: "1.0.0",
                target_triple: "x86_64-unknown-linux-gnu", features: &fs_ab,
            }),
            Cache::fingerprint(&CacheKey {
                rustc_version: "1.87.0", crate_name: "x", crate_version: "1.0.0",
                target_triple: "x86_64-unknown-linux-gnu", features: &fs_ba,
            }),
            "feature order must not affect fingerprint"
        );
    }

    #[test]
    fn test_cache_store_and_lookup() {
        use std::fs;
        let tmp_root = std::env::temp_dir().join("ignite-test-cache-store");
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).unwrap();

        let cache = Cache { root: tmp_root.clone(), verbose: false };

        let fp = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";

        // Lookup on empty cache returns None
        assert!(cache.lookup(fp).is_none());

        // Create a fake artifacts directory
        let artifacts_src = tmp_root.join("fake-artifacts");
        fs::create_dir_all(&artifacts_src).unwrap();
        fs::write(artifacts_src.join("libfoo.rlib"), b"fake rlib").unwrap();

        let meta = CacheMeta {
            crate_name: "foo".to_string(),
            version: "1.0.0".to_string(),
            features: "ALL".to_string(),
            rustc: "1.87.0".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            cached_at: 1000,
        };

        cache.store(fp, &artifacts_src, &meta).unwrap();

        // Now lookup should return the artifacts dir
        let hit = cache.lookup(fp);
        assert!(hit.is_some(), "should be a cache hit after store");
        let artifacts_dir = hit.unwrap();
        assert!(artifacts_dir.join("libfoo.rlib").exists(), "rlib should be in cache");

        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_lru_eviction() {
        use std::fs;
        let tmp_root = std::env::temp_dir().join("ignite-test-cache-lru");
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).unwrap();

        let cache = Cache { root: tmp_root.clone(), verbose: false };

        // Store two entries with different timestamps in lru.json
        for (fp, ts) in &[("aaa", 100u64), ("bbb", 200u64)] {
            let artifacts_src = tmp_root.join(format!("fake-{fp}"));
            fs::create_dir_all(&artifacts_src).unwrap();
            // Write a 1-byte file to each entry to simulate small size
            fs::write(artifacts_src.join("lib.rlib"), b"x").unwrap();
            let meta = CacheMeta {
                crate_name: fp.to_string(), version: "1.0.0".to_string(),
                features: "ALL".to_string(), rustc: "1.87.0".to_string(),
                target: "x86_64-unknown-linux-gnu".to_string(), cached_at: *ts,
            };
            cache.store(fp, &artifacts_src, &meta).unwrap();
            // Override cached_at in lru.json to test eviction order
            cache.set_lru_timestamp(fp, *ts).unwrap();
        }

        // Force eviction with a very small limit (1 byte)
        cache.evict_if_needed_with_limit(1).unwrap();

        // "aaa" (ts=100, older) should be evicted; "bbb" (ts=200) should remain
        assert!(cache.lookup("bbb").is_some(), "newer entry should survive");
        assert!(cache.lookup("aaa").is_none(), "older entry should be evicted");

        let _ = fs::remove_dir_all(&tmp_root);
    }
}
