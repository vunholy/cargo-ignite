use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Value, value};

#[derive(Debug)]
pub struct Manifest {
    path: PathBuf,
    doc: DocumentMut,
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("Cargo.toml");
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("not in a Rust project (no Cargo.toml found at {})", path.display()))?;
        let doc = content
            .parse::<DocumentMut>()
            .context("failed to parse Cargo.toml")?;
        Ok(Self { path, doc })
    }

    pub fn upsert_dependency(&mut self, name: &str, version: &str, features: &[String]) {
        if self.doc.get("dependencies").is_none() {
            self.doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        if features.is_empty() {
            self.doc["dependencies"][name] = value(version);
        } else {
            let mut inline = InlineTable::new();
            inline.insert("version", version.into());
            let mut arr = Array::new();
            for f in features {
                arr.push(f.as_str());
            }
            inline.insert("features", Value::Array(arr));
            self.doc["dependencies"][name] = Item::Value(Value::InlineTable(inline));
        }
    }

    /// Remove a dependency. Returns `true` if it was present, `false` if not found.
    pub fn remove_dependency(&mut self, name: &str) -> bool {
        self.doc
            .get_mut("dependencies")
            .and_then(|d| d.as_table_mut())
            .map(|t| t.remove(name).is_some())
            .unwrap_or(false)
    }

    pub fn save(&self) -> Result<()> {
        let content = self.doc.to_string();
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, &content)
            .context("failed to write temp Cargo.toml")?;
        std::fs::rename(&tmp, &self.path)
            .context("failed to rename Cargo.toml into place")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_insert_simple_dep() {
        let dir = std::env::temp_dir().join("ignite-test-manifest-insert");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();

        let mut manifest = Manifest::load(&dir).unwrap();
        manifest.upsert_dependency("serde", "1.0.219", &[]);
        manifest.save().unwrap();

        let content = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(content.contains("serde"), "dep should appear in Cargo.toml");
        assert!(content.contains("1.0.219"), "version should appear");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_insert_dep_with_features() {
        let dir = std::env::temp_dir().join("ignite-test-manifest-features");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();

        let mut manifest = Manifest::load(&dir).unwrap();
        manifest.upsert_dependency("serde", "1.0.219", &["derive".to_string()]);
        manifest.save().unwrap();

        let content = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(content.contains("derive"), "features should appear");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_updates_version() {
        let dir = std::env::temp_dir().join("ignite-test-manifest-upsert");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1.0.100\"\n").unwrap();

        let mut manifest = Manifest::load(&dir).unwrap();
        manifest.upsert_dependency("serde", "1.0.219", &[]);
        manifest.save().unwrap();

        let content = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(content.contains("1.0.219"), "version should be updated");
        assert!(!content.contains("1.0.100"), "old version should be gone");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_missing_cargo_toml() {
        let dir = std::env::temp_dir().join("ignite-test-manifest-missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let result = Manifest::load(&dir);
        assert!(result.is_err(), "should error when no Cargo.toml");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no Cargo.toml"), "error message should mention Cargo.toml");
        let _ = fs::remove_dir_all(&dir);
    }
}
