use std::io::{self, Write};

use crate::commands::Command;

mod a {
    pub const R: &str = "\x1b[0m";
    pub const G: &str = "\x1b[37m";
    pub const Y: &str = "\x1b[33m";
    pub const BC: &str = "\x1b[1;36m";
    pub const BY: &str = "\x1b[1;33m";
    pub const BG: &str = "\x1b[1;37m";
    pub const BM: &str = "\x1b[1;35m";
}

#[derive(Default)]
pub struct Help;

impl Command for Help {
    fn feed(&mut self, _arguments: Vec<String>) {}

    fn run(&self) -> Result<(), String> {
        use a::*;

        let mut buf = Vec::with_capacity(2048);

        writeln!(buf, "\t{BC}hatch{R}  {G}— fast dependency management for Rust{R}").unwrap();
        writeln!(buf).unwrap();

        writeln!(buf, "\t{BG}usage{R}").unwrap();
        writeln!(buf, "\t  {Y}hatch{R} {G}<command>{R} {G}[args]{R}").unwrap();
        writeln!(buf).unwrap();

        writeln!(buf, "\t{BG}commands{R}").unwrap();

        // fetch
        writeln!(buf, "\t  {BC}fetch{R} {BY}<crate>{R} {G}[version] [--metadata]{R}").unwrap();
        writeln!(buf, "\t    {G}look up crate info from the local index{R}").unwrap();
        writeln!(buf).unwrap();

        // add
        writeln!(buf, "\t  {BC}add{R} {BY}<crate[@version]>{R} {G}[crate2 ...] [--features f1,f2] [--precompile]{R}").unwrap();
        writeln!(buf, "\t    {G}add crates to Cargo.toml and prefetch source tarballs{R}").unwrap();
        writeln!(buf, "\t    {G}use {R}{BM}--precompile{G} to build and cache artifacts for faster downstream builds{R}").unwrap();
        writeln!(buf).unwrap();

        // remove
        writeln!(buf, "\t  {BC}remove{R} {BY}<crate>{R} {G}[crate2 ...]{R}").unwrap();
        writeln!(buf, "\t    {G}remove crates from Cargo.toml{R}").unwrap();
        writeln!(buf).unwrap();

        // install
        writeln!(buf, "\t  {BC}install{R} {BY}<crate>{R} {G}[version] [--features f1,f2]{R}").unwrap();
        writeln!(buf, "\t    {G}compile and install a binary crate to {R}{Y}~/.cargo/bin/{R}").unwrap();
        writeln!(buf, "\t    {G}uses a content-addressed cache — repeated installs are instant{R}").unwrap();
        writeln!(buf).unwrap();

        // help
        writeln!(buf, "\t  {BC}help{R}").unwrap();
        writeln!(buf, "\t    {G}show this message{R}").unwrap();
        writeln!(buf).unwrap();

        writeln!(buf, "\t{BG}flags{R}").unwrap();
        writeln!(buf, "\t  {BM}--verbose{R} {G}/ {R}{BM}-v{R}   {G}print cache decisions, detected tools, compiler flags{R}").unwrap();

        io::stdout().lock().write_all(&buf).map_err(|e| e.to_string())
    }
}
