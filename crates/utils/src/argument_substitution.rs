//! Slash-command argument substitution.
//!
//! Templates like `/review PR $1` or `/open $FILE:$LINE` expand against
//! a list of positional args + a named-arg map. Three forms are supported,
//! matching CCB's `utils/argumentSubstitution.ts` behaviour:
//!
//! - `$N` — positional, 1-indexed. `$1` is the first arg, `$2` the second, etc.
//! - `$NAME` — named. Looked up in the `named_args` map. Uppercase letters, digits,
//!   underscores; must start with a letter or underscore. Reserved shorthands
//!   (`$ARGS`, `$FILE`, `$LINE`, `$PR`) are treated as named args with no special
//!   handling — callers can populate them in `named_args`.
//! - `${NAME:-default}` — named with default when unset or empty.
//!
//! A missing reference expands to empty string by default (consistent with
//! shell). Use [`substitute_strict`] if missing references should be an error.

use std::collections::HashMap;
use std::hash::BuildHasher;

// ─── Public API ────────────────────────────────────────────────────────

/// Substitute positional + named args into a template. Missing
/// references expand to empty string.
#[must_use]
pub fn substitute<S: BuildHasher>(
    template: &str,
    positional: &[&str],
    named: &HashMap<String, String, S>,
) -> String {
    substitute_impl(template, positional, named, false).unwrap_or_default()
}

/// Like [`substitute`], but returns an error listing every unresolved
/// reference rather than silently swallowing them.
///
/// # Errors
///
/// Returns a list of unresolved keys (positional indices like `"$1"` or
/// named like `"$FILE"`) that had no value in the provided args.
pub fn substitute_strict<S: BuildHasher>(
    template: &str,
    positional: &[&str],
    named: &HashMap<String, String, S>,
) -> Result<String, Vec<String>> {
    substitute_impl(template, positional, named, true)
}

// ─── Implementation ────────────────────────────────────────────────────

fn substitute_impl<S: BuildHasher>(
    template: &str,
    positional: &[&str],
    named: &HashMap<String, String, S>,
    strict: bool,
) -> Result<String, Vec<String>> {
    let mut out = String::with_capacity(template.len());
    let mut missing: Vec<String> = Vec::new();
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // After `$`; peek to dispatch.
        match chars.peek().copied() {
            // `$$` → literal `$`
            Some('$') => {
                chars.next();
                out.push('$');
            }
            // `${NAME:-default}`
            Some('{') => {
                chars.next(); // consume `{`
                let (body, closed) = take_until_close(&mut chars);
                if !closed {
                    // No matching `}` — emit the raw `${` back + body so
                    // the caller sees that the template is malformed.
                    out.push_str("${");
                    out.push_str(&body);
                    continue;
                }
                let (name, default) = match body.split_once(":-") {
                    Some((n, d)) => (n.to_string(), Some(d.to_string())),
                    None => (body, None),
                };
                match lookup(&name, positional, named) {
                    Some(val) if !val.is_empty() => out.push_str(val),
                    _ => match default {
                        Some(d) => out.push_str(&d),
                        None => {
                            if strict {
                                missing.push(format!("${{{name}}}"));
                            }
                        }
                    },
                }
            }
            // `$N` (positional) or `$NAME` (named)
            Some(next) if next == '_' || next.is_ascii_alphanumeric() => {
                let mut name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == '_' || ch.is_ascii_alphanumeric() {
                        name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match lookup(&name, positional, named) {
                    Some(val) => out.push_str(val),
                    None => {
                        if strict {
                            missing.push(format!("${name}"));
                        }
                    }
                }
            }
            // Lone `$` at end of string or followed by disallowed char —
            // emit literal and let the next iteration handle the char.
            _ => {
                out.push('$');
            }
        }
    }

    if strict && !missing.is_empty() {
        Err(missing)
    } else {
        Ok(out)
    }
}

/// Consume characters from `chars` until `}`, returning `(body, closed_flag)`.
fn take_until_close<I>(chars: &mut std::iter::Peekable<I>) -> (String, bool)
where
    I: Iterator<Item = char>,
{
    let mut body = String::new();
    for c in chars.by_ref() {
        if c == '}' {
            return (body, true);
        }
        body.push(c);
    }
    (body, false)
}

/// Resolve a name:
/// - All-digit → positional (1-indexed; returns `None` on out-of-range)
/// - Otherwise → named map lookup
fn lookup<'a, S: BuildHasher>(
    name: &str,
    positional: &'a [&'a str],
    named: &'a HashMap<String, String, S>,
) -> Option<&'a str> {
    if name.chars().all(|c| c.is_ascii_digit()) && !name.is_empty() {
        let idx: usize = name.parse().ok()?;
        if idx == 0 {
            return None;
        }
        positional.get(idx - 1).copied()
    } else {
        named.get(name).map(String::as_str)
    }
}

#[cfg(test)]
// `${NAME:-world}` in test literals looks like an unrelated format arg to
// clippy (it's shell/template syntax here).
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::*;

    fn named(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(substitute("hello world", &[], &named(&[])), "hello world");
    }

    #[test]
    fn positional_one_indexed() {
        assert_eq!(substitute("$1", &["first"], &named(&[])), "first");
        assert_eq!(substitute("$1 and $2", &["a", "b"], &named(&[])), "a and b");
    }

    #[test]
    fn positional_out_of_range_is_empty() {
        assert_eq!(substitute("$5", &["a"], &named(&[])), "");
    }

    #[test]
    fn dollar_zero_is_empty() {
        // $0 is not a valid positional (we're 1-indexed).
        assert_eq!(substitute("$0", &["a"], &named(&[])), "");
    }

    #[test]
    fn named_args_resolve() {
        let n = named(&[("FILE", "main.rs"), ("LINE", "42")]);
        assert_eq!(substitute("$FILE:$LINE", &[], &n), "main.rs:42");
    }

    #[test]
    fn default_used_when_missing() {
        assert_eq!(
            substitute("hello ${NAME:-world}", &[], &named(&[])),
            "hello world"
        );
    }

    #[test]
    fn default_used_when_empty_string() {
        let n = named(&[("NAME", "")]);
        assert_eq!(substitute("hello ${NAME:-world}", &[], &n), "hello world");
    }

    #[test]
    fn default_skipped_when_value_present() {
        let n = named(&[("NAME", "Alice")]);
        assert_eq!(substitute("hello ${NAME:-world}", &[], &n), "hello Alice");
    }

    #[test]
    fn escaped_dollar_emits_literal() {
        assert_eq!(substitute("cost: $$100", &[], &named(&[])), "cost: $100");
    }

    #[test]
    fn lone_dollar_at_end_passes_through() {
        assert_eq!(substitute("pay $", &[], &named(&[])), "pay $");
    }

    #[test]
    fn dollar_before_non_alpha_passes_through() {
        assert_eq!(substitute("$!oops", &[], &named(&[])), "$!oops");
    }

    #[test]
    fn unclosed_brace_passes_through() {
        assert_eq!(substitute("${UNCLOSED", &[], &named(&[])), "${UNCLOSED");
    }

    #[test]
    fn mixed_positional_and_named() {
        let n = named(&[("BRANCH", "main")]);
        assert_eq!(
            substitute("review $1 on $BRANCH (context: $2)", &["PR#42", "fix"], &n),
            "review PR#42 on main (context: fix)"
        );
    }

    #[test]
    fn strict_reports_missing_keys() {
        let err = substitute_strict("hi $FILE and $1", &[], &named(&[])).unwrap_err();
        assert_eq!(err.len(), 2);
        assert!(err.iter().any(|s| s == "$FILE"));
        assert!(err.iter().any(|s| s == "$1"));
    }

    #[test]
    fn strict_ok_when_all_resolved() {
        let n = named(&[("A", "x"), ("B", "y")]);
        assert_eq!(substitute_strict("$A$B", &[], &n).unwrap(), "xy");
    }

    #[test]
    fn strict_treats_default_as_resolution() {
        // `${X:-fallback}` with X unset uses the default — not a missing ref.
        assert_eq!(
            substitute_strict("${MISSING:-fallback}", &[], &named(&[])).unwrap(),
            "fallback"
        );
    }

    #[test]
    fn multi_digit_positional() {
        let args: Vec<&str> = (0..12).map(|_| "x").collect::<Vec<_>>();
        let args_with_value: Vec<&str> = args
            .iter()
            .enumerate()
            .map(|(i, _)| {
                if i == 9 {
                    "TENTH"
                } else {
                    *args.get(i).unwrap()
                }
            })
            .collect();
        assert_eq!(substitute("$10", &args_with_value, &named(&[])), "TENTH");
    }
}
