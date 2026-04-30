use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Spinner {
    shared: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn new(msg: impl Into<String> + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(false), Condvar::new()));
        let shared2 = Arc::clone(&shared);
        let msg = msg.into();

        let handle = std::thread::spawn(move || {
            let (lock, cvar) = &*shared2;
            let mut frame = 0usize;
            loop {
                print!(
                    "\r\x1b[2K\t  \x1b[36m{}\x1b[0m \x1b[90m{}\x1b[0m",
                    FRAMES[frame % FRAMES.len()],
                    msg
                );
                std::io::stdout().flush().ok();
                frame += 1;

                // Wait up to 80ms for the next frame, but wake immediately when stopped.
                // Condvar::wait_timeout_while holds the lock and re-checks the predicate
                // after each wakeup, so a notify_one() during the wait is never lost.
                let guard = lock.lock().unwrap();
                let (guard, _) = cvar
                    .wait_timeout_while(guard, Duration::from_millis(80), |stopped| !*stopped)
                    .unwrap();
                if *guard {
                    break;
                }
            }
        });

        Self { shared, handle: Some(handle) }
    }

    pub fn finish_with(mut self, line: impl Into<String>) {
        self.stop_inner();
        println!("\r\x1b[2K{}", line.into());
    }

    pub fn finish_lines(mut self, lines: impl IntoIterator<Item = impl Into<String>>) {
        self.stop_inner();
        print!("\r\x1b[2K");
        for line in lines {
            println!("{}", line.into());
        }
    }

    fn stop_inner(&mut self) {
        let (lock, cvar) = &*self.shared;
        *lock.lock().unwrap() = true;
        cvar.notify_one(); // wakes the thread immediately instead of waiting out the sleep
        if let Some(h) = self.handle.take() {
            h.join().ok();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_inner();
        print!("\r\x1b[2K");
        std::io::stdout().flush().ok();
    }
}
