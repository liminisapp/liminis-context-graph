//! A scripted [`ModelClient`] for tests (FR-021): answers from a closure, counts calls, and never
//! touches the network.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;

use crate::llm_client::{ModelClient, ModelRequest};

type Script = Box<dyn Fn(&ModelRequest) -> Result<String, String> + Send + Sync>;

pub struct ScriptedModel {
    script: Script,
    calls: AtomicUsize,
    /// Every request seen, in call order.
    pub log: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    pub fn new(
        f: impl Fn(&ModelRequest) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            script: Box::new(f),
            calls: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl ModelClient for ScriptedModel {
    fn complete<'a>(&'a self, req: &'a ModelRequest) -> BoxFuture<'a, Result<String, String>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.log.lock().expect("log lock").push(req.clone());
        let r = (self.script)(req);
        Box::pin(async move { r })
    }
}

/// A client for when no provider is configured: every call is an error, so a fully cached run
/// still works and an uncached one fails with a clear message.
pub struct Unconfigured;

impl ModelClient for Unconfigured {
    fn complete<'a>(&'a self, _req: &'a ModelRequest) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async {
            Err(
                "no model provider configured (set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN, or \
                 an Anthropic-compatible --anthropic-base-url / ANTHROPIC_BASE_URL) and the \
                 response is not in --llm-cache"
                    .to_string(),
            )
        })
    }
}
