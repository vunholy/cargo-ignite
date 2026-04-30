use std::{process::exit, str::FromStr};

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod ansi_color;
pub mod cache;
mod commands;
pub mod compiler;
pub mod crates;
pub mod manifest;
pub mod progress;
pub mod spinner;

use crate::{ansi_color::AnsiColor, commands::Command as CommandTrait};

#[derive(PartialEq)]
enum Command {
    Fetch,
    Add,
    Install,
    Remove,
    Help,
}

impl FromStr for Command {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fetch" => Ok(Command::Fetch),
            "add" => Ok(Command::Add),
            "install" => Ok(Command::Install),
            "remove" => Ok(Command::Remove),
            "help" => Ok(Command::Help),
            _ => Err(format!(
                "{}{} {}{} {} {}{}{}",
                "<".grey(),
                "Uh".b_red(),
                "Oh".b_yellow(),
                ">".grey(),
                "Please enter a valid argument from the following list:".b_black(),
                "[".grey().bold(),
                "fetch | add | install | remove | help".b_magenta(),
                "]".grey().bold()
            )),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command: Command =
        Command::from_str(args.first().unwrap_or(&String::from("help")).as_str())
            .map_err(|err| {
                println!(
                    "{err}\n{} {}{}{} {}\n",
                    "You wrote".b_red(),
                    '"'.grey().bold(),
                    args.first().unwrap().b_yellow(),
                    '"'.grey().bold(),
                    "as a first argument, which is not a valid command!".b_yellow(),
                );
                // Code 100 is for Invalid Command Argument
                exit(100);
            })
            .unwrap();

    let feedable_args = args.iter().skip(1).cloned().collect();

    match command {
        Command::Fetch => {
            let mut cmd = commands::Fetch::default();
            cmd.feed(feedable_args);
            let _res = cmd.run();
        }
        Command::Add => {
            let mut cmd = commands::Add::default();
            cmd.feed(feedable_args);
            if let Err(e) = cmd.run() {
                eprintln!("{}", e);
                exit(1);
            }
        }
        Command::Install => {
            let mut cmd = commands::Install::default();
            cmd.feed(feedable_args);
            if let Err(e) = cmd.run() {
                eprintln!("{}", e);
                exit(1);
            }
        }
        Command::Remove => {
            let mut cmd = commands::Remove::default();
            cmd.feed(feedable_args);
            if let Err(e) = cmd.run() {
                eprintln!("{}", e);
                exit(1);
            }
        }
        _ => {
            commands::Help.run().ok();
        }
    }

    anyhow::Ok(())
}
