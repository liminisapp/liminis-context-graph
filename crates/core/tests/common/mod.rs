//! Helpers shared by integration tests (each test file is its own crate; `mod common;` pulls
//! this in). Not every file uses every helper.
#![allow(dead_code)]

/// The method names `handlers::handle()` dispatches, read from its `match req.method.as_str()`
/// arms (the arms are the only 8-space-indented `"name" =>` lines between the match and its
/// `Method not found` fallback). Depends on `health_check` staying the first arm and on
/// rustfmt's layout — it fails loudly, not silently, if either moves.
pub fn dispatched_methods() -> std::collections::BTreeSet<String> {
    let src = include_str!("../../src/handlers.rs");
    let start = src
        .find("    match req.method.as_str() {\n        \"health_check\"")
        .expect("handle() dispatch match not found — update this helper if it moved");
    let end = start
        + src[start..]
            .find("Method not found")
            .expect("handle() fallback arm not found");
    src[start..end]
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("        \"")?;
            let (name, tail) = rest.split_once('"')?;
            tail.trim_start()
                .starts_with("=>")
                .then(|| name.to_string())
        })
        .collect()
}
