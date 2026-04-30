pub trait Command {
    fn feed(&mut self, arguments: Vec<String>);
    fn run(&self) -> Result<(), String>;
}

mod add;
mod fetch;
mod help;
mod install;
mod remove;

pub use add::Add;
pub use fetch::Fetch;
pub use help::Help;
pub use install::Install;
pub use remove::Remove;
