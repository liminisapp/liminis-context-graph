//! Grounded-evidence judge (FR-013, FR-020).
//!
//! A separate trait and cache from the extraction judge (`JudgeClient` / `JudgeCache`) so that
//! cache's untagged-enum invariant is untouched. The judge is **blind**: it sees only a key fact
//! and one returned text, never which system or configuration produced it.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::judge::AnthropicJudgeClient;

/// Bump when the prompt or schema changes, so stale cache entries are never served.
pub const PROMPT_VERSION: &str = "evidence-v1";

pub const EVIDENCE_PROMPT: &str = "You are checking whether a passage of retrieved text supports a specific fact.

FACT:
{key_fact}

PASSAGE:
{evidence}

Decide whether the passage, taken on its own, states or directly implies the FACT. Judge only the content of the passage. A passage that is merely on the same topic, or that mentions the same entities without stating the fact, does NOT support it. A passage that states an older or superseded version of the fact (for example a decision that was later replaced) does NOT support the current fact unless it says the same thing.

Respond with ONLY this JSON:
{\"supports\": true|false, \"rationale\": \"one short sentence\"}";

pub trait EvidenceJudge: Send + Sync {
    fn model(&self) -> &str;
    fn supports<'a>(
        &'a self,
        key_fact: &'a str,
        evidence: &'a str,
    ) -> BoxFuture<'a, Result<bool, String>>;
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "supports": {"type": "boolean"},
            "rationale": {"type": "string"},
        },
        "required": ["supports", "rationale"],
        "additionalProperties": false
    })
}

#[derive(Deserialize)]
struct RawVerdict {
    supports: bool,
}

pub fn parse_verdict(text: &str) -> Result<bool, String> {
    let start = text.find('{');
    let end = text.rfind('}');
    let slice = match (start, end) {
        (Some(s), Some(e)) if s < e => &text[s..=e],
        _ => text,
    };
    serde_json::from_str::<RawVerdict>(slice)
        .map(|v| v.supports)
        .map_err(|e| format!("evidence judge response not valid JSON ({e}): {text}"))
}

impl EvidenceJudge for AnthropicJudgeClient {
    fn model(&self) -> &str {
        AnthropicJudgeClient::model(self)
    }

    fn supports<'a>(
        &'a self,
        key_fact: &'a str,
        evidence: &'a str,
    ) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let prompt = EVIDENCE_PROMPT
                .replace("{key_fact}", key_fact)
                .replace("{evidence}", evidence);
            let text = self.request_judge_text(prompt, schema()).await?;
            parse_verdict(&text)
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CacheLine {
    key: String,
    supports: bool,
}

pub fn cache_key(model: &str, key_fact: &str, evidence: &str) -> String {
    let canon = json!({
        "prompt_version": PROMPT_VERSION,
        "judge_model": model,
        "key_fact": key_fact,
        "evidence": evidence,
    });
    let bytes = serde_json::to_vec(&canon).expect("json! value serializes");
    lcg_core::sha256_hex::sha256_hex(&bytes)[..24].to_string()
}

/// Wraps an [`EvidenceJudge`] with an append-only JSONL verdict cache so re-runs are free.
pub struct CachedEvidenceJudge<'a> {
    inner: &'a dyn EvidenceJudge,
    path: PathBuf,
    entries: Mutex<HashMap<String, bool>>,
    paid_calls: AtomicUsize,
    cache_hits: AtomicUsize,
}

impl<'a> CachedEvidenceJudge<'a> {
    pub fn load(inner: &'a dyn EvidenceJudge, path: &Path) -> Result<Self, String> {
        let mut entries = HashMap::new();
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read evidence cache {}: {e}", path.display()))?;
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let l: CacheLine = serde_json::from_str(line).map_err(|e| {
                    format!("evidence cache {} line {}: {e}", path.display(), i + 1)
                })?;
                entries.insert(l.key, l.supports);
            }
        }
        Ok(Self {
            inner,
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
            paid_calls: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
        })
    }

    pub fn model(&self) -> &str {
        self.inner.model()
    }
    pub fn paid_calls(&self) -> usize {
        self.paid_calls.load(Ordering::Relaxed)
    }
    pub fn cache_hits(&self) -> usize {
        self.cache_hits.load(Ordering::Relaxed)
    }

    pub async fn supports(&self, key_fact: &str, evidence: &str) -> Result<bool, String> {
        let key = cache_key(self.inner.model(), key_fact, evidence);
        if let Some(v) = self.entries.lock().expect("cache lock").get(&key) {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(*v);
        }
        let v = self.inner.supports(key_fact, evidence).await?;
        self.paid_calls.fetch_add(1, Ordering::Relaxed);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("cannot append evidence cache {}: {e}", self.path.display()))?;
        let line = serde_json::to_string(&CacheLine {
            key: key.clone(),
            supports: v,
        })
        .expect("cache line serializes");
        writeln!(f, "{line}").map_err(|e| format!("evidence cache write failed: {e}"))?;
        self.entries.lock().expect("cache lock").insert(key, v);
        Ok(v)
    }

    /// 0-based rank of the first of `texts` that supports `key_fact`, stopping at the first hit
    /// (fewer paid calls). `Ok(None)` when none does.
    pub async fn first_supporting_rank(
        &self,
        key_fact: &str,
        texts: &[String],
    ) -> Result<Option<usize>, String> {
        for (i, t) in texts.iter().enumerate() {
            if self.supports(key_fact, t).await? {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// Canned judge: supports iff the evidence contains the key fact (case-insensitive).
    pub struct SubstringJudge {
        pub calls: AtomicUsize,
    }
    impl Default for SubstringJudge {
        fn default() -> Self {
            Self::new()
        }
    }
    impl SubstringJudge {
        pub fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }
    impl EvidenceJudge for SubstringJudge {
        fn model(&self) -> &str {
            "substring-test-judge"
        }
        fn supports<'a>(&'a self, k: &'a str, e: &'a str) -> BoxFuture<'a, Result<bool, String>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(e.to_lowercase().contains(&k.to_lowercase())) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::SubstringJudge;
    use super::*;

    #[test]
    fn prompt_has_placeholders_and_is_blind() {
        assert!(EVIDENCE_PROMPT.contains("{key_fact}") && EVIDENCE_PROMPT.contains("{evidence}"));
        let lower = EVIDENCE_PROMPT.to_lowercase();
        assert!(!lower.contains("qmd") && !lower.contains("lcg"));
    }

    #[test]
    fn verdict_parsing() {
        assert!(parse_verdict(r#"{"supports": true, "rationale": "x"}"#).unwrap());
        assert!(!parse_verdict("noise {\"supports\": false, \"rationale\": \"y\"} tail").unwrap());
        assert!(parse_verdict("nope").is_err());
    }

    #[test]
    fn cache_key_depends_on_every_input() {
        let a = cache_key("m", "f", "e");
        assert_eq!(a, cache_key("m", "f", "e"));
        assert_ne!(a, cache_key("m2", "f", "e"));
        assert_ne!(a, cache_key("m", "f2", "e"));
        assert_ne!(a, cache_key("m", "f", "e2"));
    }

    #[tokio::test]
    async fn cache_round_trips_and_avoids_repeat_calls() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("ev.jsonl");
        let inner = SubstringJudge::new();
        {
            let j = CachedEvidenceJudge::load(&inner, &path).unwrap();
            assert!(j.supports("fact", "this has a Fact in it").await.unwrap());
            assert!(!j.supports("fact", "nothing").await.unwrap());
            assert!(j.supports("fact", "this has a Fact in it").await.unwrap());
            assert_eq!((j.paid_calls(), j.cache_hits()), (2, 1));
        }
        // A fresh load serves from the file with zero paid calls.
        let j2 = CachedEvidenceJudge::load(&inner, &path).unwrap();
        assert!(j2.supports("fact", "this has a Fact in it").await.unwrap());
        assert_eq!(j2.paid_calls(), 0);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn first_supporting_rank_stops_early() {
        let d = tempfile::tempdir().unwrap();
        let inner = SubstringJudge::new();
        let j = CachedEvidenceJudge::load(&inner, &d.path().join("c.jsonl")).unwrap();
        let texts: Vec<String> = ["x", "has fact", "also fact"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            j.first_supporting_rank("fact", &texts).await.unwrap(),
            Some(1)
        );
        assert_eq!(j.paid_calls(), 2);
        assert_eq!(j.first_supporting_rank("zzz", &texts).await.unwrap(), None);
    }

    #[test]
    fn bad_cache_line_is_reported_with_line_number() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.jsonl");
        std::fs::write(&p, "{\"key\":\"a\",\"supports\":true}\nnot json\n").unwrap();
        let inner = SubstringJudge::new();
        let e = CachedEvidenceJudge::load(&inner, &p).err().unwrap();
        assert!(e.contains("line 2"), "{e}");
    }
}
