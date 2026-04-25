//! Value-layer deep-merge engine for Crab configuration.
//!
//! All configuration sources (defaults, plugin layer, file layers, runtime
//! layer) are merged at the `toml::Value` level before being deserialized
//! into [`crate::config::Config`]. The single recursive function
//! [`merge_toml_values`] defines the semantics for every layer transition,
//! so adding a new field to `Config` requires no merge-logic change.
//!
//! Semantics (aligned with `docs/config.md` §4):
//!
//! | overlay kind | base kind | result |
//! |---|---|---|
//! | Table | Table | recurse: keys in overlay are merged into base |
//! | Array | Array | concatenate then deduplicate (insertion order preserved) |
//! | scalar / table-vs-scalar / array-vs-scalar | any | overlay wins |
//!
//! TOML has no native `Null`; JSON-converted plugin configs that contain
//! `serde_json::Value::Null` are dropped at the JSON→TOML conversion step
//! before reaching this function. As a defense in depth, callers should not
//! pass synthetic `toml::Value` variants containing placeholder nulls — the
//! "skalar later-wins" rule treats every concrete overlay scalar as a real
//! value, so an explicit overlay always replaces the base scalar.

use std::collections::HashSet;

use toml::Value;

/// Recursively merge `overlay` into `base`.
///
/// - Tables deep-merge: keys present only in `overlay` are inserted; keys
///   present in both recurse on the corresponding values.
/// - Arrays concatenate then deduplicate, preserving the first occurrence
///   of each element. Equality is structural (via JSON serialization).
/// - All other combinations (scalar overlay, type mismatch) replace `base`
///   with `overlay` — "later wins".
pub fn merge_toml_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Table(b), Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge_toml_values(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (Value::Array(b), Value::Array(o)) => {
            b.extend(o);
            dedup_preserving_order(b);
        }
        (slot, overlay) => {
            *slot = overlay;
        }
    }
}

/// Remove duplicate elements from `vec` in place, keeping the first
/// occurrence of each value (insertion order preserved).
///
/// Equality is structural: two elements are considered equal iff their
/// JSON serialization is identical. This makes the comparison work for
/// scalars, nested tables, and table-typed array elements alike.
pub fn dedup_preserving_order(vec: &mut Vec<Value>) {
    let mut seen: HashSet<String> = HashSet::with_capacity(vec.len());
    vec.retain(|v| {
        let key = serde_json::to_string(v).unwrap_or_default();
        seen.insert(key)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::value::Table;

    fn parse(s: &str) -> Value {
        toml::from_str(s).expect("valid TOML")
    }

    #[test]
    fn scalar_later_wins() {
        let mut base = parse("x = 1");
        merge_toml_values(&mut base, parse("x = 2"));
        assert_eq!(base.get("x").unwrap().as_integer(), Some(2));
    }

    #[test]
    fn empty_overlay_no_op() {
        let mut base = parse("x = 1\ny = 'a'");
        merge_toml_values(&mut base, Value::Table(Table::new()));
        assert_eq!(base.get("x").unwrap().as_integer(), Some(1));
        assert_eq!(base.get("y").unwrap().as_str(), Some("a"));
    }

    #[test]
    fn table_deep_merge_two_levels() {
        let mut base = parse("[a]\nx = 1");
        merge_toml_values(&mut base, parse("[a]\ny = 2"));
        let a = base.get("a").unwrap().as_table().unwrap();
        assert_eq!(a.get("x").unwrap().as_integer(), Some(1));
        assert_eq!(a.get("y").unwrap().as_integer(), Some(2));
    }

    #[test]
    fn array_concat_dedup_scalars() {
        let mut base = parse("xs = [1, 2, 3]");
        merge_toml_values(&mut base, parse("xs = [3, 4, 1]"));
        let xs = base.get("xs").unwrap().as_array().unwrap();
        let ints: Vec<i64> = xs.iter().map(|v| v.as_integer().unwrap()).collect();
        assert_eq!(ints, vec![1, 2, 3, 4]);
    }

    #[test]
    fn type_mismatch_overlay_wins() {
        let mut base = parse("[a]\nx = 1");
        merge_toml_values(&mut base, parse("a = 'replaced'"));
        assert_eq!(base.get("a").unwrap().as_str(), Some("replaced"));
    }

    #[test]
    fn dedup_preserves_first_occurrence_order() {
        let mut arr = vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("a".into()),
            Value::String("c".into()),
            Value::String("b".into()),
        ];
        dedup_preserving_order(&mut arr);
        let strs: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(strs, vec!["a", "b", "c"]);
    }
}
