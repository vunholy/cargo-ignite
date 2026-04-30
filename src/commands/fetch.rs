use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::crates::CrateMetadata;
use crate::{
    commands::Command,
    crates::{CratesAPI, IndexEntry},
};

mod a {
    pub const R: &str = "\x1b[0m";
    pub const G: &str = "\x1b[37m";
    pub const Y: &str = "\x1b[33m";
    pub const C: &str = "\x1b[36m";
    pub const M: &str = "\x1b[35m";
    pub const BR: &str = "\x1b[1;91m";
    pub const BY: &str = "\x1b[1;33m";
    pub const BC: &str = "\x1b[1;36m";
    pub const BM: &str = "\x1b[1;35m";
    pub const BG: &str = "\x1b[1;37m";
}

/// Thread-safe cache for crate metadata using Arc<Mutex<>> for zero-copy sharing
#[derive(Default)]
pub struct MetadataCache {
    cache: Arc<Mutex<HashMap<String, CrateMetadata>>>,
}

impl MetadataCache {
    /// Build cache key from crate name + version
    #[inline]
    fn make_key(name: &str, version: Option<&str>) -> String {
        match version {
            Some(v) => format!("{}@{}", name, v),
            None => format!("{}@latest", name),
        }
    }

    /// Get metadata from cache
    pub fn get(&self, name: &str, version: Option<&str>) -> Option<CrateMetadata> {
        let key = Self::make_key(name, version);
        self.cache.lock().unwrap().get(&key).cloned()
    }

    /// Insert metadata into cache
    pub fn insert(&self, name: String, version: Option<String>, metadata: CrateMetadata) {
        let key = Self::make_key(&name, version.as_deref());
        self.cache.lock().unwrap().insert(key, metadata);
    }
}

#[derive(Default)]
pub struct Fetch {
    name: String,
    version: Option<String>,
    entry: Option<IndexEntry>,
    api: CratesAPI,
    show_metadata: bool,
    metadata_cache: MetadataCache,
}

impl std::fmt::Debug for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetch")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("show_metadata", &self.show_metadata)
            .finish()
    }
}

#[inline(always)]
fn write_feats<W: Write>(buf: &mut W, feats: &[(String, Vec<String>)], kc: &str) -> io::Result<()> {
    use a::*;
    for (k, v) in feats {
        write!(buf, "\t    {G}-{R} {kc}{k}{R} {G}=>{R} ")?;
        if v.is_empty() {
            write!(buf, "{BM}none{R}")?;
        } else {
            buf.write_all(b"[")?;
            let mut it = v.iter();
            if let Some(f) = it.next() {
                write!(buf, "{BY}{f}{R}")?;
            }
            for i in it {
                write!(buf, ", {BY}{i}{R}")?;
            }
            buf.write_all(b"]")?;
        }
        buf.write_all(b"\n")?;
    }
    Ok(())
}

#[inline(always)]
fn write_metadata<W: Write>(buf: &mut W, metadata: &CrateMetadata) -> io::Result<()> {
    use a::*;

    writeln!(buf, "\t{BC}crate metadata{R}")?;

    // Description
    match &metadata.description {
        Some(desc) => writeln!(buf, "\t  {G}description  :{R} {Y}{desc}{R}")?,
        None => writeln!(buf, "\t  {G}description  :{R} {BR}N/A{R}")?,
    }

    // Tags
    write!(buf, "\t  {G}tags         :{R} ")?;
    if metadata.tags.is_empty() {
        writeln!(buf, "{BR}none{R}")?;
    } else {
        buf.write_all(b"[")?;
        let mut it = metadata.tags.iter();
        if let Some(tag) = it.next() {
            write!(buf, "{BY}{tag}{R}")?;
        }
        for tag in it {
            write!(buf, ", {BY}{tag}{R}")?;
        }
        buf.write_all(b"]\n")?;
    }

    Ok(())
}

impl Command for Fetch {
    fn feed(&mut self, arguments: Vec<String>) {
        let args = arguments.into_iter();

        // Parse arguments looking for --metadata/-m flag
        let mut name_arg = None;
        let mut version_arg = None;

        for arg in args {
            match arg.as_str() {
                "--metadata" | "-m" => {
                    self.show_metadata = true;
                }
                _ => {
                    if name_arg.is_none() {
                        name_arg = Some(arg);
                    } else if version_arg.is_none() {
                        version_arg = Some(arg);
                    }
                }
            }
        }

        self.name = name_arg.expect("Crate name is required");
        self.version = version_arg;
        self.entry = self.api.get(&self.name, self.version.as_deref());
    }

    fn run(&self) -> Result<(), String> {
        use a::*;

        match &self.entry {
            Some(e) => {
                let mut buf = Vec::with_capacity(8192);

                // Writing to Vec<u8> is infallible. .unwrap() is zero-cost here
                // because the compiler optimizes away the panic branch entirely.
                writeln!(buf, "\t{BC}crate info{R}").unwrap();
                writeln!(buf, "\t  {G}version      :{R} {Y}{}{R}", e.vers).unwrap();
                writeln!(buf, "\t  {G}rust version :{R} {BY}{}{R}", e.rust_version).unwrap();
                writeln!(buf, "\t  {G}checksum     :{R} {BY}{}{R}", e.cksum).unwrap();
                writeln!(buf, "\t  {G}yanked       :{R} {BR}{}{R}", e.yanked).unwrap();
                writeln!(buf, "\t{BG}  features     :{R}").unwrap();
                write_feats(&mut buf, &e.features, M).unwrap();
                writeln!(buf, "\t{BG}  features2    :{R}").unwrap();
                write_feats(&mut buf, &e.features2, C).unwrap();

                // Handle metadata if requested
                if self.show_metadata {
                    let metadata = self.get_or_fetch_metadata()?;
                    write_metadata(&mut buf, &metadata)
                        .map_err(|e| format!("Failed to write metadata: {}", e))?;
                }

                io::stdout()
                    .lock()
                    .write_all(&buf)
                    .map_err(|e| e.to_string())
            }
            None => Err(String::from("There's no valid entry. :(")),
        }
    }
}

impl Fetch {
    /// Get or fetch metadata with PER-VERSION caching
    #[inline]
    fn get_or_fetch_metadata(&self) -> Result<CrateMetadata, String> {
        // FAST PATH: Check instance-level cache with VERSION-AWARE key
        // Key format: "serde@1.0.152" or "serde@latest" if no version specified
        if let Some(cached) = self.metadata_cache.get(&self.name, self.version.as_deref()) {
            return Ok(cached);
        }

        // FALLBACK: Fetch from API (handles L1/L2/L3 caching internally)
        let metadata = self
            .api
            .get_metadata(&self.name)
            .ok_or("Failed to fetch crate metadata")?;

        // Store with VERSION-SPECIFIC key
        self.metadata_cache.insert(
            self.name.clone(),
            self.version.clone(), // Some("1.0.152") or None
            metadata.clone(),
        );

        Ok(metadata)
    }
}
