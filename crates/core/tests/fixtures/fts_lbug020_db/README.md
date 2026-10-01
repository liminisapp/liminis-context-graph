# fts_lbug020_db

A notebook written by the **published v0.15.0** `liminis-context-graph` (lbug 0.20.0, storage
version 47) with non-ASCII terms in all three FTS-indexed tables. Used by
`crates/core/tests/fts_lbug020_migration.rs` for issue #649 (the #646 regression: an FTS index
built by lbug 0.20 cannot be maintained or searched by lbug 0.21 for non-ASCII terms).

## Contents of `t.db.tar.gz`

- `t.db` — the database after a **clean shutdown** (SIGTERM, preceded by
  `knowledge_prepare_checkpoint`), so no `t.db.wal` sidecar.
- `wal/liminis/*.jsonl` — the matching WAL (embeddings are stripped from it by design), used to
  exercise `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}`.

Embedding dimension is **768**.

| table | row | non-ASCII term |
|---|---|---|
| `Entity` | `Zürich` (summary "A city in Switzerland"), `東京` (summary "Capital of Japan"), `Alice` (summary "Alice lives in Zürich"), `Bob` | `Zürich`, `東京` |
| `RelatesToNode_` | `LIVES_IN`, fact `Alice → Zürich` | `→`, `Zürich` |
| `RelatesToNode_` | `KNOWS`, fact `Alice knows Bob` | (ASCII) |
| `Episodic` | `ep-nonascii`: `Alice — moved to Zürich, then visited 東京` | `—`, `Zürich`, `東京` |
| `Episodic` | `ep-ascii`: `Bob met Alice at the office` | (ASCII) |

## How it was built

```sh
gh release download v0.15.0 -R verveguy/liminis-context-graph -p 'lcg-service-aarch64-apple-darwin.tar.xz'
tar -xJf lcg-service-aarch64-apple-darwin.tar.xz
python3 fake_embedder.py &                      # deterministic /v1/embeddings on :8765, dim 768
mkdir ws && cd ws
LCG_EMBEDDING_URL=http://127.0.0.1:8765/v1/embeddings ../lcg-service-aarch64-apple-darwin/liminis-context-graph &
python3 ../build.py .lcg/service.sock           # writes the rows above, then prepare_checkpoint
kill -TERM %2                                   # clean shutdown
cp .lcg/db/liminis.db t.db && cp -R .lcg/wal wal && tar -czf t.db.tar.gz t.db wal
```

`build.py` and `fake_embedder.py` are kept here for reproducibility. Under lbug 0.20 `QUERY_FTS_INDEX`
for `→`, `Zürich`, `東京` and `—` returns the rows above; under lbug 0.21 against the *unrebuilt*
index it returns zero rows and deletes fail with `FTS index '<idx>' is inconsistent`.
