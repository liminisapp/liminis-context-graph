//! The report's `inputs` block (issue #872, schema version 1): a digest of every input that can
//! change a retrieval run's results, so a later `--compare` can tell whether two runs measured the
//! same thing without re-deriving it.
//!
//! Every digest is a full 64-hex sha256 in JSON (16 hex for display, see [`short`]), or one of two
//! literals: [`NONE`] (the input is not used by this run) and [`UNKNOWN`] (it is used but could not
//! be identified). The comparison rules for these literals live in `compare.rs`.
//!
//! Each digest is computed from *only* its own input, so that changing one input changes exactly
//! one entry (SC-002): the extractor identity carries no cassette path, and the effective-env
//! digest excludes `LCG_REPLAY_LLM` / `LCG_RECORD_LLM` (the `cassette` digest owns those).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::lcg::EmbedderIdentity;

pub const SCHEMA_VERSION: u64 = 1;
/// The input is not used by this run (no cassette, no variants file, no LCG service).
pub const NONE: &str = "none";
/// The input is used but its identity could not be determined.
pub const UNKNOWN: &str = "unknown";
/// Hex characters shown in Markdown; the JSON carries the full digest.
pub const DISPLAY_LEN: usize = 16;

/// The `inputs` entries, in report order. `corpus` and `questions` are the full-length forms of
/// `meta.corpus_hash` / `meta.questions_hash`.
pub const KEYS: [&str; 8] = [
    "corpus",
    "questions",
    "variants",
    "cassette",
    "embedder_model",
    "extractor",
    "service_binary",
    "service_env",
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputDigests {
    pub corpus: String,
    pub questions: String,
    pub variants: String,
    pub cassette: String,
    pub embedder_model: String,
    pub extractor: String,
    pub service_binary: String,
    pub service_env: String,
    /// Sorted names (never values) of the `LCG_*` / `GRAPHITI_*` variables in the digest, so a `service_env`
    /// mismatch is explainable.
    pub service_env_names: Vec<String>,
}

impl InputDigests {
    pub fn get(&self, key: &str) -> Option<&str> {
        Some(match key {
            "corpus" => &self.corpus,
            "questions" => &self.questions,
            "variants" => &self.variants,
            "cassette" => &self.cassette,
            "embedder_model" => &self.embedder_model,
            "extractor" => &self.extractor,
            "service_binary" => &self.service_binary,
            "service_env" => &self.service_env,
            _ => return None,
        })
    }

    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("schema_version".into(), json!(SCHEMA_VERSION));
        for k in KEYS {
            o.insert(k.into(), json!(self.get(k).unwrap_or(UNKNOWN)));
        }
        o.insert("service_env_names".into(), json!(self.service_env_names));
        Value::Object(o)
    }
}

/// First [`DISPLAY_LEN`] characters of a digest (`none` / `unknown` pass through unchanged).
pub fn short(d: &str) -> &str {
    let end = d
        .char_indices()
        .nth(DISPLAY_LEN)
        .map(|(i, _)| i)
        .unwrap_or(d.len());
    &d[..end]
}

/// Streamed sha256 of a file's bytes (constant memory for a large binary or cassette).
pub fn file_digest(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(lcg_core::sha256_hex::to_hex(&h.finalize()))
}

/// Digest of an optional file: `none` when there is no such input, `unknown` when it is named but
/// unreadable.
pub fn optional_file_digest(path: Option<&Path>) -> String {
    match path {
        None => NONE.to_string(),
        Some(p) => file_digest(p).unwrap_or_else(|_| UNKNOWN.to_string()),
    }
}

fn sha_of(domain: &str, parts: &[&str]) -> String {
    let mut buf = String::from(domain);
    for p in parts {
        buf.push('\0');
        buf.push_str(p);
    }
    lcg_core::sha256_hex::sha256_hex(buf.as_bytes())
}

/// Order-independent combination of per-run digests (one per embedding profile): sorted and
/// de-duplicated, then returned as-is when there is one and hashed together when there are more.
fn combine(mut digests: Vec<String>) -> String {
    digests.sort();
    digests.dedup();
    match digests.len() {
        0 => NONE.to_string(),
        1 => digests.remove(0),
        _ => sha_of(
            "combined",
            &digests.iter().map(String::as_str).collect::<Vec<_>>(),
        ),
    }
}

/// Embedder model digest (FR-005): sha256 of the model name, dimension and effective instruction
/// profile the service reported in `knowledge_status`. The service does not expose model files, so
/// this identifies the model by what it says it is. `none` with no LCG service; `unknown` if any
/// service did not report a model.
pub fn embedder_digest(ids: &[EmbedderIdentity]) -> String {
    if ids.is_empty() {
        return NONE.to_string();
    }
    let mut per = Vec::new();
    for id in ids {
        let Some(model) = &id.model else {
            return UNKNOWN.to_string();
        };
        let dim = id.dim.map(|d| d.to_string()).unwrap_or_else(|| "-".into());
        let profile = id.profile.clone().unwrap_or_else(|| "-".into());
        per.push(sha_of("embedder", &[model, &dim, &profile]));
    }
    combine(per)
}

/// Path-free identity of the extraction backend: replay of a cassette (the `cassette` digest owns
/// the cassette), an explicit endpoint with credentials stripped, or the service default.
pub fn extractor_identity(replay: bool, endpoint: Option<&str>) -> String {
    if replay {
        "cassette-replay".to_string()
    } else if let Some(e) = endpoint {
        format!("endpoint:{}", super::workspace::redact_endpoint(e))
    } else {
        "service-default".to_string()
    }
}

/// `none` with no LCG service, else sha256 of the identity.
pub fn extractor_digest(identity: Option<&str>) -> String {
    match identity {
        None => NONE.to_string(),
        Some(i) => sha_of("extractor", &[i]),
    }
}

/// Resolves a service binary: a path with a separator is used as given; a bare name is searched
/// for on `PATH`.
pub fn resolve_binary(bin: &Path, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if bin.components().count() > 1 || bin.is_absolute() {
        return bin.exists().then(|| bin.to_path_buf());
    }
    std::env::split_paths(path_var?)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// Service binary digest: streamed sha256 of the resolved binary's bytes; failing that, sha256 of
/// its `--version` string; failing that, `unknown`. `none` when no service runs.
pub fn service_binary_digest(
    bin: Option<(&Path, &str)>,
    path_var: Option<&std::ffi::OsStr>,
) -> String {
    let Some((bin, version)) = bin else {
        return NONE.to_string();
    };
    if let Some(p) = resolve_binary(bin, path_var) {
        if let Ok(d) = file_digest(&p) {
            return d;
        }
    }
    if !version.is_empty() && version != UNKNOWN {
        return sha_of("service-version", &[version]);
    }
    UNKNOWN.to_string()
}

/// Variables whose values are credentials or endpoint locations: recorded by name only.
fn is_secret_name(name: &str) -> bool {
    ["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"]
        .iter()
        .any(|s| name.ends_with(s))
        || name.contains("URL")
        || name.contains("ENDPOINT")
}

/// Variables the service reads: the `LCG_*` names and the legacy `GRAPHITI_*` spellings it still
/// accepts as fallbacks (`lcg_core::env::lcg_env_var`), which change its behaviour just the same.
fn is_service_var(name: &str) -> bool {
    name.starts_with("LCG_") || name.starts_with("GRAPHITI_")
}

/// Variables the `cassette` digest owns; excluded here so a changed cassette path changes one entry.
fn is_cassette_var(name: &str) -> bool {
    name == "LCG_REPLAY_LLM" || name == "LCG_RECORD_LLM"
}

/// Effective-environment digest over the `LCG_*` and legacy `GRAPHITI_*` variables in each child's environment (as
/// computed by `lcg::effective_service_env`, never from `std::env` directly). Each run yields the
/// sorted `KEY=value` lines, with secret-bearing names contributing `KEY=<redacted>` so that no
/// low-entropy secret is hashed into a shared report. Returns the digest and the sorted union of
/// names. `none` with no LCG service.
pub fn service_env_digest(envs: &[BTreeMap<String, String>]) -> (String, Vec<String>) {
    if envs.is_empty() {
        return (NONE.to_string(), Vec::new());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut per = Vec::new();
    for env in envs {
        let lines: Vec<String> = env
            .iter()
            .filter(|(k, _)| is_service_var(k) && !is_cassette_var(k))
            .map(|(k, v)| {
                names.insert(k.clone());
                if is_secret_name(k) {
                    format!("{k}=<redacted>")
                } else {
                    format!("{k}={v}")
                }
            })
            .collect();
        per.push(sha_of(
            "service-env",
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }
    (combine(per), names.into_iter().collect())
}

/// Everything [`InputDigests::build`] reads.
pub struct InputSources<'a> {
    pub corpus: &'a str,
    pub questions: &'a str,
    /// `<corpus>/query-variants.jsonl` when a variants config ran, else `None`.
    pub variants_file: Option<&'a Path>,
    /// The `--replay-cassette` / `--record-cassette` file; read at build time, so build after ingest.
    pub cassette_file: Option<&'a Path>,
    /// One identity per spawned service (per embedding profile); empty when none ran.
    pub embedders: &'a [EmbedderIdentity],
    /// `None` when no LCG service ran.
    pub extractor_identity: Option<&'a str>,
    /// `(binary, --version string)`; `None` when no LCG service ran.
    pub service_bin: Option<(&'a Path, &'a str)>,
    pub path_var: Option<&'a std::ffi::OsStr>,
    /// Each child's effective environment.
    pub envs: &'a [BTreeMap<String, String>],
}

impl InputDigests {
    pub fn build(s: &InputSources<'_>) -> Self {
        let (service_env, service_env_names) = service_env_digest(s.envs);
        Self {
            corpus: s.corpus.to_string(),
            questions: s.questions.to_string(),
            variants: optional_file_digest(s.variants_file),
            cassette: optional_file_digest(s.cassette_file),
            embedder_model: embedder_digest(s.embedders),
            extractor: extractor_digest(s.extractor_identity),
            service_binary: service_binary_digest(s.service_bin, s.path_var),
            service_env,
            service_env_names,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emb(model: Option<&str>) -> EmbedderIdentity {
        EmbedderIdentity {
            model: model.map(str::to_string),
            dim: Some(384),
            profile: Some("none".into()),
        }
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const HEX64: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn file(&self, name: &str, body: &str) -> PathBuf {
            let p = self.dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            p
        }
    }

    fn build(
        cassette: Option<&Path>,
        envs: &[BTreeMap<String, String>],
        embedders: &[EmbedderIdentity],
    ) -> InputDigests {
        InputDigests::build(&InputSources {
            corpus: HEX64,
            questions: HEX64,
            variants_file: None,
            cassette_file: cassette,
            embedders,
            extractor_identity: Some("cassette-replay"),
            service_bin: Some((Path::new("/definitely/not/there"), "liminis 1.0")),
            path_var: None,
            envs,
        })
    }

    #[test]
    fn no_cassette_and_no_variants_record_none() {
        let d = build(None, &[env(&[])], &[emb(Some("m"))]);
        assert_eq!(d.cassette, "none");
        assert_eq!(d.variants, "none");
    }

    #[test]
    fn a_service_that_reports_no_model_is_unknown() {
        assert_eq!(embedder_digest(&[emb(None)]), "unknown");
        assert_eq!(embedder_digest(&[emb(Some("m")), emb(None)]), "unknown");
        assert_eq!(embedder_digest(&[]), "none", "no service, no embedder");
        assert_eq!(embedder_digest(&[emb(Some("m"))]).len(), 64);
        assert_ne!(
            embedder_digest(&[emb(Some("a"))]),
            embedder_digest(&[emb(Some("b"))])
        );
    }

    #[test]
    fn changing_only_the_cassette_changes_exactly_the_cassette_digest() {
        let fx = Fixture {
            dir: tempfile::tempdir().unwrap(),
        };
        let c1 = fx.file("a.json", "one");
        let c2 = fx.file("b.json", "two");
        let e = [env(&[
            ("LCG_RERANK_TOP_N", "40"),
            ("LCG_REPLAY_LLM", "/a.json"),
        ])];
        let e2 = [env(&[
            ("LCG_RERANK_TOP_N", "40"),
            ("LCG_REPLAY_LLM", "/b.json"),
        ])];
        let a = build(Some(&c1), &e, &[emb(Some("m"))]);
        let b = build(Some(&c2), &e2, &[emb(Some("m"))]);
        let diff: Vec<&str> = KEYS.into_iter().filter(|k| a.get(k) != b.get(k)).collect();
        assert_eq!(diff, vec!["cassette"]);
        assert_eq!(a.cassette.len(), 64);
        // The same contents at a different path digest identically.
        let c3 = fx.file("c.json", "one");
        assert_eq!(build(Some(&c3), &e, &[emb(Some("m"))]).cassette, a.cassette);
    }

    #[test]
    fn extractor_identity_carries_no_cassette_path_and_no_credentials() {
        assert_eq!(
            extractor_identity(true, Some("http://x")),
            "cassette-replay"
        );
        assert_eq!(extractor_identity(false, None), "service-default");
        let id = extractor_identity(false, Some("https://user:pw@host/v1?key=s3cret"));
        assert!(id.starts_with("endpoint:"));
        assert!(!id.contains("s3cret") && !id.contains("pw"), "{id}");
    }

    #[test]
    fn secret_env_values_never_reach_the_digest_and_changing_one_changes_nothing() {
        let a = env(&[
            ("LCG_EMBEDDING_API_KEY", "sk-aaa"),
            ("LCG_EMBEDDING_URL", "http://a"),
            ("LCG_X_TOKEN", "t1"),
            ("LCG_RERANK_TOP_N", "40"),
            ("HOME", "/h"),
        ]);
        let mut b = a.clone();
        b.insert("LCG_EMBEDDING_API_KEY".into(), "sk-bbb".into());
        b.insert("LCG_EMBEDDING_URL".into(), "http://b".into());
        b.insert("HOME".into(), "/other".into());
        let (da, names) = service_env_digest(std::slice::from_ref(&a));
        assert_eq!(da, service_env_digest(&[b]).0);
        // A tuning value does change it.
        let mut c = a;
        c.insert("LCG_RERANK_TOP_N".into(), "80".into());
        assert_ne!(da, service_env_digest(&[c]).0);
        // Names (not values) are listed; non-LCG names are not.
        assert!(names.contains(&"LCG_EMBEDDING_API_KEY".to_string()));
        assert!(!names.contains(&"HOME".to_string()));
        assert!(!names.iter().any(|n| n.contains("sk-")));
    }

    #[test]
    fn legacy_graphiti_variables_are_digested_and_their_secrets_redacted() {
        let a = env(&[
            ("GRAPHITI_EXTRACTION_LLM", "model-a"),
            ("GRAPHITI_EMBEDDING_API_KEY", "sk-aaa"),
        ]);
        let (da, names) = service_env_digest(std::slice::from_ref(&a));
        // A behaviour-changing legacy variable changes the digest.
        let mut b = a.clone();
        b.insert("GRAPHITI_EXTRACTION_LLM".into(), "model-b".into());
        assert_ne!(da, service_env_digest(&[b]).0);
        // A legacy credential is recorded by name only.
        let mut c = a;
        c.insert("GRAPHITI_EMBEDDING_API_KEY".into(), "sk-bbb".into());
        assert_eq!(da, service_env_digest(&[c]).0);
        assert!(names.contains(&"GRAPHITI_EXTRACTION_LLM".to_string()));
        assert!(names.contains(&"GRAPHITI_EMBEDDING_API_KEY".to_string()));
    }

    #[test]
    fn multi_profile_digests_are_independent_of_profile_order() {
        let p1 = emb(Some("m"));
        let mut p2 = emb(Some("m"));
        p2.profile = Some("embedder".into());
        assert_eq!(
            embedder_digest(&[p1.clone(), p2.clone()]),
            embedder_digest(&[p2.clone(), p1.clone()])
        );
        let (e1, e2) = (
            env(&[("LCG_EMBEDDING_PROFILE", "none")]),
            env(&[("LCG_EMBEDDING_PROFILE", "embedder")]),
        );
        assert_eq!(
            service_env_digest(&[e1.clone(), e2.clone()]).0,
            service_env_digest(&[e2, e1]).0
        );
        assert_eq!(service_env_digest(&[]).0, "none");
    }

    #[test]
    fn service_binary_hashes_the_file_then_falls_back_to_the_version_string() {
        let fx = Fixture {
            dir: tempfile::tempdir().unwrap(),
        };
        let bin = fx.file("svc", "binary bytes");
        let d = service_binary_digest(Some((&bin, "v1")), None);
        assert_eq!(d, file_digest(&bin).unwrap());
        // A bare name is resolved against PATH.
        let found = service_binary_digest(
            Some((Path::new("svc"), "v1")),
            Some(fx.dir.path().as_os_str()),
        );
        assert_eq!(found, d);
        // Unresolvable: version string, then unknown, then none with no service.
        let gone = Path::new("/definitely/not/there");
        assert_eq!(
            service_binary_digest(Some((gone, "v1")), None),
            sha_of("service-version", &["v1"])
        );
        assert_eq!(
            service_binary_digest(Some((gone, "unknown")), None),
            "unknown"
        );
        assert_eq!(service_binary_digest(None, None), "none");
    }

    #[test]
    fn json_carries_full_digests_and_display_shortens_them() {
        let d = build(None, &[env(&[])], &[emb(Some("m"))]);
        let j = d.to_json();
        assert_eq!(j["schema_version"], 1);
        assert_eq!(j["corpus"].as_str().unwrap().len(), 64);
        assert_eq!(j["cassette"], "none");
        assert_eq!(short(HEX64).len(), 16);
        assert_eq!(short("none"), "none");
        assert_eq!(short("unknown"), "unknown");
    }
}
