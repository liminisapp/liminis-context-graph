//! Literal-value heuristics for the ingest-time misattachment diagnostic (issue #823, FR-007,
//! ADR-0823).
//!
//! A *suspected misattachment* is an edge between two different entities whose fact text states a
//! literal value (an amount, date, price or figure) but does not name both endpoints — the shape
//! of "Marlin Core has a monthly budget of $4,200" stored as `Marlin Core HAS_MEASURE Tomas
//! Kowalski`. The check is deliberately text-only and needs no skeleton, so it has false positives
//! and negatives; that is why ingest *marks* such edges (`attributes.suspected_misattachment`)
//! and never drops them, in the spirit of #816's `signature_mismatch`.
//!
//! These helpers are reimplemented here rather than shared with `lcg-eval`'s graph-integrity
//! matcher: core cannot depend on eval, and moving eval's matcher would risk shifting ADR-0819's
//! metric definitions.

use crate::prompts::normalize_name;

/// Currency symbols that mark a fact as stating a literal value even without a digit nearby.
const CURRENCY_SYMBOLS: [char; 4] = ['$', '€', '£', '¥'];

/// Whether `fact` states a literal value: it contains an ASCII digit or a currency symbol.
///
/// Month names alone ("in March") are deliberately not treated as values — "March" or "May" can
/// be a product or a person, and the dates this targets carry a year.
pub fn states_literal_value(fact: &str) -> bool {
    fact.chars()
        .any(|c| c.is_ascii_digit() || CURRENCY_SYMBOLS.contains(&c))
}

/// Whether `fact` names `name`: a case-insensitive match of the normalized name, bounded on both
/// sides by a non-alphanumeric character or the end of the text ("Ines" does not match "Inesa";
/// "Marlin Core's" does match "Marlin Core"). An empty name is never named.
pub fn fact_names(fact: &str, name: &str) -> bool {
    let needle = normalize_name(name);
    if needle.is_empty() {
        return false;
    }
    let hay = fact.to_lowercase();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(&needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = hay[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = hay[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        // Advance by one character (not one byte) so a multi-byte char is never split.
        from = start + hay[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Whether an edge `src_name → dst_name` carrying `fact` is a suspected misattachment: the two
/// endpoints are different entities, the fact states a literal value, and the fact does not name
/// both endpoints. A self-loop (the sanctioned literal-object form under an ontology) is never
/// suspected.
pub fn is_suspected_misattachment(fact: &str, src_name: &str, dst_name: &str) -> bool {
    if normalize_name(src_name) == normalize_name(dst_name) {
        return false;
    }
    states_literal_value(fact) && !(fact_names(fact, src_name) && fact_names(fact, dst_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_values_are_detected() {
        assert!(states_literal_value(
            "Marlin Core has a monthly budget of $4,200"
        ));
        assert!(states_literal_value("Orbit Supply charges 43 per unit"));
        assert!(states_literal_value("Launch is scheduled for March 2027"));
        assert!(states_literal_value("Uptime was 99.9%"));
        assert!(states_literal_value("Costs are quoted in €"));
        assert!(!states_literal_value("Alice works at Acme"));
        assert!(!states_literal_value("Launch is in March"));
    }

    #[test]
    fn fact_names_is_word_bounded_and_case_insensitive() {
        let fact = "Ines Lindqvist negotiated Orbit Supply's $43 unit price";
        assert!(fact_names(fact, "Ines Lindqvist"));
        assert!(fact_names(fact, "orbit supply"));
        assert!(fact_names(fact, "  ORBIT SUPPLY "));
        assert!(!fact_names("Inesa signed the deal", "Ines"));
        assert!(!fact_names("The Supplyline costs $4", "Supply"));
        assert!(
            fact_names("Inesa and Ines signed", "Ines"),
            "later bounded hit"
        );
        assert!(fact_names("Ines", "Ines"), "whole text");
        assert!(!fact_names("anything", ""));
        assert!(fact_names("Zoë's budget is €5", "zoë"), "non-ASCII name");
    }

    #[test]
    fn suspected_misattachment_rules() {
        let pinned = "Marlin Core has a monthly budget of $4,200";
        assert!(is_suspected_misattachment(
            pinned,
            "Marlin Core",
            "Tomas Kowalski"
        ));
        // Both endpoints named → a legitimate relation.
        assert!(!is_suspected_misattachment(
            "Tomas Kowalski set Marlin Core's monthly budget at $4,200",
            "Tomas Kowalski",
            "Marlin Core"
        ));
        // No literal value → out of scope for this heuristic.
        assert!(!is_suspected_misattachment(
            "Marlin Core is run by a small team",
            "Marlin Core",
            "Tomas Kowalski"
        ));
        // Self-loop is the sanctioned literal-object form.
        assert!(!is_suspected_misattachment(
            pinned,
            "Marlin Core",
            "marlin core"
        ));
        // Neither endpoint named, value present → suspected.
        assert!(is_suspected_misattachment(
            "The budget is $4,200",
            "Marlin Core",
            "Tomas Kowalski"
        ));
    }
}
