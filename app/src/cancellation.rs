//! Cooperative cancellation for synchronous work executed off the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct State {
    cancelled: AtomicBool,
    parent: Option<Cancellation>,
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<State>);

impl Cancellation {
    pub fn child(&self) -> Self {
        Self(Arc::new(State {
            cancelled: AtomicBool::new(false),
            parent: Some(self.clone()),
        }))
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
            || self.0.parent.as_ref().is_some_and(Self::is_cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_cancellation_is_local_but_session_cancellation_reaches_all_children() {
        let session = Cancellation::default();
        let first = session.child();
        let second = session.child();
        first.cancel();
        assert!(!session.is_cancelled());
        assert!(!second.is_cancelled());
        session.cancel();
        assert!(second.is_cancelled());
    }

    #[test]
    fn old_operations_stay_cancelled_when_returning_to_the_same_folder() {
        let first_a = Cancellation::default();
        let delayed_a = first_a.clone();
        first_a.cancel();
        let b = Cancellation::default();
        b.cancel();
        let second_a = Cancellation::default();
        assert!(delayed_a.is_cancelled());
        assert!(!second_a.is_cancelled());
    }

    #[test]
    fn cancelling_a_transition_does_not_cancel_the_active_session() {
        let session = Cancellation::default();
        let transition = Cancellation::default();
        transition.cancel();
        assert!(!session.is_cancelled());
        assert!(transition.is_cancelled());
    }
}
