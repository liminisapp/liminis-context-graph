//! Issue #737: `Db::open_with` applies an explicit lbug tuning. Uses `open_with` (not env vars)
//! so it cannot race with other tests in the parallel suite that call `Db::open`.

use lcg_core::lbug_config::LbugTuning;
use lcg_core::Db;

#[test]
fn open_with_explicit_pool_and_threads_opens_and_serves_schema() {
    let dir = tempfile::TempDir::new().unwrap();
    let tuning = LbugTuning::from_vars(Some("256MiB"), Some("2")).unwrap();
    let db = Db::open_with(dir.path().join("t.db").to_str().unwrap(), &tuning).unwrap();
    let conn = db.connect().unwrap();
    conn.init_schema(8).unwrap();
}

#[test]
fn open_with_default_tuning_matches_plain_open() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Db::open_with(
        dir.path().join("t.db").to_str().unwrap(),
        &LbugTuning::default(),
    )
    .unwrap();
    db.connect().unwrap().init_schema(8).unwrap();
}
