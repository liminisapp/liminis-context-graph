//! Answer scoring, ported from the E13 probe's `score()` (`answer-eval.py`, FR-022).
//!
//! * `single`: 1 when the reader's first answer matches the first gold answer, else 0.
//! * `set`: F1 of the reader's answers against the gold set (lists and joins).
//! * `ordered`: 1 only for the same number of answers, each matching gold in order (history).
//!
//! Matching is the probe's: both sides are normalised ([`norm`]) and an answer matches when the
//! gold contains it or it contains the gold (the first only for answers of 3+ characters), so case,
//! surrounding punctuation and spacing never count as wrong.

/// Lowercase, collapse every run of characters outside `[a-z0-9%$.]` to one space, then strip
/// spaces and dots from both ends (`re.sub(r"[^a-z0-9%$.]+", " ", s.lower()).strip(" .")`).
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '%' | '$' | '.') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push(' ');
            in_run = true;
        }
    }
    out.trim_matches(|c| c == ' ' || c == '.').to_string()
}

/// `match(g, p)`: `p` non-empty, and gold ⊆ answer, or (answer of 3+ chars) answer ⊆ gold.
pub fn matches(gold: &str, pred: &str) -> bool {
    if pred.is_empty() {
        return false;
    }
    let (g, p) = (norm(gold), norm(pred));
    p.contains(&g) || (p.chars().count() >= 3 && g.contains(&p))
}

/// The score of `preds` against `gold` under `scoring` (`single`, `set` or `ordered`).
pub fn score(scoring: &str, gold: &[String], preds: &[String]) -> f64 {
    match scoring {
        "ordered" => {
            let ok =
                preds.len() == gold.len() && gold.iter().zip(preds).all(|(g, p)| matches(g, p));
            f64::from(u8::from(ok))
        }
        "set" => {
            if gold.is_empty() {
                return 0.0;
            }
            let tp = gold
                .iter()
                .filter(|g| preds.iter().any(|p| matches(g, p)))
                .count() as f64;
            let prec = if preds.is_empty() {
                0.0
            } else {
                preds
                    .iter()
                    .filter(|p| gold.iter().any(|g| matches(g, p)))
                    .count() as f64
                    / preds.len() as f64
            };
            let rec = tp / gold.len() as f64;
            if prec + rec > 0.0 {
                2.0 * prec * rec / (prec + rec)
            } else {
                0.0
            }
        }
        _ => {
            let ok = match (gold.first(), preds.first()) {
                (Some(g), Some(p)) => matches(g, p),
                _ => false,
            };
            f64::from(u8::from(ok))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn norm_matches_the_reference_regex() {
        assert_eq!(norm("  Noor Okafor. "), "noor okafor");
        assert_eq!(norm("$1,200/month"), "$1 200 month");
        assert_eq!(norm("99.9%"), "99.9%");
        assert_eq!(norm("...Lisbon..."), "lisbon");
        assert_eq!(norm("Zoë"), "zo");
        assert_eq!(norm(""), "");
    }

    #[test]
    fn matching_ignores_case_punctuation_and_whitespace() {
        assert!(matches("Noor Okafor", "noor  okafor!"));
        assert!(matches("Noor Okafor", "Noor Okafor (account manager)"));
        assert!(
            matches("Helix Supply", "Helix"),
            "answer inside gold, 3+ chars"
        );
        assert!(
            !matches("Helix Supply", "He"),
            "too short to count as contained"
        );
        assert!(!matches("Noor Okafor", ""));
        assert!(!matches("Noor Okafor", "Tomas Kowalski"));
    }

    #[test]
    fn single_scores_the_first_answer_only() {
        let g = v(&["Lisbon"]);
        assert_eq!(score("single", &g, &v(&["lisbon"])), 1.0);
        assert_eq!(score("single", &g, &v(&["Porto", "Lisbon"])), 0.0);
        assert_eq!(score("single", &g, &[]), 0.0);
        assert_eq!(score("single", &g, &v(&["unknown"])), 0.0);
    }

    #[test]
    fn set_is_f1_with_known_arithmetic() {
        let g = v(&["A Core", "B Core", "C Core"]);
        assert_eq!(score("set", &g, &v(&["a core", "b core", "c core"])), 1.0);
        // 2 of 3 gold found, 2 of 4 predictions right: P = 0.5, R = 2/3, F1 = 4/7.
        let f1 = score("set", &g, &v(&["A Core", "B Core", "X", "Y"]));
        assert!((f1 - 4.0 / 7.0).abs() < 1e-12, "{f1}");
        assert_eq!(score("set", &g, &[]), 0.0);
        assert_eq!(score("set", &g, &v(&["none"])), 0.0);
    }

    #[test]
    fn ordered_needs_every_value_in_order() {
        let g = v(&["Postgres", "MySQL", "SQLite"]);
        assert_eq!(
            score("ordered", &g, &v(&["postgres", "mysql", "sqlite"])),
            1.0
        );
        assert_eq!(
            score("ordered", &g, &v(&["MySQL", "Postgres", "SQLite"])),
            0.0
        );
        assert_eq!(score("ordered", &g, &v(&["Postgres", "MySQL"])), 0.0);
        assert_eq!(score("ordered", &g, &[]), 0.0);
    }
}
