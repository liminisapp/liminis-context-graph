//! Durable per-group record of rows stored with a fallback zero vector because the embedder
//! failed during WAL replay recompute (issue #808, ADR-0808).
//!
//! The record is an lcg-only node table, `EmbeddingZeroRow(id, wal_group, kind, uuid)`, with
//! `id = "{group_id}|{kind}|{uuid}"`. It lives in the database, not in a sidecar, because replay
//! writes it **inside `flush_batch`'s own transaction** (see [`apply_zero_effects`]): an entry
//! commits or rolls back with the zero-vector row it describes, so a rollback, cancel or crash can
//! neither leave a phantom entry nor lose one. A database wipe or full rebuild resets it with the
//! data it describes.
//!
//! Rows with legitimately no source text, and rows in the embedding-skip set (#526), are never
//! recorded — only a genuine embedder failure is.
//!
//! The query/maintenance methods on [`Conn`] are the contract for the follow-up repair tool
//! (`knowledge_reembed_missing`): [`Conn::list_zero_vector_rows`] selects the rows to repair and
//! [`Conn::remove_zero_vector_rows`] drops them once repaired, which decrements the status count.
//! None of these writes are recorded into the WAL — the record is derived state.

use std::collections::HashMap;

use crate::db::Conn;
use crate::error::Error;

/// Which embedded column of a row holds the zero vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ZeroKind {
    Name,
    Fact,
    Summary,
    Content,
}

impl ZeroKind {
    pub const ALL: [ZeroKind; 4] = [
        ZeroKind::Name,
        ZeroKind::Fact,
        ZeroKind::Summary,
        ZeroKind::Content,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ZeroKind::Name => "name",
            ZeroKind::Fact => "fact",
            ZeroKind::Summary => "summary",
            ZeroKind::Content => "content",
        }
    }

    pub fn parse(s: &str) -> Option<ZeroKind> {
        ZeroKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The kind for a replay vector param key (`name_embedding` → `Name`, ...).
    pub fn from_vec_key(vec_key: &str) -> Option<ZeroKind> {
        match vec_key {
            "name_embedding" => Some(ZeroKind::Name),
            "fact_embedding" => Some(ZeroKind::Fact),
            "summary_embedding" => Some(ZeroKind::Summary),
            "content_embedding" => Some(ZeroKind::Content),
            _ => None,
        }
    }
}

/// Per-kind counts of recorded zero-vector rows for one group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZeroVectorCounts {
    pub name: u64,
    pub fact: u64,
    pub summary: u64,
    pub content: u64,
}

impl ZeroVectorCounts {
    pub fn total(&self) -> u64 {
        self.name + self.fact + self.summary + self.content
    }

    pub fn get(&self, kind: ZeroKind) -> u64 {
        match kind {
            ZeroKind::Name => self.name,
            ZeroKind::Fact => self.fact,
            ZeroKind::Summary => self.summary,
            ZeroKind::Content => self.content,
        }
    }

    fn add(&mut self, kind: ZeroKind, n: u64) {
        match kind {
            ZeroKind::Name => self.name += n,
            ZeroKind::Fact => self.fact += n,
            ZeroKind::Summary => self.summary += n,
            ZeroKind::Content => self.content += n,
        }
    }

    /// The `knowledge_status` `wal_groups.<gid>.zero_vector_rows` JSON shape. Always carries every
    /// kind, so a clean group reads `{"total":0,"by_kind":{"name":0,...}}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total(),
            "by_kind": {
                "name": self.name,
                "fact": self.fact,
                "summary": self.summary,
                "content": self.content,
            }
        })
    }
}

/// A pending change to the record, produced while resolving a replay window and applied in the
/// same transaction as the rows it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZeroEffect {
    /// A zero-vector fallback was stored for `(kind, uuid)` because the embedder failed.
    Add { kind: ZeroKind, uuid: String },
    /// `(kind, uuid)` now holds a real embedding (or the row is gone): drop any stale entry.
    Clear { kind: ZeroKind, uuid: String },
}

pub(crate) fn row_id(group_id: &str, kind: ZeroKind, uuid: &str) -> String {
    format!("{group_id}|{}|{uuid}", kind.as_str())
}

fn value_as_count(v: &lbug::Value) -> u64 {
    match v {
        lbug::Value::Int64(n) => (*n).max(0) as u64,
        lbug::Value::UInt64(n) => *n,
        lbug::Value::Int32(n) => (*n).max(0) as u64,
        _ => 0,
    }
}

const ADD_Q: &str = "MERGE (z:EmbeddingZeroRow {id: $id}) \
                     SET z.wal_group = $group_id, z.kind = $kind, z.uuid = $uuid";
const DEL_Q: &str = "MATCH (z:EmbeddingZeroRow {id: $id}) DELETE z";

/// Folds `effects` (in order) into their net result per `(kind, uuid)` and applies it through
/// `conn`, **unrecorded** (never reaches the WAL). Intended to run inside `flush_batch`'s open
/// transaction, after the batch's rows executed and before `COMMIT`. Idempotent (`MERGE`/`DELETE`
/// by primary key), so a retry never duplicates an entry.
pub(crate) fn apply_zero_effects(
    conn: &Conn<'_>,
    group_id: &str,
    effects: &[ZeroEffect],
) -> Result<(), Error> {
    if effects.is_empty() {
        return Ok(());
    }
    // Last write wins per id; BTreeMap keeps application order deterministic.
    let mut net: std::collections::BTreeMap<String, (bool, ZeroKind, &str)> =
        std::collections::BTreeMap::new();
    for e in effects {
        match e {
            ZeroEffect::Add { kind, uuid } => {
                net.insert(row_id(group_id, *kind, uuid), (true, *kind, uuid));
            }
            ZeroEffect::Clear { kind, uuid } => {
                net.insert(row_id(group_id, *kind, uuid), (false, *kind, uuid));
            }
        }
    }
    let mut add_stmt = None;
    let mut del_stmt = None;
    for (id, (is_add, kind, uuid)) in net {
        if is_add {
            if add_stmt.is_none() {
                add_stmt = Some(conn.prepare(ADD_Q)?);
            }
            let params = serde_json::json!({
                "id": id, "group_id": group_id, "kind": kind.as_str(), "uuid": uuid,
            });
            conn.execute_prepared(add_stmt.as_mut().expect("prepared above"), &params)?;
        } else {
            if del_stmt.is_none() {
                del_stmt = Some(conn.prepare(DEL_Q)?);
            }
            let params = serde_json::json!({ "id": id });
            conn.execute_prepared(del_stmt.as_mut().expect("prepared above"), &params)?;
        }
    }
    Ok(())
}

impl Conn<'_> {
    /// Per-group, per-kind counts of recorded zero-vector rows, for every group that has any
    /// (a group absent from the map has none). One grouped scan, counts only — never a list — so
    /// it is cheap enough for `knowledge_status`. A database without the table (an old file that
    /// has not run the idempotent schema step yet) reads as empty.
    pub fn zero_vector_counts_all(&self) -> Result<HashMap<String, ZeroVectorCounts>, Error> {
        let rows = match self.query_params(
            "MATCH (z:EmbeddingZeroRow) RETURN z.wal_group, z.kind, count(*)",
            serde_json::json!({}),
        ) {
            Ok(rows) => rows,
            Err(e) if is_missing_table(&e) => return Ok(HashMap::new()),
            Err(e) => return Err(e),
        };
        let mut out: HashMap<String, ZeroVectorCounts> = HashMap::new();
        for row in rows {
            let (Some(group), Some(kind)) = (
                row.first().map(crate::db::value_as_string),
                row.get(1)
                    .map(crate::db::value_as_string)
                    .and_then(|k| ZeroKind::parse(&k)),
            ) else {
                continue;
            };
            let n = row.get(2).map(value_as_count).unwrap_or(0);
            out.entry(group).or_default().add(kind, n);
        }
        Ok(out)
    }

    /// Counts for one group (zeros when it has no record).
    pub fn zero_vector_counts(&self, group_id: &str) -> Result<ZeroVectorCounts, Error> {
        Ok(self
            .zero_vector_counts_all()?
            .remove(group_id)
            .unwrap_or_default())
    }

    /// Every recorded `(kind, uuid)` for `group_id`, optionally restricted to one kind, in a
    /// stable order. Repair tooling reads this to select rows to re-embed.
    pub fn list_zero_vector_rows(
        &self,
        group_id: &str,
        kind: Option<ZeroKind>,
    ) -> Result<Vec<(ZeroKind, String)>, Error> {
        let rows = match self.query_params(
            "MATCH (z:EmbeddingZeroRow {wal_group: $group_id}) RETURN z.kind, z.uuid",
            serde_json::json!({ "group_id": group_id }),
        ) {
            Ok(rows) => rows,
            Err(e) if is_missing_table(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out: Vec<(ZeroKind, String)> = rows
            .iter()
            .filter_map(|row| {
                let kind = ZeroKind::parse(&crate::db::value_as_string(row.first()?))?;
                let uuid = crate::db::value_as_string(row.get(1)?);
                Some((kind, uuid))
            })
            .filter(|(k, _)| kind.is_none_or(|want| want == *k))
            .collect();
        out.sort();
        Ok(out)
    }

    /// Removes the given entries; an entry that is not recorded is a no-op, not an error.
    /// Returns how many were actually removed. Does not re-embed anything — repair is the
    /// caller's job — and is not recorded into the WAL.
    pub fn remove_zero_vector_rows(
        &self,
        group_id: &str,
        entries: &[(ZeroKind, String)],
    ) -> Result<usize, Error> {
        let mut removed = 0;
        for (kind, uuid) in entries {
            let id = row_id(group_id, *kind, uuid);
            let existing = self.query_params(
                "MATCH (z:EmbeddingZeroRow {id: $id}) RETURN count(*)",
                serde_json::json!({ "id": id }),
            )?;
            let present = existing
                .first()
                .and_then(|r| r.first())
                .map(value_as_count)
                .unwrap_or(0)
                > 0;
            if present {
                self.exec_params_unrecorded(DEL_Q, serde_json::json!({ "id": id }))?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Whether the record table exists (probed before a transaction, where a failed statement
    /// would abort the whole transaction).
    pub(crate) fn has_zero_vector_table(&self) -> bool {
        self.query_params(
            "MATCH (z:EmbeddingZeroRow) RETURN z.id LIMIT 0",
            serde_json::json!({}),
        )
        .is_ok()
    }

    /// Drops every entry for `group_id` (a full rebuild/purge resets the record). Unrecorded.
    pub fn clear_zero_vector_rows(&self, group_id: &str) -> Result<(), Error> {
        match self.exec_params_unrecorded(
            "MATCH (z:EmbeddingZeroRow {wal_group: $group_id}) DELETE z",
            serde_json::json!({ "group_id": group_id }),
        ) {
            Ok(()) => Ok(()),
            Err(e) if is_missing_table(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn is_missing_table(e: &Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("table") && (s.contains("does not exist") || s.contains("not exist"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn open_db(dir: &std::path::Path) -> Db {
        let path = dir.join("test.db").to_str().unwrap().to_string();
        let db = Db::open(&path).expect("open db");
        db.connect().unwrap().init_schema(4).unwrap();
        db
    }

    fn add(kind: ZeroKind, uuid: &str) -> ZeroEffect {
        ZeroEffect::Add {
            kind,
            uuid: uuid.to_string(),
        }
    }
    fn clear(kind: ZeroKind, uuid: &str) -> ZeroEffect {
        ZeroEffect::Clear {
            kind,
            uuid: uuid.to_string(),
        }
    }

    #[test]
    fn kind_round_trips_and_maps_vec_keys() {
        for k in ZeroKind::ALL {
            assert_eq!(ZeroKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(
            ZeroKind::from_vec_key("fact_embedding"),
            Some(ZeroKind::Fact)
        );
        assert_eq!(ZeroKind::from_vec_key("nope"), None);
        assert_eq!(ZeroKind::parse("nope"), None);
    }

    #[test]
    fn clean_group_reads_all_zero_counts() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let conn = db.connect().unwrap();
        assert!(conn.zero_vector_counts_all().unwrap().is_empty());
        let c = conn.zero_vector_counts("g1").unwrap();
        assert_eq!(c.total(), 0);
        let j = c.to_json();
        assert_eq!(j["total"], 0);
        for k in ["name", "fact", "summary", "content"] {
            assert_eq!(j["by_kind"][k], 0, "{k}");
        }
    }

    #[test]
    fn add_list_by_kind_remove_and_noop_remove() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let conn = db.connect().unwrap();
        apply_zero_effects(
            &conn,
            "g1",
            &[
                add(ZeroKind::Name, "u1"),
                add(ZeroKind::Fact, "u2"),
                add(ZeroKind::Name, "u3"),
                add(ZeroKind::Name, "u1"), // duplicate — idempotent
            ],
        )
        .unwrap();
        apply_zero_effects(&conn, "g2", &[add(ZeroKind::Content, "u9")]).unwrap();

        let c = conn.zero_vector_counts("g1").unwrap();
        assert_eq!((c.name, c.fact, c.summary, c.content), (2, 1, 0, 0));
        assert_eq!(c.total(), 3);
        assert_eq!(conn.zero_vector_counts("g2").unwrap().content, 1);

        let all = conn.list_zero_vector_rows("g1", None).unwrap();
        assert_eq!(all.len(), 3);
        let names = conn
            .list_zero_vector_rows("g1", Some(ZeroKind::Name))
            .unwrap();
        assert_eq!(
            names,
            vec![
                (ZeroKind::Name, "u1".to_string()),
                (ZeroKind::Name, "u3".to_string())
            ]
        );

        let removed = conn
            .remove_zero_vector_rows("g1", &[(ZeroKind::Name, "u1".to_string())])
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(conn.zero_vector_counts("g1").unwrap().total(), 2);
        // Not recorded → no-op, not an error.
        let removed = conn
            .remove_zero_vector_rows("g1", &[(ZeroKind::Summary, "nope".to_string())])
            .unwrap();
        assert_eq!(removed, 0);
        assert_eq!(conn.zero_vector_counts("g1").unwrap().total(), 2);
        // Other group untouched.
        assert_eq!(conn.zero_vector_counts("g2").unwrap().total(), 1);
    }

    #[test]
    fn net_effect_last_write_wins_per_id() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let conn = db.connect().unwrap();
        apply_zero_effects(
            &conn,
            "g1",
            &[
                add(ZeroKind::Name, "u1"),
                clear(ZeroKind::Name, "u1"),
                clear(ZeroKind::Fact, "u2"),
                add(ZeroKind::Fact, "u2"),
            ],
        )
        .unwrap();
        let rows = conn.list_zero_vector_rows("g1", None).unwrap();
        assert_eq!(rows, vec![(ZeroKind::Fact, "u2".to_string())]);
    }

    #[test]
    fn clear_group_only_clears_that_group() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let conn = db.connect().unwrap();
        apply_zero_effects(&conn, "g1", &[add(ZeroKind::Name, "u1")]).unwrap();
        apply_zero_effects(&conn, "g2", &[add(ZeroKind::Name, "u1")]).unwrap();
        conn.clear_zero_vector_rows("g1").unwrap();
        assert_eq!(conn.zero_vector_counts("g1").unwrap().total(), 0);
        assert_eq!(conn.zero_vector_counts("g2").unwrap().total(), 1);
    }

    #[test]
    fn record_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = open_db(dir.path());
            let conn = db.connect().unwrap();
            apply_zero_effects(
                &conn,
                "g1",
                &[add(ZeroKind::Summary, "u1"), add(ZeroKind::Content, "u2")],
            )
            .unwrap();
        }
        let db = open_db(dir.path());
        let conn = db.connect().unwrap();
        let c = conn.zero_vector_counts("g1").unwrap();
        assert_eq!((c.summary, c.content, c.total()), (1, 1, 2));
    }
}
