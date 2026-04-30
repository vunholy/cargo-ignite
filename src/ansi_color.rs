use std::fmt::Display;

#[allow(unused)]
pub trait AnsiColor: Display + Sized {
    #[inline]
    fn wrap(self, prefix: &str) -> String {
        format!("{prefix}{self}\x1b[0m")
    }

    fn magenta(self) -> String {
        self.wrap("\x1b[35m")
    }
    fn yellow(self) -> String {
        self.wrap("\x1b[33m")
    }
    fn grey(self) -> String {
        self.wrap("\x1b[37m")
    }
    fn blue(self) -> String {
        self.wrap("\x1b[34m")
    }
    fn cyan(self) -> String {
        self.wrap("\x1b[36m")
    }
    fn black(self) -> String {
        self.wrap("\x1b[30m")
    }

    fn b_black(self) -> String {
        self.wrap("\x1b[90m")
    }
    fn b_red(self) -> String {
        self.wrap("\x1b[91m")
    }
    fn b_yellow(self) -> String {
        self.wrap("\x1b[93m")
    }
    fn b_magenta(self) -> String {
        self.wrap("\x1b[95m")
    }
    fn b_cyan(self) -> String {
        self.wrap("\x1b[96m")
    }

    fn bold(self) -> String {
        self.wrap("\x1b[1m")
    }
    fn underlined(self) -> String {
        self.wrap("\x1b[4m")
    }
}

impl<T: Display> AnsiColor for T {}
