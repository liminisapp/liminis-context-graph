// Issue #719 / #930: on Windows, lbug <= 0.21.2 could silently corrupt STRING and INT8/UINT8
// column data at CHECKPOINT. MSVC's AVX2 `std::minmax_element` returned a wrong min/max for some
// byte arrays, so a column chunk that was *nearly* constant was stored as CONSTANT and its
// outliers were lost, with no error (LadybugDB/ladybug#1447; fixed by #1593, first in lbug 0.23.0).
//
// This is the regression check. It writes columns that are constant except for a few outliers
// (the shape that trips CONSTANT detection) across several column chunks, runs CHECKPOINT, reopens
// the database and requires every value to read back exactly. It runs on every platform: only
// Windows was ever affected, so a pass on macOS/Linux proves nothing about the fix -- the
// meaningful run is `windows.yml` and the maintainer's real-machine run. The database lives under a
// path with a space and non-ASCII characters to cover the Windows path changes (#1496, #1534).
//
// Run alone: `cargo test -p lcg-core --test windows_checkpoint_719`.

use lbug::{Connection, Database, SystemConfig, Value};

/// Rows written; > 2 chunks of 2048 so outliers land at chunk starts, ends and middles.
const N: i64 = 6000;

fn i8_at(i: i64) -> i8 {
    match i {
        _ if i % 97 == 0 => -128,
        _ if i % 89 == 0 => 127,
        _ => 0,
    }
}

/// Mostly-zero column with 255 outliers.
fn u8_at(i: i64) -> u8 {
    if i % 101 == 0 {
        255
    } else if i % 53 == 0 {
        1
    } else {
        0
    }
}

/// Mostly-255 column with a few 0 outliers (the inverse shape).
fn u8_hi_at(i: i64) -> u8 {
    if i % 211 == 0 {
        0
    } else {
        255
    }
}

fn s_at(i: i64) -> String {
    if i % 131 == 0 {
        format!("outlier-é-日本-{i}")
    } else if i % 2048 == 0 || i % 2048 == 2047 {
        "edge".to_string() // chunk boundaries
    } else {
        "same".to_string()
    }
}

/// Genuinely constant column: must still read back as the constant.
const CONST_S: &str = "constant-ünï";

fn write(db_path: &std::path::Path) {
    let db = Database::new(db_path, SystemConfig::default()).unwrap();
    let c = Connection::new(&db).unwrap();
    c.query(
        "CREATE NODE TABLE T(id INT64, i8 INT8, u8 UINT8, u8hi UINT8, s STRING, k STRING, \
         PRIMARY KEY(id))",
    )
    .unwrap();
    c.query("BEGIN TRANSACTION").unwrap();
    for i in 0..N {
        c.query(&format!(
            "CREATE (:T {{id: {i}, i8: CAST({} AS INT8), u8: CAST({} AS UINT8), \
             u8hi: CAST({} AS UINT8), s: '{}', k: '{CONST_S}'}})",
            i8_at(i),
            u8_at(i),
            u8_hi_at(i),
            s_at(i)
        ))
        .unwrap();
    }
    c.query("COMMIT").unwrap();
    c.query("CHECKPOINT").unwrap();
}

fn read_back_and_compare(db_path: &std::path::Path) {
    let db = Database::new(db_path, SystemConfig::default()).unwrap();
    let c = Connection::new(&db).unwrap();
    let rows: Vec<Vec<Value>> = c
        .query("MATCH (t:T) RETURN t.id, t.i8, t.u8, t.u8hi, t.s, t.k ORDER BY t.id")
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        N as usize,
        "row count changed across CHECKPOINT"
    );
    for (n, row) in rows.iter().enumerate() {
        let i = n as i64;
        assert!(
            matches!(&row[0], Value::Int64(v) if *v == i),
            "id at {i}: {:?}",
            row[0]
        );
        assert!(
            matches!(&row[1], Value::Int8(v) if *v == i8_at(i)),
            "i8 at {i}: {:?}",
            row[1]
        );
        assert!(
            matches!(&row[2], Value::UInt8(v) if *v == u8_at(i)),
            "u8 at {i}: {:?}",
            row[2]
        );
        assert!(
            matches!(&row[3], Value::UInt8(v) if *v == u8_hi_at(i)),
            "u8hi at {i}: {:?}",
            row[3]
        );
        assert!(
            matches!(&row[4], Value::String(v) if *v == s_at(i)),
            "s at {i}: {:?}",
            row[4]
        );
        assert!(
            matches!(&row[5], Value::String(v) if v == CONST_S),
            "k at {i}: {:?}",
            row[5]
        );
    }
}

#[test]
fn nearly_constant_columns_survive_checkpoint_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    // A space and non-ASCII characters in the workspace path (#1496 / #1534).
    let root = dir.path().join("lcg 719 データ");
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("t.db");

    write(&db_path);
    read_back_and_compare(&db_path);
}

/// Same data, but the checkpoint is the one a clean close performs (no explicit CHECKPOINT
/// statement), then a *second* reopen: guards a value that only goes wrong on the second read.
#[test]
fn nearly_constant_columns_survive_close_checkpoint_and_two_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db");
    {
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let c = Connection::new(&db).unwrap();
        c.query(
            "CREATE NODE TABLE T(id INT64, i8 INT8, u8 UINT8, u8hi UINT8, s STRING, k STRING, \
             PRIMARY KEY(id))",
        )
        .unwrap();
        c.query("BEGIN TRANSACTION").unwrap();
        for i in 0..N {
            c.query(&format!(
                "CREATE (:T {{id: {i}, i8: CAST({} AS INT8), u8: CAST({} AS UINT8), \
                 u8hi: CAST({} AS UINT8), s: '{}', k: '{CONST_S}'}})",
                i8_at(i),
                u8_at(i),
                u8_hi_at(i),
                s_at(i)
            ))
            .unwrap();
        }
        c.query("COMMIT").unwrap();
    }
    read_back_and_compare(&db_path);
    read_back_and_compare(&db_path);
}
