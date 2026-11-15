//! Issue #755 / ADR-0755, SC-004: `knowledge_lint` must scale near-linearly with group size — a
//! full pass at 10× the episodes should cost no more than about 12× the time. One `[[bench]]`
//! target (ADR-0316), run on demand through `bench.yml`; the ratio is read off the Criterion output
//! (and recorded in the PR), deliberately not asserted here: a timing gate in a shared-runner run
//! would be flaky.
//!
//! The fixture is `n` episodes, each primary with its own `doc_id`, producing one fact between two
//! entities and mentioning both. A tenth of the episodes are derived pages restating the previous
//! episode's fact, so stale-page, weak-support, orphan and near-duplicate analysis all do real work.

use criterion::{criterion_group, criterion_main, Criterion};
use lcg_core::lint::{run_lint, LintRequest};
use serde_json::json;

#[path = "common/lint_fixture.rs"]
mod lint_fixture;
use lint_fixture::{seed, GROUP};

fn bench_lint(c: &mut Criterion) {
    let mut group = c.benchmark_group("lint_full_pass");
    group.sample_size(10);
    for (label, n) in [("1k", 1_000usize), ("10k", 10_000), ("50k", 50_000)] {
        let (db, _dir) = seed(n);
        let req = LintRequest::parse(
            &json!({"limit": 500, "single_valued_relations": ["related_to"]}),
            Some(vec![GROUP.to_string()]),
        )
        .unwrap();
        let now = chrono::Utc::now().timestamp_micros();
        group.bench_function(format!("lint_{label}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                run_lint(&conn, &req, now).unwrap()
            })
        });
    }
    group.finish();
}

criterion_group!(lint, bench_lint);
criterion_main!(lint);
