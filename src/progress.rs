// TODO: Implement this into the compiling
// Progress tracking for compilation
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

    pub fn increment(&self) -> usize {
        let prev = self.completed.fetch_add(1, Ordering::SeqCst);
        prev + 1
    }

    pub fn percent(&self) -> usize {
        let completed = self.completed.load(Ordering::SeqCst);
        let total = self.total.load(Ordering::SeqCst);
        if total == 0 {
            return 100;
        }
        (completed * 100) / total
    }

    pub fn status(&self) -> String {
        format!("{}%", self.percent())
    }
}
