use memchr::memchr;
use memchr::memmem::Finder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use simd_json::{
    BorrowedValue,
    base::{ValueAsArray, ValueAsObject, ValueAsScalar},
    to_borrowed_value,
};

#[derive(Default, Debug, Clone)]
pub struct CrateMetadata {
    pub description: Option<String>,
    pub tags: Vec<String>,
}

static METADATA_CACHE: OnceLock<Arc<Mutex<HashMap<String, CrateMetadata>>>> = OnceLock::new();

fn get_metadata_cache() -> &'static Arc<Mutex<HashMap<String, CrateMetadata>>> {
    METADATA_CACHE.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

#[derive(Debug, Clone)]
pub struct IndexDep {
    pub name: String,
    pub req: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub default_features: bool,
    pub kind: String,
}

#[derive(Default, Debug)]
pub struct IndexEntry {
    pub vers: String,
    pub cksum: String,
    pub rust_version: String,
    pub features: Vec<(String, Vec<String>)>,
    pub features2: Vec<(String, Vec<String>)>,
    pub yanked: bool,
    pub deps: Vec<IndexDep>,
}

#[derive(Default)]
pub struct CratesAPI;

// Computed once per process; scanning ~/.cargo/registry/index/ on every lookup
// would require a directory walk for each crate name query.
static INDEX_CACHE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

impl CratesAPI {
    #[inline]
    pub fn new() -> Self {
        Self
    }

    #[inline]
    pub fn index_path(name: &str) -> String {
        let b = name.as_bytes();
        match b.len() {
            1 => {
                let mut s = String::with_capacity(3);
                s.push_str("1/");
                s.push(b[0] as char);
                s
            }
            2 => {
                let mut s = String::with_capacity(4);
                s.push_str("2/");
                s.push(b[0] as char);
                s.push(b[1] as char);
                s
            }
            3 => {
                let mut s = String::with_capacity(5);
                s.push_str("3/");
                s.push(b[0] as char);
                s.push('/');
                s.push_str(name);
                s
            }
            _ => {
                let mut s = String::with_capacity(name.len() + 3);
                s.push(b[0] as char);
                s.push(b[1] as char);
                s.push('/');
                s.push(b[2] as char);
                s.push(b[3] as char);
                s.push('/');
                s.push_str(name);
                s
            }
        }
    }

    fn cargo_home() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("CARGO_HOME") {
            return Some(p.into());
        }
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").ok()?;
        #[cfg(not(windows))]
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".cargo"))
    }

    fn compute_cache_dir() -> Option<PathBuf> {
        let base = Self::cargo_home()?.join("registry").join("index");
        let dir = std::fs::read_dir(base)
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .as_encoded_bytes()
                    .starts_with(b"index.crates.io-")
            })?
            .path();
        Some(dir.join("cache"))
    }

    #[inline]
    pub fn index_cache_dir() -> Option<&'static Path> {
        INDEX_CACHE_DIR
            .get_or_init(Self::compute_cache_dir)
            .as_deref()
    }

    #[inline]
    fn read_local(path: &str) -> Option<Vec<u8>> {
        std::fs::read(Self::index_cache_dir()?.join(path)).ok()
    }

    #[inline]
    fn write_local(path: &str, bytes: &[u8]) {
        let Some(cache) = Self::index_cache_dir() else {
            return;
        };
        let file = cache.join(path);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(file, bytes);
    }

    #[inline]
    fn fetch(crate_name: &str) -> Option<Vec<u8>> {
        let name = match crate_name.len() {
            1 => format!("1/{crate_name}"),
            2 => format!("2/{crate_name}"),
            3 => format!("3/{}/{crate_name}", &crate_name[..1]),
            _ => format!("{}/{}/{crate_name}", &crate_name[..2], &crate_name[2..4]),
        };

        ureq::get(&format!("https://index.crates.io/{name}"))
            .header(
                "User-Agent",
                concat!("cargo-ignite/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .ok()?
            .body_mut()
            .read_to_vec()
            .ok()
    }

    #[inline]
    pub fn get_metadata(&self, crate_name: &str) -> Option<CrateMetadata> {
        // Checks memory cache, then disk cache, then network.
        // Lock is dropped before any I/O to avoid holding it during a slow fetch.
        if let Ok(cache) = get_metadata_cache().lock()
            && let Some(meta) = cache.get(crate_name)
        {
            return Some(meta.clone());
        }

        let meta = self
            .read_metadata_from_disk(crate_name)
            .or_else(|| self.fetch_and_parse_metadata(crate_name))?;

        if let Ok(mut cache) = get_metadata_cache().lock() {
            cache.insert(crate_name.to_string(), meta.clone());
        }

        Some(meta)
    }

    #[inline]
    fn read_metadata_from_disk(&self, crate_name: &str) -> Option<CrateMetadata> {
        let path = format!("meta/{}", crate_name);
        let bytes = Self::read_local(&path)?;
        self.parse_metadata_bytes(&bytes)
    }

    #[inline]
    fn fetch_and_parse_metadata(&self, crate_name: &str) -> Option<CrateMetadata> {
        let bytes = ureq::get(&format!("https://crates.io/api/v1/crates/{crate_name}"))
            .header(
                "User-Agent",
                concat!("cargo-ignite/", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "application/json")
            .call()
            .ok()?
            .into_body()
            .read_to_vec()
            .ok()?;

        let meta = self.parse_metadata_bytes(&bytes)?;
        let path = format!("meta/{}", crate_name);
        Self::write_local(&path, &bytes);

        Some(meta)
    }

    #[inline]
    fn parse_metadata_bytes(&self, bytes: &[u8]) -> Option<CrateMetadata> {
        let mut scratch = bytes.to_vec();
        let value = to_borrowed_value(&mut scratch).ok()?;
        let obj = value.as_object()?;

        // crates.io wraps everything in {"crate": {...}}
        let crate_obj = obj.get("crate")?.as_object()?;

        // memmem fast-path: description appears near the top of the response, so
        // scanning for the key string is cheaper than a full SIMD-JSON field lookup.
        let desc_finder = Finder::new(b"\"description\"");
        let description = if let Some(pos) = desc_finder.find(bytes) {
            self.extract_string_field(bytes, pos + "\"description\":".len())
        } else {
            crate_obj
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from)
        };

        let tags = if let Some(tags_val) = crate_obj.get("keywords") {
            if let Some(arr) = tags_val.as_array() {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Some(CrateMetadata { description, tags })
    }

    #[inline(always)]
    fn extract_string_field(&self, bytes: &[u8], start_pos: usize) -> Option<String> {
        let mut pos = start_pos;
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b':') {
            pos += 1;
        }
        if bytes.get(pos)? != &b'"' {
            return None;
        }
        pos += 1;
        let end = memchr(b'"', &bytes[pos..])? + pos;
        let str_slice = std::str::from_utf8(&bytes[pos..end]).ok()?;
        Some(str_slice.to_string())
    }

    #[inline]
    fn parse_features(obj: &BorrowedValue) -> Vec<(String, Vec<String>)> {
        let Some(map) = obj.as_object() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(map.len());
        for (k, v) in map {
            let Some(arr) = v.as_array() else { continue };
            let feats: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(|e| e.to_string()))
                .collect();
            out.push((k.to_string(), feats));
        }
        out
    }

    #[inline]
    fn build_entry(obj: &BorrowedValue, vers: &str) -> Option<IndexEntry> {
        let map = obj.as_object()?;

        let deps = map
            .get("deps")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| {
                        let dm = d.as_object()?;
                        Some(IndexDep {
                            name: dm.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            req: dm.get("req").and_then(|v| v.as_str()).unwrap_or("*").to_string(),
                            features: dm
                                .get("features")
                                .and_then(|v| v.as_array())
                                .map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect())
                                .unwrap_or_default(),
                            optional: dm.get("optional").and_then(|v| v.as_bool()).unwrap_or(false),
                            default_features: dm
                                .get("default_features")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true),
                            kind: dm
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .unwrap_or("normal")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(IndexEntry {
            vers: String::from(vers),
            cksum: String::from(map.get("cksum").and_then(|v| v.as_str()).unwrap_or("")),
            rust_version: String::from(
                map.get("rust_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            ),
            features: map
                .get("features")
                .map(Self::parse_features)
                .unwrap_or_default(),
            features2: map
                .get("features2")
                .map(Self::parse_features)
                .unwrap_or_default(),
            yanked: map.get("yanked").and_then(|v| v.as_bool()).unwrap_or(false),
            deps,
        })
    }

    #[inline]
    fn find(bytes: &[u8], version: Option<&str>) -> Option<IndexEntry> {
        let mut scratch = Vec::new();

        if let Some(ver) = version {
            let mut pos = 0;
            while pos < bytes.len() {
                let line_end = memchr(b'\n', &bytes[pos..])
                    .map(|i| pos + i)
                    .unwrap_or(bytes.len());
                let line = &bytes[pos..line_end];

                if line.first() == Some(&b'{')
                    && let Some(extracted_ver) = Self::extract_version_field(line)
                    && extracted_ver == ver
                {
                    scratch.resize(line.len(), 0);
                    scratch.copy_from_slice(line);
                    if let Ok(v) = to_borrowed_value(&mut scratch) {
                        return Self::build_entry(&v, ver);
                    }
                }
                pos = line_end.saturating_add(1);
            }
        } else {
            let yanked_finder = Finder::new(b"\"yanked\":true");

            // Collect all valid (non-yanked) release versions
            let mut candidates: Vec<(String, usize)> = Vec::new(); // (version_string, byte_offset)
            let mut pos = 0;

            while pos < bytes.len() {
                let line_end = memchr(b'\n', &bytes[pos..])
                    .map(|i| pos + i)
                    .unwrap_or(bytes.len());
                let line = &bytes[pos..line_end];

                if line.first() == Some(&b'{')
                    && yanked_finder.find(line).is_none()  // Not yanked
                    && let Some(vers) = Self::extract_version_field(line)
                    && Self::is_valid_release_version(vers)
                {
                    candidates.push((vers.to_string(), pos));
                }
                pos = line_end.saturating_add(1);
            }

            // Find the SEMANTICALLY latest version (not just last in file!)
            if let Some((latest_ver, offset)) = Self::find_latest_version(&candidates) {
                let line_start = offset;
                let line_end = memchr(b'\n', &bytes[line_start..])
                    .map(|i| line_start + i)
                    .unwrap_or(bytes.len());
                let line = &bytes[line_start..line_end];

                scratch.resize(line.len(), 0);
                scratch.copy_from_slice(line);
                if let Ok(v) = to_borrowed_value(&mut scratch) {
                    return Self::build_entry(&v, &latest_ver);
                }
            }
        }
        None
    }

    /// Find the semantically latest version from candidates
    /// Handles version comparison correctly: 0.13.3 > 0.13.2 > 0.13.1
    #[inline]
    fn find_latest_version(candidates: &[(String, usize)]) -> Option<(String, usize)> {
        candidates
            .iter()
            .max_by(|(a, _), (b, _)| Self::compare_semver(a, b))
            .map(|(v, o)| (v.clone(), *o))
    }

    /// Compare two semantic version strings
    /// Returns Ordering indicating which is greater
    #[inline]
    fn compare_semver(a: &str, b: &str) -> std::cmp::Ordering {
        // Parse version parts (major.minor.patch)
        let parse_parts =
            |v: &str| -> Vec<u64> { v.split('.').map(|s| s.parse().unwrap_or(0)).collect() };

        let parts_a = parse_parts(a);
        let parts_b = parse_parts(b);

        // Compare each part
        for (pa, pb) in parts_a.iter().zip(parts_b.iter()) {
            match pa.cmp(pb) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }

        // If all equal so far, longer version (more parts) wins
        parts_a.len().cmp(&parts_b.len())
    }

    /// Check if version represents a valid stable release
    #[inline(always)]
    fn is_valid_release_version(version: &str) -> bool {
        !version.is_empty()
            && version.contains('.')       // Must have dots (e.g., "1.0.0")
            && !version.contains('-')      // No pre-release markers
            && !version.contains("alpha")  // No alpha/beta keywords
            && !version.contains("beta")
            && !version.contains("rc")
            && version.chars().all(|c| c.is_ascii_digit() || c == '.')
    }

    /// Extract ONLY the "vers" field value from a JSON line
    #[inline(always)]
    fn extract_version_field(line: &[u8]) -> Option<&str> {
        static VERS_FINDER: OnceLock<Finder<'static>> = OnceLock::new();
        let finder = VERS_FINDER.get_or_init(|| Finder::new(b"\"vers\":\""));

        let pos = finder.find(line)?;
        let value_start = pos + 8; // Skip "vers":"
        let value_end = memchr(b'"', &line[value_start..])? + value_start;

        std::str::from_utf8(&line[value_start..value_end]).ok()
    }

    /// Maximum cache age before forcing refresh (24 hours)
    const CACHE_TTL_SECS: u64 = 86400;

    #[inline]
    pub fn get(&self, crate_name: &str, version: Option<&str>) -> Option<IndexEntry> {
        let path = Self::index_path(crate_name);

        // Pinned-version queries never expire: the content of a specific version
        // on crates.io is immutable by registry policy. Only "latest" queries
        // need the 24h TTL check.
        let should_refresh = if version.is_some() {
            false
        } else {
            Self::is_cache_stale(&path).unwrap_or(true)
        };

        let bytes = if should_refresh {
            // Force refresh for latest version queries
            Self::fetch(crate_name)
                .inspect(|b| Self::write_local(&path, b))
                .or_else(|| Self::read_local(&path))
        } else {
            // Use cache (or fetch if missing)
            Self::read_local(&path)
                .or_else(|| Self::fetch(crate_name).inspect(|b| Self::write_local(&path, b)))
        }?;

        Self::find(&bytes, version)
    }

    /// Check if cached file is older than CACHE_TTL_SECS
    #[inline]
    fn is_cache_stale(path: &str) -> Option<bool> {
        let cache_path = match Self::index_cache_dir() {
            Some(dir) => dir.join(path),
            None => return Some(true), // No cache dir = always refresh
        };

        // Get file metadata
        let metadata = match std::fs::metadata(&cache_path) {
            Ok(m) => m,
            Err(_) => return Some(true), // File doesn't exist = needs fetch
        };

        // Check modification time
        let modified = metadata.modified().ok()?;
        let age_secs = SystemTime::now()
            .duration_since(modified)
            .ok()
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        Some(age_secs > Self::CACHE_TTL_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_dep_parsing() {
        let line = br#"{"name":"foo","vers":"1.0.0","deps":[{"name":"serde","req":"^1.0","features":["derive"],"optional":false,"default_features":true,"kind":"normal"},{"name":"tokio","req":"^1","features":[],"optional":true,"default_features":true,"kind":"normal"}],"cksum":"abc","features":{},"yanked":false}"#;
        let mut scratch = line.to_vec();
        let v = simd_json::to_borrowed_value(&mut scratch).unwrap();
        let entry = CratesAPI::build_entry(&v, "1.0.0").unwrap();
        assert_eq!(entry.deps.len(), 2);
        assert_eq!(entry.deps[0].name, "serde");
        assert_eq!(entry.deps[0].kind, "normal");
        assert!(!entry.deps[0].optional);
        assert_eq!(entry.deps[1].name, "tokio");
        assert!(entry.deps[1].optional);
    }
}
