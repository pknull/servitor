//! Clock and identifier source for telemetry published to the feed.
//!
//! Every timestamp and generated identifier in exported telemetry comes from
//! here, so tests can freeze both and compare published bytes exactly.

use chrono::{DateTime, Utc};

/// Current time for an exported telemetry record.
pub fn now() -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(at) = frozen::tick() {
        return at;
    }
    Utc::now()
}

/// A fresh identifier for a trace, span or execution: 32 lowercase hex
/// characters, independent of any task content.
pub fn generate_id() -> String {
    #[cfg(test)]
    if let Some(id) = frozen::next_id() {
        return id;
    }
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
pub(crate) use frozen::freeze;

#[cfg(test)]
mod frozen {
    use std::cell::RefCell;

    use chrono::{DateTime, Duration, Utc};

    struct State {
        at: DateTime<Utc>,
        next_id: u64,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    /// Freeze telemetry on this thread: [`super::now`] starts at `at` and
    /// advances 1ms per call, and [`super::generate_id`] counts up from 1.
    /// The clock thaws when the guard drops.
    pub(crate) fn freeze(at: DateTime<Utc>) -> Frozen {
        STATE.with(|state| *state.borrow_mut() = Some(State { at, next_id: 1 }));
        Frozen(())
    }

    pub(crate) struct Frozen(());

    impl Drop for Frozen {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = None);
        }
    }

    pub(super) fn tick() -> Option<DateTime<Utc>> {
        STATE.with(|state| {
            state.borrow_mut().as_mut().map(|state| {
                let at = state.at;
                state.at += Duration::milliseconds(1);
                at
            })
        })
    }

    pub(super) fn next_id() -> Option<String> {
        STATE.with(|state| {
            state.borrow_mut().as_mut().map(|state| {
                let id = format!("{:032x}", state.next_id);
                state.next_id += 1;
                id
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_clock_repeats_exactly_and_thaws() {
        let at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let run = || {
            let _frozen = freeze(at);
            (now(), now(), generate_id(), generate_id())
        };

        let first = run();
        assert_eq!(first, run());
        assert_eq!(first.1 - first.0, chrono::Duration::milliseconds(1));
        assert_eq!(first.2, format!("{:032x}", 1));

        assert_ne!(generate_id(), generate_id());
    }
}
