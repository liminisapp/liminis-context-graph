//! Bounded retry with backoff, window splitting and a cross-window circuit breaker for the WAL
//! replay embedding recompute path (issue #808, ADR-0808).
//!
//! `Embedder::embed_batch` is all-or-nothing, so before this module one transient failure
//! (timeout, connection reset, HTTP 5xx/429) zero-filled an entire replay window (up to 64 rows ×
//! 4 vector keys). [`embed_with_retry`] wraps the window's single embed call:
//!
//! - a **transient** error is retried with exponential backoff and jitter, at most
//!   [`EmbedRetryPolicy::retries`] extra times;
//! - when retries are exhausted for a multi-request call, the call is **split** in half and each
//!   half is resolved recursively, down to a single request — one text the embedder can never
//!   process fails only its own request;
//! - a **permanent** error (HTTP 4xx other than 429, shape mismatch, ...) is never retried; on a
//!   multi-request call it splits once straight away (a batch-level 4xx is usually caused by one
//!   text), and a single request that hits one fails after exactly one attempt;
//! - a **per-window budget** on transiently failing calls bounds the slow work across the whole
//!   split tree, and a **cross-window breaker** drops the next window to one attempt with no retry or
//!   split when the previous window failed entirely and transiently (a dead embedder), so a dead
//!   embedder costs about one timeout per window — the pre-#808 cost — instead of hours.
//!
//! Replay runs in `spawn_blocking` (or a bare-sync caller), so backoff is a plain sleep, taken in
//! short slices so a cancellation or shutdown is noticed promptly.
//!
//! Live ingest (`add_episode`) deliberately does not use this module: it has no zero-vector
//! fallback (an embedder failure fails the ingest) and the #510/#541 no-retry-on-timeout decision
//! still stands there.

use std::time::Duration;

use crate::embedder::{classify_embed_error, EmbedErrorClass};
use crate::error::Error;

pub const RETRIES_ENV: &str = "LCG_REPLAY_EMBED_RETRIES";
pub const BACKOFF_MS_ENV: &str = "LCG_REPLAY_EMBED_BACKOFF_MS";
pub const BACKOFF_MAX_MS_ENV: &str = "LCG_REPLAY_EMBED_BACKOFF_MAX_MS";

pub const DEFAULT_RETRIES: u32 = 4;
pub const DEFAULT_BACKOFF_MS: u64 = 500;
pub const DEFAULT_BACKOFF_MAX_MS: u64 = 10_000;

pub const MAX_RETRIES: u32 = 10;
pub const MAX_BACKOFF_MS: u64 = 60_000;
pub const MAX_BACKOFF_MAX_MS: u64 = 300_000;

/// Longest single uninterrupted sleep, so a cancel is noticed within this long.
const SLEEP_SLICE: Duration = Duration::from_millis(100);

/// Retry/backoff settings for replay embedding recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedRetryPolicy {
    /// Extra attempts after the first (0 disables retry *and* splitting).
    pub retries: u32,
    /// Base backoff; attempt `n` waits `min(cap, base * 2^n)` with jitter.
    pub base_ms: u64,
    /// Upper bound on one backoff wait.
    pub cap_ms: u64,
}

impl Default for EmbedRetryPolicy {
    fn default() -> Self {
        Self {
            retries: DEFAULT_RETRIES,
            base_ms: DEFAULT_BACKOFF_MS,
            cap_ms: DEFAULT_BACKOFF_MAX_MS,
        }
    }
}

fn invalid(var: &str, raw: &str, reason: &str, expected: &str) -> Error {
    Error::Config(format!(
        "invalid {var}=\"{raw}\": {reason}; expected {expected}"
    ))
}

fn parse_in_range(var: &str, raw: &str, min: u64, max: u64, expected: &str) -> Result<u64, Error> {
    let n: u64 = raw
        .trim()
        .parse()
        .map_err(|_| invalid(var, raw, "not a valid non-negative integer", expected))?;
    if n < min || n > max {
        return Err(invalid(var, raw, "out of range", expected));
    }
    Ok(n)
}

impl EmbedRetryPolicy {
    /// Reads and validates the three env vars. An unset var keeps its default; an invalid value
    /// is an `Error::Config` naming the variable (never a silent fallback — same model as
    /// `LbugTuning::from_env`, #737). Called at service startup and by `replay_opts`.
    pub fn from_env() -> Result<Self, Error> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`Self::from_env`] with an injectable variable lookup (tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let mut p = Self::default();
        if let Some(raw) = get(RETRIES_ENV) {
            p.retries = parse_in_range(
                RETRIES_ENV,
                &raw,
                0,
                MAX_RETRIES as u64,
                "an integer 0-10 (0 disables retry and splitting)",
            )? as u32;
        }
        if let Some(raw) = get(BACKOFF_MS_ENV) {
            p.base_ms = parse_in_range(
                BACKOFF_MS_ENV,
                &raw,
                1,
                MAX_BACKOFF_MS,
                "an integer number of milliseconds, 1-60000",
            )?;
        }
        if let Some(raw) = get(BACKOFF_MAX_MS_ENV) {
            p.cap_ms = parse_in_range(
                BACKOFF_MAX_MS_ENV,
                &raw,
                1,
                MAX_BACKOFF_MAX_MS,
                "an integer number of milliseconds, 1-300000",
            )?;
        }
        if p.cap_ms < p.base_ms {
            return Err(Error::Config(format!(
                "invalid {BACKOFF_MAX_MS_ENV}={}: must be >= {BACKOFF_MS_ENV} ({}); expected a \
                 cap at least as large as the base backoff",
                p.cap_ms, p.base_ms
            )));
        }
        Ok(p)
    }

    /// One-line startup log description.
    pub fn describe(&self) -> String {
        if self.retries == 0 {
            "replay embedding retry disabled (LCG_REPLAY_EMBED_RETRIES=0)".to_string()
        } else {
            format!(
                "replay embedding retry: up to {} retries, backoff {}ms base, {}ms cap",
                self.retries, self.base_ms, self.cap_ms
            )
        }
    }

    /// Un-jittered backoff for 0-based retry number `attempt`: `min(cap, base * 2^attempt)`.
    pub fn backoff_ceiling(&self, attempt: u32) -> Duration {
        let shifted = self
            .base_ms
            .checked_shl(attempt.min(32))
            .unwrap_or(u64::MAX);
        Duration::from_millis(shifted.min(self.cap_ms))
    }

    /// Jittered backoff: uniform in `[ceiling/2, ceiling]`. `unit` is a number in `[0, 1]`.
    pub fn backoff_with_unit(&self, attempt: u32, unit: f64) -> Duration {
        let ceiling = self.backoff_ceiling(attempt);
        let half = ceiling / 2;
        half + (ceiling - half).mul_f64(unit.clamp(0.0, 1.0))
    }

    /// Maximum number of *transiently failing* embed calls (timeouts, dead connections — the
    /// slow kind) one window may spend across its whole retry/split tree:
    /// `(retries+1)*(depth+2) + depth` where `depth = ceil(log2(window_requests))` — enough for
    /// one always-transiently-failing text to be isolated down to a single request with slack,
    /// but not enough for a dead embedder to pay a full retry budget on every one of the up to
    /// `2n-1` sub-requests. Successful and permanently failing calls are not counted (they are
    /// prompt and structurally bounded). Once spent, every still-unresolved request in the
    /// window fails without a further call. With retries disabled the window gets exactly one
    /// call.
    pub fn call_budget(&self, requests: usize) -> u32 {
        if self.retries == 0 {
            return 1;
        }
        let depth = requests.max(1).next_power_of_two().trailing_zeros();
        (self.retries + 1) * (depth + 2) + depth
    }
}

fn random_unit() -> f64 {
    (uuid::Uuid::new_v4().as_u128() >> 75) as f64 / (1u128 << 53) as f64
}

/// Cross-window circuit breaker state, owned by one `replay_opts` run.
#[derive(Debug, Default)]
pub struct RetryBreaker {
    open: bool,
}

impl RetryBreaker {
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Result of one request's resolution.
pub type RequestResult = Result<Vec<f32>, String>;

/// Hooks `embed_with_retry` needs from its caller (injectable for deterministic tests).
pub struct RetryEnv<'a> {
    /// Polled before every attempt and between sleep slices; `true` aborts the window.
    pub cancel: &'a dyn Fn() -> bool,
    /// Sleeps for (at most) the given duration. Production passes `std::thread::sleep`.
    pub sleep: &'a dyn Fn(Duration),
    /// Source of jitter in `[0, 1]`.
    pub jitter: &'a dyn Fn() -> f64,
}

/// Production `sleep`/`jitter` hooks.
pub fn real_sleep(d: Duration) {
    std::thread::sleep(d);
}
pub fn real_jitter() -> f64 {
    random_unit()
}

/// Outcome of [`embed_with_retry`].
#[derive(Debug)]
pub enum RetryOutcome {
    /// One entry per input text, in order.
    Resolved(Vec<RequestResult>),
    /// `cancel` fired; nothing should be counted or recorded for this window.
    Cancelled,
}

struct Ctx<'a, F> {
    texts: &'a [&'a str],
    embed_fn: &'a F,
    policy: EmbedRetryPolicy,
    env: &'a RetryEnv<'a>,
    budget: u32,
    results: Vec<Option<RequestResult>>,
    transient_failures: usize,
    cancelled: bool,
}

impl<F> Ctx<'_, F>
where
    F: Fn(&[&str]) -> Result<Vec<Vec<f32>>, Error>,
{
    fn fail_range(&mut self, lo: usize, hi: usize, msg: &str, transient: bool) {
        for i in lo..hi {
            self.results[i] = Some(Err(msg.to_string()));
        }
        if transient {
            self.transient_failures += hi - lo;
        }
    }

    /// Sleeps `d` in slices, returning `false` if cancelled meanwhile.
    fn backoff_sleep(&mut self, d: Duration) -> bool {
        let mut left = d;
        while !left.is_zero() {
            if (self.env.cancel)() {
                self.cancelled = true;
                return false;
            }
            let step = left.min(SLEEP_SLICE);
            (self.env.sleep)(step);
            left -= step;
        }
        true
    }

    fn solve(&mut self, lo: usize, hi: usize) {
        let n = hi - lo;
        let mut attempt: u32 = 0;
        loop {
            // `cancel` is deliberately polled only on the failure paths (backoff sleeps and
            // split points), never before a first attempt: a healthy window must not add any
            // `cancel_fn` calls beyond the replay loop's own per-row polling.
            if self.cancelled {
                return;
            }
            if self.budget == 0 {
                self.fail_range(
                    lo,
                    hi,
                    "embed retry call budget exhausted for this window",
                    true,
                );
                return;
            }
            let (err_msg, class) = match (self.embed_fn)(&self.texts[lo..hi]) {
                Ok(vectors) if vectors.len() == n => {
                    for (slot, v) in self.results[lo..hi].iter_mut().zip(vectors) {
                        *slot = Some(Ok(v));
                    }
                    return;
                }
                Ok(vectors) => (
                    format!(
                        "embed batch returned {} vectors for {} requested texts",
                        vectors.len(),
                        n
                    ),
                    EmbedErrorClass::Permanent,
                ),
                Err(e) => (e.to_string(), classify_embed_error(&e)),
            };

            // Only a *transient* failure — the slow kind: a timeout or a dead connection — spends
            // the window budget. Successes and permanent errors return promptly and are bounded
            // structurally (a permanent error splits at most once per tree node, so at most
            // `2n-1` calls), and must not be starved by other rows' failures: with many poison
            // texts in one window their permanent errors would otherwise burn the budget and
            // fail innocent neighbours.
            if class == EmbedErrorClass::Transient {
                self.budget = self.budget.saturating_sub(1);
                // The budget is spent: the next loop iteration would fail this range without a
                // call, so skip the (up to `cap_ms`) backoff sleep and fail it now.
                if self.budget == 0 {
                    self.fail_range(
                        lo,
                        hi,
                        "embed retry call budget exhausted for this window",
                        true,
                    );
                    return;
                }
            }

            if class == EmbedErrorClass::Transient && attempt < self.policy.retries {
                let d = self.policy.backoff_with_unit(attempt, (self.env.jitter)());
                attempt += 1;
                if !self.backoff_sleep(d) {
                    return;
                }
                continue;
            }

            // Retries exhausted (transient) or not applicable (permanent).
            if n == 1 || self.policy.retries == 0 {
                self.fail_range(lo, hi, &err_msg, class == EmbedErrorClass::Transient);
                return;
            }
            let mid = lo + n / 2;
            for (a, b) in [(lo, mid), (mid, hi)] {
                if (self.env.cancel)() {
                    self.cancelled = true;
                    return;
                }
                self.solve(a, b);
                if self.cancelled {
                    return;
                }
            }
            return;
        }
    }
}

/// Resolves `texts` through `embed_fn` with the retry/split/budget policy described in the module
/// docs, updating the cross-window `breaker`. Returns one [`RequestResult`] per text, or
/// [`RetryOutcome::Cancelled`].
pub fn embed_with_retry<F>(
    texts: &[&str],
    policy: &EmbedRetryPolicy,
    breaker: &mut RetryBreaker,
    env: &RetryEnv<'_>,
    embed_fn: &F,
) -> RetryOutcome
where
    F: Fn(&[&str]) -> Result<Vec<Vec<f32>>, Error>,
{
    if texts.is_empty() {
        return RetryOutcome::Resolved(Vec::new());
    }
    // Breaker open: the previous window failed entirely and transiently — probe with a single
    // attempt, no retries, no splitting.
    let effective = if breaker.open {
        EmbedRetryPolicy {
            retries: 0,
            ..*policy
        }
    } else {
        *policy
    };
    let mut ctx = Ctx {
        texts,
        embed_fn,
        policy: effective,
        env,
        budget: effective.call_budget(texts.len()),
        results: (0..texts.len()).map(|_| None).collect(),
        transient_failures: 0,
        cancelled: false,
    };
    ctx.solve(0, texts.len());
    if ctx.cancelled {
        return RetryOutcome::Cancelled;
    }
    let any_ok = ctx.results.iter().any(|r| matches!(r, Some(Ok(_))));
    breaker.open = !any_ok && ctx.transient_failures == texts.len();
    RetryOutcome::Resolved(
        ctx.results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err("embed request left unresolved".to_string())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn env_of<'a>(
        cancel: &'a dyn Fn() -> bool,
        sleep: &'a dyn Fn(Duration),
        jitter: &'a dyn Fn() -> f64,
    ) -> RetryEnv<'a> {
        RetryEnv {
            cancel,
            sleep,
            jitter,
        }
    }

    fn vec_for(t: &str) -> Vec<f32> {
        vec![t.len() as f32, 1.0]
    }

    fn timeout_err() -> Error {
        Error::Ipc("UDS send request: timed out after 30000ms".to_string())
    }

    fn permanent_err() -> Error {
        Error::Ipc("UDS embedder returned status 400 Bad Request".to_string())
    }

    fn run<F>(texts: &[&str], policy: EmbedRetryPolicy, f: F) -> (Vec<RequestResult>, RetryBreaker)
    where
        F: Fn(&[&str]) -> Result<Vec<Vec<f32>>, Error>,
    {
        let mut breaker = RetryBreaker::default();
        let out = run_with(texts, policy, &mut breaker, f);
        (out, breaker)
    }

    fn run_with<F>(
        texts: &[&str],
        policy: EmbedRetryPolicy,
        breaker: &mut RetryBreaker,
        f: F,
    ) -> Vec<RequestResult>
    where
        F: Fn(&[&str]) -> Result<Vec<Vec<f32>>, Error>,
    {
        let cancel = || false;
        let sleep = |_d: Duration| {};
        let jitter = || 0.5;
        match embed_with_retry(
            texts,
            &policy,
            breaker,
            &env_of(&cancel, &sleep, &jitter),
            &f,
        ) {
            RetryOutcome::Resolved(r) => r,
            RetryOutcome::Cancelled => panic!("unexpected cancel"),
        }
    }

    fn policy(retries: u32) -> EmbedRetryPolicy {
        EmbedRetryPolicy {
            retries,
            base_ms: 10,
            cap_ms: 100,
        }
    }

    #[test]
    fn transient_then_success_within_budget() {
        let calls = Cell::new(0u32);
        let (out, breaker) = run(&["a", "bb", "ccc"], policy(4), |t| {
            calls.set(calls.get() + 1);
            if calls.get() <= 3 {
                Err(timeout_err())
            } else {
                Ok(t.iter().map(|x| vec_for(x)).collect())
            }
        });
        assert_eq!(calls.get(), 4);
        assert!(out.iter().all(|r| r.is_ok()));
        assert_eq!(out[1].as_ref().unwrap(), &vec_for("bb"));
        assert!(!breaker.is_open());
    }

    #[test]
    fn permanent_single_request_makes_exactly_one_attempt() {
        let calls = Cell::new(0u32);
        let (out, _) = run(&["only"], policy(4), |_| {
            calls.set(calls.get() + 1);
            Err(permanent_err())
        });
        assert_eq!(calls.get(), 1, "a permanent error must not be retried");
        assert!(out[0].is_err());
    }

    #[test]
    fn single_request_exhausts_retries_then_fails_without_split() {
        let calls = Cell::new(0u32);
        let (out, breaker) = run(&["only"], policy(4), |_| {
            calls.set(calls.get() + 1);
            Err(timeout_err())
        });
        assert_eq!(calls.get(), 5, "1 attempt + 4 retries");
        assert!(out[0].is_err());
        assert!(
            breaker.is_open(),
            "all-transient total failure opens the breaker"
        );
    }

    #[test]
    fn poison_text_transient_fails_only_its_own_request() {
        let texts: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let calls = Cell::new(0u32);
        let (out, breaker) = run(&refs, policy(4), |t| {
            calls.set(calls.get() + 1);
            if t.contains(&"t17") {
                Err(timeout_err())
            } else {
                Ok(t.iter().map(|x| vec_for(x)).collect())
            }
        });
        let failed: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_err())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(failed, vec![17]);
        assert!(!breaker.is_open());
        assert!(calls.get() <= policy(4).call_budget(64));
    }

    #[test]
    fn poison_text_permanent_batch_error_splits_once_without_retrying() {
        let texts: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let calls = Cell::new(0u32);
        let seen: RefCell<Vec<usize>> = RefCell::new(Vec::new());
        let (out, _) = run(&refs, policy(4), |t| {
            calls.set(calls.get() + 1);
            seen.borrow_mut().push(t.len());
            if t.contains(&"t40") {
                Err(permanent_err())
            } else {
                Ok(t.iter().map(|x| vec_for(x)).collect())
            }
        });
        let failed: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_err())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(failed, vec![40]);
        // Never retried: 1 root + 1 per split level on the poison path (6) + sibling halves (6).
        assert_eq!(calls.get(), 13);
    }

    #[test]
    fn many_permanent_poison_texts_do_not_starve_innocent_neighbours() {
        let texts: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        // 40 of 64 texts are poison (permanent error); the rest must still be embedded.
        let is_bad = |t: &str| t[1..].parse::<usize>().unwrap() % 8 < 5;
        let (out, _) = run(&refs, policy(4), |t| {
            if t.iter().any(|x| is_bad(x)) {
                Err(permanent_err())
            } else {
                Ok(t.iter().map(|x| vec_for(x)).collect())
            }
        });
        for (i, r) in out.iter().enumerate() {
            assert_eq!(r.is_err(), is_bad(&texts[i]), "row {i}");
        }
    }

    #[test]
    fn dead_embedder_is_bounded_by_the_window_budget() {
        let texts: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let calls = Cell::new(0u32);
        let (out, breaker) = run(&refs, policy(4), |_| {
            calls.set(calls.get() + 1);
            Err(timeout_err())
        });
        assert!(out.iter().all(|r| r.is_err()));
        assert_eq!(calls.get(), policy(4).call_budget(64));
        assert!(breaker.is_open());
    }

    #[test]
    fn spent_budget_skips_the_final_backoff_sleep() {
        // Dead embedder, 64-row window: the window budget is the limiter. The call that spends
        // the budget must not be followed by a backoff sleep — the retry it would wait for can
        // never be made.
        let events: RefCell<Vec<char>> = RefCell::new(Vec::new());
        let cancel = || false;
        let sleep = |_d: Duration| events.borrow_mut().push('S');
        let jitter = || 0.5;
        let f = |_: &[&str]| {
            events.borrow_mut().push('C');
            Err(timeout_err())
        };
        let texts: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let pol = policy(4);
        let mut breaker = RetryBreaker::default();
        let _ = embed_with_retry(
            &refs,
            &pol,
            &mut breaker,
            &env_of(&cancel, &sleep, &jitter),
            &f,
        );
        let ev = events.borrow();
        assert_eq!(
            ev.iter().filter(|c| **c == 'C').count() as u32,
            pol.call_budget(64)
        );
        assert_eq!(
            ev.last(),
            Some(&'C'),
            "a backoff sleep trailed the last call"
        );
    }

    #[test]
    fn open_breaker_gives_one_attempt_then_closes_on_success() {
        let mut breaker = RetryBreaker { open: true };
        let calls = Cell::new(0u32);
        let out = run_with(&["a", "b"], policy(4), &mut breaker, |_| {
            calls.set(calls.get() + 1);
            Err(timeout_err())
        });
        assert_eq!(
            calls.get(),
            1,
            "no retry and no split while the breaker is open"
        );
        assert!(out.iter().all(|r| r.is_err()));
        assert!(breaker.is_open());

        let out = run_with(&["a", "b"], policy(4), &mut breaker, |t| {
            Ok(t.iter().map(|x| vec_for(x)).collect())
        });
        assert!(out.iter().all(|r| r.is_ok()));
        assert!(!breaker.is_open());
    }

    #[test]
    fn retries_zero_disables_retry_and_splitting() {
        let calls = Cell::new(0u32);
        let (out, _) = run(&["a", "b", "c"], policy(0), |_| {
            calls.set(calls.get() + 1);
            Err(timeout_err())
        });
        assert_eq!(calls.get(), 1);
        assert!(out.iter().all(|r| r.is_err()));
    }

    #[test]
    fn length_mismatch_is_permanent_and_splits() {
        let calls = Cell::new(0u32);
        let (out, _) = run(&["a", "b"], policy(4), |t| {
            calls.set(calls.get() + 1);
            if t.len() == 2 {
                Ok(vec![vec![1.0]])
            } else {
                Ok(t.iter().map(|x| vec_for(x)).collect())
            }
        });
        assert_eq!(calls.get(), 3);
        assert!(out.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn cancel_during_backoff_aborts_the_window() {
        let polls = Cell::new(0u32);
        let cancel = || {
            polls.set(polls.get() + 1);
            polls.get() > 2
        };
        let sleep = |_d: Duration| {};
        let jitter = || 0.5;
        let mut breaker = RetryBreaker::default();
        let calls = Cell::new(0u32);
        let f = |_t: &[&str]| -> Result<Vec<Vec<f32>>, Error> {
            calls.set(calls.get() + 1);
            Err(timeout_err())
        };
        let out = embed_with_retry(
            &["a"],
            &policy(4),
            &mut breaker,
            &env_of(&cancel, &sleep, &jitter),
            &f,
        );
        assert!(matches!(out, RetryOutcome::Cancelled));
        assert!(calls.get() < 5);
        assert!(
            !breaker.is_open(),
            "a cancelled window must not move the breaker"
        );
    }

    #[test]
    fn backoff_is_exponential_capped_and_jittered_within_bounds() {
        let p = EmbedRetryPolicy {
            retries: 4,
            base_ms: 500,
            cap_ms: 3000,
        };
        assert_eq!(p.backoff_ceiling(0), Duration::from_millis(500));
        assert_eq!(p.backoff_ceiling(1), Duration::from_millis(1000));
        assert_eq!(p.backoff_ceiling(2), Duration::from_millis(2000));
        assert_eq!(p.backoff_ceiling(3), Duration::from_millis(3000));
        assert_eq!(p.backoff_ceiling(40), Duration::from_millis(3000));
        assert_eq!(p.backoff_with_unit(1, 0.0), Duration::from_millis(500));
        assert_eq!(p.backoff_with_unit(1, 1.0), Duration::from_millis(1000));
        for _ in 0..50 {
            let d = p.backoff_with_unit(2, random_unit());
            assert!(d >= Duration::from_millis(1000) && d <= Duration::from_millis(2000));
        }
    }

    #[test]
    fn total_sleep_is_bounded_by_attempts_times_cap() {
        let slept = Cell::new(Duration::ZERO);
        let cancel = || false;
        let sleep = |d: Duration| slept.set(slept.get() + d);
        let jitter = || 1.0;
        let p = EmbedRetryPolicy {
            retries: 4,
            base_ms: 500,
            cap_ms: 1000,
        };
        let mut breaker = RetryBreaker::default();
        let f = |_t: &[&str]| -> Result<Vec<Vec<f32>>, Error> { Err(timeout_err()) };
        let _ = embed_with_retry(
            &["a"],
            &p,
            &mut breaker,
            &env_of(&cancel, &sleep, &jitter),
            &f,
        );
        // 500 + 1000 + 1000 + 1000
        assert_eq!(slept.get(), Duration::from_millis(3500));
    }

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn from_lookup_defaults() {
        let p = EmbedRetryPolicy::from_lookup(lookup(&[])).unwrap();
        assert_eq!(p, EmbedRetryPolicy::default());
        assert_eq!(p.retries, 4);
        assert_eq!(p.base_ms, 500);
        assert_eq!(p.cap_ms, 10_000);
    }

    #[test]
    fn from_lookup_accepts_valid_values() {
        let p = EmbedRetryPolicy::from_lookup(lookup(&[
            (RETRIES_ENV, "0"),
            (BACKOFF_MS_ENV, "100"),
            (BACKOFF_MAX_MS_ENV, "100"),
        ]))
        .unwrap();
        assert_eq!((p.retries, p.base_ms, p.cap_ms), (0, 100, 100));
    }

    #[test]
    fn from_lookup_rejects_invalid_values_naming_the_variable() {
        for (pairs, var) in [
            (&[(RETRIES_ENV, "abc")][..], RETRIES_ENV),
            (&[(RETRIES_ENV, "11")][..], RETRIES_ENV),
            (&[(RETRIES_ENV, "-1")][..], RETRIES_ENV),
            (&[(BACKOFF_MS_ENV, "0")][..], BACKOFF_MS_ENV),
            (&[(BACKOFF_MS_ENV, "60001")][..], BACKOFF_MS_ENV),
            (&[(BACKOFF_MS_ENV, "")][..], BACKOFF_MS_ENV),
            (&[(BACKOFF_MAX_MS_ENV, "300001")][..], BACKOFF_MAX_MS_ENV),
            (&[(BACKOFF_MAX_MS_ENV, "x")][..], BACKOFF_MAX_MS_ENV),
            (
                &[(BACKOFF_MS_ENV, "2000"), (BACKOFF_MAX_MS_ENV, "1000")][..],
                BACKOFF_MAX_MS_ENV,
            ),
        ] {
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
            let err = EmbedRetryPolicy::from_lookup(|k| {
                owned.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
            })
            .unwrap_err();
            assert!(
                matches!(&err, Error::Config(m) if m.contains(var)),
                "{pairs:?} -> {err}"
            );
        }
    }

    #[test]
    fn call_budget_grows_with_window_depth_but_stays_small() {
        let p = policy(4);
        assert_eq!(p.call_budget(1), 5 * 2);
        assert_eq!(p.call_budget(64), 5 * 8 + 6);
        assert_eq!(policy(0).call_budget(64), 1);
        assert!(p.call_budget(256) < 100);
    }
}
