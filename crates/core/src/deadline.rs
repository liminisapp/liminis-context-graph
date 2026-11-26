//! Per-request deadline for `knowledge_process_chunk` / `knowledge_process_document` (issue #914,
//! ADR-0914).
//!
//! A [`RequestDeadline`] is built at handler entry from the request's cancellation token and the
//! optional `deadline_ms`. It hands the handler a *child* token to use wherever it used the parent
//! one, so every existing pre-commit cancellation point (ADR-0861) also enforces the deadline with
//! no new points. When the timer fires it records that it did, then cancels the child.
//!
//! Nothing here touches the commit: a token that fires after the write lock is held is simply
//! never looked at. Whether a *successful* request overran its deadline is answered by
//! [`RequestDeadline::exceeded`], which compares the clock after the commit returned.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::error::Error;

/// The last phase a request reached, shared between the code doing the work and the handler that
/// has to name it in a `deadline_exceeded` error.
#[derive(Debug, Clone)]
pub struct PhaseCell(Arc<Mutex<&'static str>>);

impl Default for PhaseCell {
    fn default() -> Self {
        Self::new("embed")
    }
}

impl PhaseCell {
    pub fn new(initial: &'static str) -> Self {
        Self(Arc::new(Mutex::new(initial)))
    }

    pub fn set(&self, phase: &'static str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = phase;
    }

    pub fn get(&self) -> &'static str {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Parses the optional `deadline_ms` param: absent/null is `None`; anything but a positive
/// integer is a validation error (FR-001), raised before any work or registration.
pub fn parse_deadline_ms(params: &serde_json::Value) -> Result<Option<u64>, Error> {
    match &params["deadline_ms"] {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => match n.as_u64() {
            Some(ms) if ms > 0 => Ok(Some(ms)),
            _ => Err(Error::Ipc(
                "deadline_ms must be a positive integer number of milliseconds".to_string(),
            )),
        },
        _ => Err(Error::Ipc(
            "deadline_ms must be a positive integer number of milliseconds".to_string(),
        )),
    }
}

/// A started deadline. Dropping it aborts the timer, so it never fires after the request ends.
pub struct RequestDeadline {
    at: Option<Instant>,
    token: CancellationToken,
    parent: CancellationToken,
    fired: Arc<AtomicBool>,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl RequestDeadline {
    /// Starts the clock now. `None` (or a value too large to represent as an `Instant`) means no
    /// deadline: the token is then just the parent.
    pub fn start(parent: CancellationToken, deadline_ms: Option<u64>) -> Self {
        let at = deadline_ms.and_then(|ms| Instant::now().checked_add(Duration::from_millis(ms)));
        let Some(at) = at else {
            return Self {
                at: None,
                token: parent.clone(),
                parent,
                fired: Arc::new(AtomicBool::new(false)),
                timer: None,
            };
        };
        let token = parent.child_token();
        let fired = Arc::new(AtomicBool::new(false));
        let timer = {
            let (token, fired) = (token.clone(), Arc::clone(&fired));
            tokio::spawn(async move {
                tokio::time::sleep_until(at).await;
                fired.store(true, Ordering::SeqCst);
                token.cancel();
            })
        };
        Self {
            at: Some(at),
            token,
            parent,
            fired,
            timer: Some(timer),
        }
    }

    /// The token to hand to the cancellation points in place of the request's own.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// True when a deadline was set and the clock has reached it.
    pub fn exceeded(&self) -> bool {
        self.at.is_some_and(|at| Instant::now() >= at)
    }

    /// Maps an error returned by the work. A `Cancelled` caused by the deadline timer — and not
    /// by the client or shutdown cancelling the parent — becomes `DeadlineExceeded` naming
    /// `phase`; every other error is returned unchanged. The parent wins ties: a client that has
    /// gone away, or a shutdown, is reported as the cancellation it is.
    pub fn classify(&self, err: Error, phase: &'static str, document: bool) -> Error {
        match err {
            Error::Cancelled
                if self.fired.load(Ordering::SeqCst) && !self.parent.is_cancelled() =>
            {
                Error::DeadlineExceeded { phase, document }
            }
            other => other,
        }
    }
}

impl Drop for RequestDeadline {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_only_positive_integers() {
        assert_eq!(parse_deadline_ms(&json!({})).unwrap(), None);
        assert_eq!(
            parse_deadline_ms(&json!({"deadline_ms": null})).unwrap(),
            None
        );
        assert_eq!(
            parse_deadline_ms(&json!({"deadline_ms": 250})).unwrap(),
            Some(250)
        );
        for bad in [
            json!(0),
            json!(-5),
            json!(1.5),
            json!("100"),
            json!(true),
            json!([1]),
        ] {
            assert!(
                parse_deadline_ms(&json!({"deadline_ms": bad})).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn no_deadline_uses_parent_token() {
        let parent = CancellationToken::new();
        let d = RequestDeadline::start(parent.clone(), None);
        assert!(!d.exceeded());
        parent.cancel();
        assert!(d.token().is_cancelled());
        assert!(matches!(
            d.classify(Error::Cancelled, "extract", false),
            Error::Cancelled
        ));
    }

    #[tokio::test]
    async fn overflowing_deadline_is_no_deadline() {
        let d = RequestDeadline::start(CancellationToken::new(), Some(u64::MAX));
        assert!(!d.exceeded());
        assert!(!d.token().is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn timer_cancels_child_and_classifies_as_deadline() {
        let parent = CancellationToken::new();
        let d = RequestDeadline::start(parent.clone(), Some(100));
        assert!(!d.token().is_cancelled());
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(d.token().is_cancelled());
        assert!(
            !parent.is_cancelled(),
            "the deadline must not cancel the parent"
        );
        assert!(d.exceeded());
        match d.classify(Error::Cancelled, "awaiting_write_lock", false) {
            Error::DeadlineExceeded { phase, document } => {
                assert_eq!(phase, "awaiting_write_lock");
                assert!(!document);
            }
            other => panic!("expected DeadlineExceeded, got {other:?}"),
        }
        // Other errors pass through untouched.
        assert!(matches!(
            d.classify(Error::Ipc("x".into()), "extract", false),
            Error::Ipc(_)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn parent_cancel_wins_over_deadline() {
        let parent = CancellationToken::new();
        let d = RequestDeadline::start(parent.clone(), Some(100));
        tokio::time::sleep(Duration::from_millis(150)).await;
        parent.cancel();
        assert!(matches!(
            d.classify(Error::Cancelled, "extract", false),
            Error::Cancelled
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn parent_cancel_before_deadline_is_not_a_deadline() {
        let parent = CancellationToken::new();
        let d = RequestDeadline::start(parent.clone(), Some(10_000));
        parent.cancel();
        assert!(d.token().is_cancelled());
        assert!(matches!(
            d.classify(Error::Cancelled, "extract", false),
            Error::Cancelled
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_aborts_the_timer() {
        let parent = CancellationToken::new();
        let d = RequestDeadline::start(parent, Some(100));
        let token = d.token();
        drop(d);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!token.is_cancelled(), "a dropped deadline must not fire");
    }

    #[test]
    fn phase_cell_tracks_last_set() {
        let c = PhaseCell::new("embed");
        let c2 = c.clone();
        c2.set("extract");
        assert_eq!(c.get(), "extract");
    }
}
