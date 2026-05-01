use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct CompileProgress {
    pub total: AtomicUsize,
    pub completed: AtomicUsize,
}

impl CompileProgress {
    pub fn new(total: usize) -> Self {
        Self {
            total: AtomicUsize::new(total),
            completed: AtomicUsize::new(0),
        }
    }

    pub fn set_total(&self, n: usize) {
        self.total.store(n, Ordering::SeqCst);
    }

    pub fn increment(&self) -> usize {
        let prev = self.completed.fetch_add(1, Ordering::SeqCst);
        prev + 1
    }

    pub fn fraction(&self) -> (usize, usize) {
        let completed = self.completed.load(Ordering::SeqCst);
        let total = self.total.load(Ordering::SeqCst);
        (completed, total)
    }

    pub fn percent(&self) -> usize {
        let (completed, total) = self.fraction();
        if total == 0 { return 100; }
        (completed * 100) / total
    }

    pub fn status(&self) -> String {
        format!("{}%", self.percent())
    }
}

pub enum Severity {
    Warning,
    Error,
}

pub struct Diagnostic {
    pub severity: Severity,
    pub crate_name: String,
    pub message: String,
}

#[derive(Clone)]
pub struct DiagnosticCollector {
    pub inner: Arc<Mutex<Vec<Diagnostic>>>,
    pub verbose: bool,
}

impl DiagnosticCollector {
    pub fn new(verbose: bool) -> Self {
        Self { inner: Arc::new(Mutex::new(Vec::new())), verbose }
    }

    pub fn push(&self, d: Diagnostic) {
        if self.verbose {
            let prefix = match d.severity {
                Severity::Warning => "\x1b[33mwarning\x1b[0m",
                Severity::Error   => "\x1b[31merror\x1b[0m",
            };
            eprintln!("\r\x1b[2K      {}: \x1b[36m{}\x1b[0m — {}", prefix, d.crate_name, d.message);
        } else {
            self.inner.lock().unwrap().push(d);
        }
    }

    pub fn drain_pretty(&self) {
        let diags = self.inner.lock().unwrap();
        if diags.is_empty() { return; }

        let warnings: Vec<&Diagnostic> = diags.iter().filter(|d| matches!(d.severity, Severity::Warning)).collect();
        let errors:   Vec<&Diagnostic> = diags.iter().filter(|d| matches!(d.severity, Severity::Error)).collect();

        if !warnings.is_empty() {
            println!("\t  \x1b[1;37mwarnings\x1b[0m");
            let last = warnings.len() - 1;
            for (i, d) in warnings.iter().enumerate() {
                let branch = if i == last { "└─" } else { "├─" };
                println!("\t    \x1b[2m{}\x1b[0m \x1b[36m{:<24}\x1b[0m \x1b[33m{}\x1b[0m", branch, d.crate_name, d.message);
            }
        }

        if !errors.is_empty() {
            println!("\t  \x1b[1;37merrors\x1b[0m");
            let last = errors.len() - 1;
            for (i, d) in errors.iter().enumerate() {
                let branch = if i == last { "└─" } else { "├─" };
                println!("\t    \x1b[2m{}\x1b[0m \x1b[36m{:<24}\x1b[0m \x1b[31m{}\x1b[0m", branch, d.crate_name, d.message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_progress_set_total() {
        let p = CompileProgress::new(0);
        assert_eq!(p.fraction(), (0, 0));
        p.set_total(10);
        assert_eq!(p.fraction(), (0, 10));
        p.increment();
        assert_eq!(p.fraction(), (1, 10));
    }

    #[test]
    fn test_diagnostic_collector_buffers_in_default_mode() {
        let d = DiagnosticCollector::new(false);
        d.push(Diagnostic { severity: Severity::Warning, crate_name: "foo".into(), message: "bar".into() });
        let diags = d.inner.lock().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].crate_name, "foo");
    }

    #[test]
    fn test_diagnostic_collector_verbose_does_not_buffer() {
        let d = DiagnosticCollector::new(true);
        d.push(Diagnostic { severity: Severity::Warning, crate_name: "foo".into(), message: "bar".into() });
        let diags = d.inner.lock().unwrap();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn test_diagnostic_collector_drain_pretty_empty_is_noop() {
        let d = DiagnosticCollector::new(false);
        d.drain_pretty();
    }
}
