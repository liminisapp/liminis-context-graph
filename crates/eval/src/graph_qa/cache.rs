//! On-disk cache of planner and reader responses (FR-020), keyed by the model plus the full prompt.
//!
//! Append-only JSONL, one `{k, role, model, v}` per response, written (and flushed) as each
//! response arrives, so an aborted run keeps everything it already paid for. The key is
//! `sha256(json([model, system, user, schema-as-sent]))`: any byte of the prompt, the schema or the
//! model changes it. Hits and paid calls are counted per role.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::llm_client::{close_schema, ModelClient, ModelRequest};

#[derive(Serialize, Deserialize)]
struct Entry {
    k: String,
    role: String,
    model: String,
    v: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub cache_hits: usize,
    pub paid_calls: usize,
}

pub struct LlmCache {
    path: PathBuf,
    entries: Mutex<HashMap<String, String>>,
    counts: Mutex<BTreeMap<String, Counts>>,
}

/// The cache key of `req` (the schema as it is sent, i.e. closed).
pub fn key(req: &ModelRequest) -> String {
    let schema = req.schema.as_ref().map(close_schema);
    let k = json!([req.model, req.system, req.user, schema]).to_string();
    let mut h = Sha256::new();
    h.update(k.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl LlmCache {
    /// Loads `path` if it exists (unparseable lines are skipped, as a truncated last line after a
    /// crash must not lose the rest).
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut entries = HashMap::new();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                for line in text.lines() {
                    if let Ok(e) = serde_json::from_str::<Entry>(line) {
                        entries.insert(e.k, e.v);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("--llm-cache {}: {e}", path.display())),
        }
        Ok(Self {
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
            counts: Mutex::default(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("cache lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn counts(&self) -> BTreeMap<String, Counts> {
        self.counts.lock().expect("cache lock").clone()
    }

    fn bump(&self, role: &str, hit: bool) {
        let mut c = self.counts.lock().expect("cache lock");
        let e = c.entry(role.to_string()).or_default();
        if hit {
            e.cache_hits += 1;
        } else {
            e.paid_calls += 1;
        }
    }

    /// The cached response for `req`, if any (counted as a hit). Never calls a model.
    pub fn lookup(&self, role: &str, req: &ModelRequest) -> Option<String> {
        let v = self
            .entries
            .lock()
            .expect("cache lock")
            .get(&key(req))
            .cloned();
        if v.is_some() {
            self.bump(role, true);
        }
        v
    }

    /// The cached response for `req`, else `client`'s (then cached). A client error is returned
    /// and nothing is cached.
    pub async fn get_or_call(
        &self,
        role: &str,
        client: &dyn ModelClient,
        req: &ModelRequest,
    ) -> Result<String, String> {
        let k = key(req);
        if let Some(v) = self.entries.lock().expect("cache lock").get(&k).cloned() {
            self.bump(role, true);
            return Ok(v);
        }
        let v = client.complete(req).await?;
        self.bump(role, false);
        let line = serde_json::to_string(&Entry {
            k: k.clone(),
            role: role.to_string(),
            model: req.model.clone(),
            v: v.clone(),
        })
        .expect("serializes");
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .map_err(|e| format!("--llm-cache {}: {e}", self.path.display()))?;
            writeln!(f, "{line}")
                .and_then(|_| f.flush())
                .map_err(|e| format!("--llm-cache {}: {e}", self.path.display()))?;
        }
        self.entries
            .lock()
            .expect("cache lock")
            .insert(k, v.clone());
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_qa::mock::ScriptedModel;

    fn req() -> ModelRequest {
        ModelRequest {
            model: "m".into(),
            system: Some("sys".into()),
            user: "Question: q".into(),
            schema: Some(json!({"type": "object"})),
            max_tokens: 10,
        }
    }

    #[test]
    fn any_byte_of_the_prompt_or_the_model_changes_the_key() {
        let base = key(&req());
        let mut r = req();
        r.user.push(' ');
        assert_ne!(key(&r), base);
        let mut r = req();
        r.system = Some("sys.".into());
        assert_ne!(key(&r), base);
        let mut r = req();
        r.model = "m2".into();
        assert_ne!(key(&r), base);
        let mut r = req();
        r.schema = Some(json!({"type": "array"}));
        assert_ne!(key(&r), base);
        // max_tokens is not part of the prompt.
        let mut r = req();
        r.max_tokens = 99;
        assert_eq!(key(&r), base);
    }

    #[tokio::test]
    async fn a_second_lookup_is_a_hit_and_survives_a_reload() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.jsonl");
        let m = ScriptedModel::new(|_| Ok("v".into()));
        let c = LlmCache::load(&p).unwrap();
        assert_eq!(c.get_or_call("reader", &m, &req()).await.unwrap(), "v");
        assert_eq!(c.get_or_call("reader", &m, &req()).await.unwrap(), "v");
        assert_eq!(m.calls(), 1);
        assert_eq!(
            c.counts()["reader"],
            Counts {
                cache_hits: 1,
                paid_calls: 1
            }
        );
        let c2 = LlmCache::load(&p).unwrap();
        assert_eq!(c2.get_or_call("reader", &m, &req()).await.unwrap(), "v");
        assert_eq!(m.calls(), 1);
        // An error is not cached.
        let bad = ScriptedModel::new(|_| Err("down".into()));
        let mut r = req();
        r.user = "other".into();
        assert!(c2.get_or_call("reader", &bad, &r).await.is_err());
        assert_eq!(c2.len(), 1);
    }
}
