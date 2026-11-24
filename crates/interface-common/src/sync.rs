//! The poison-tolerant mutex guard most crates here take.
//!
//! A panic while a guard is held marks the mutex poisoned. Every caller of
//! [`lock`] holds state that no half-finished update can break (lists, maps and
//! handles that are replaced whole), so it takes the guard anyway rather than
//! turning one panic into a cascade. A caller whose state does have such an
//! invariant keeps its own lock helper that reports poisoning instead.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex`, recovering the guard if an earlier holder panicked.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::lock;

    #[test]
    fn a_poisoned_mutex_still_yields_its_value() {
        let shared = Arc::new(Mutex::new(vec![1]));
        let poisoner = Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            let mut guard = poisoner.lock().unwrap();
            guard.push(2);
            panic!("poison the mutex");
        })
        .join();
        assert!(shared.is_poisoned());
        lock(&shared).push(3);
        assert_eq!(*lock(&shared), vec![1, 2, 3]);
    }
}
