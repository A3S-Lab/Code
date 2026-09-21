//! Hermetic mutex-poison helper for coverage tests.
//!
//! Kept outside F-table kernels so `panic` / `resume_unwind` expansions do not
//! dilute `--no-cfg-coverage` JSON line scoring for production modules.

use std::sync::Mutex;

/// Poison `mutex` by panicking while a guard is held, catching the unwind.
pub(crate) fn poison_mutex<T>(mutex: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().expect("mutex lock");
        std::panic::resume_unwind(Box::new("intentional mutex poison for test"));
    }));
}
