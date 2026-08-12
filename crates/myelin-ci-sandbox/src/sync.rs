use std::sync::{Mutex, MutexGuard};

/// Keep operational state reachable after an unrelated holder panics.
///
/// Rust's mutex poisoning is advisory. The sandbox registries protected by this
/// helper contain independently valid entries, so abandoning the whole registry
/// would prevent later teardown without making any individual entry safer.
pub(crate) fn lock_recovering_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn poisoned_operational_state_remains_reachable_for_cleanup() {
        let state = Arc::new(Mutex::new(vec!["guest-1"]));
        let held = Arc::clone(&state);

        let _ = std::thread::spawn(move || {
            let mut entries = held.lock().unwrap();
            entries.push("guest-2");
            panic!("poison the registry after a valid update");
        })
        .join();

        assert_eq!(
            lock_recovering_poison(&state).as_slice(),
            ["guest-1", "guest-2"]
        );
    }
}
