use anyhow::Result;

use crate::{commands::Command, manifest::Manifest, spinner::Spinner};

mod a {
    pub const R: &str = "\x1b[0m";
    pub const G: &str = "\x1b[37m";
    pub const Y: &str = "\x1b[33m";
    pub const BC: &str = "\x1b[1;36m";
    pub const BY: &str = "\x1b[1;33m";
    pub const BR: &str = "\x1b[1;91m";
}

#[derive(Default)]
pub struct Remove {
    crates: Vec<String>,
}

impl Command for Remove {
    fn feed(&mut self, arguments: Vec<String>) {
        for arg in arguments {
            if !arg.starts_with('-') {
                self.crates.push(arg);
            }
        }
    }

    fn run(&self) -> Result<(), String> {
        self.run_inner().map_err(|e| e.to_string())
    }
}

impl Remove {
    fn run_inner(&self) -> Result<()> {
        use a::*;

        if self.crates.is_empty() {
            anyhow::bail!("no crates specified — usage: hatch remove <crate> [crate2 ...]");
        }

        let names_display = self.crates.iter()
            .map(|n| format!("{BY}{n}{R}"))
            .collect::<Vec<_>>()
            .join("  ");
        println!("\t{BC}remove{R}  {names_display}");

        let sp = Spinner::new("updating manifest...");
        let cwd = std::env::current_dir()?;
        let mut manifest = Manifest::load(&cwd)?;

        let mut removed = Vec::new();
        let mut not_found = Vec::new();
        for name in &self.crates {
            if manifest.remove_dependency(name) {
                removed.push(name.as_str());
            } else {
                not_found.push(name.as_str());
            }
        }

        if removed.is_empty() {
            drop(sp);
            anyhow::bail!(
                "none of the specified crates were found in [dependencies]: {}",
                not_found.join(", ")
            );
        }

        manifest.save()?;

        let mut lines: Vec<String> = removed.iter()
            .map(|name| format!("\t  {G}{:<13}:{R} {Y}removed{R}", name))
            .collect();

        for name in &not_found {
            lines.push(format!("\t  {G}{:<13}:{R} {BR}not found{R}", name));
        }

        let n = removed.len();
        lines.push(format!(
            "\t  {G}{:<13}:{R} {Y}{} {} removed{R}",
            "manifest", n, if n == 1 { "crate" } else { "crates" }
        ));

        sp.finish_lines(lines);
        Ok(())
    }
}
