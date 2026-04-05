//! Settings completion hints for editors and CLI autocomplete.
//!
//! Given a partial JSON path or value, returns possible completions
//! with labels, descriptions, and insert text.

use serde_json::Value;

/// A single completion suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// Display label (e.g. `"apiProvider"`).
    pub label: String,
    /// Short description of this item.
    pub detail: String,
    /// Text to insert (may differ from label, e.g. include quotes or default).
    pub insert_text: String,
}

/// Completer that provides key and value suggestions for settings.
pub struct SettingsCompleter {
    schema: Value,
}

impl Default for SettingsCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsCompleter {
    /// Create a completer using the built-in settings schema.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: crate::schema_gen::generate_settings_schema(),
        }
    }

    /// Suggest keys that match a partial path prefix.
    ///
    /// For example, `complete_key("api")` returns `apiProvider`, `apiBaseUrl`,
    /// `apiKey`.
    #[must_use]
    pub fn complete_key(&self, partial: &str) -> Vec<CompletionItem> {
        let Some(props) = self.schema.get("properties").and_then(Value::as_object) else {
            return Vec::new();
        };

        let lower = partial.to_lowercase();
        props
            .iter()
            .filter(|(key, _)| key.to_lowercase().starts_with(&lower))
            .map(|(key, prop)| {
                let description = prop
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let type_str = prop.get("type").and_then(Value::as_str).unwrap_or("any");
                CompletionItem {
                    label: key.clone(),
                    detail: format!("({type_str}) {description}"),
                    insert_text: key.clone(),
                }
            })
            .collect()
    }

    /// Suggest values for a given settings key, optionally filtered by a
    /// partial value prefix.
    ///
    /// Returns enum values, defaults, and examples from the schema.
    #[must_use]
    pub fn complete_value(&self, key: &str, partial: &str) -> Vec<CompletionItem> {
        let Some(prop) = self.schema.get("properties").and_then(|p| p.get(key)) else {
            return Vec::new();
        };

        let lower = partial.to_lowercase();
        let mut items = Vec::new();

        // Enum values
        if let Some(enum_vals) = prop.get("enum").and_then(Value::as_array) {
            for val in enum_vals {
                if let Some(s) = val.as_str()
                    && s.to_lowercase().starts_with(&lower)
                {
                    items.push(CompletionItem {
                        label: s.to_string(),
                        detail: "enum value".to_string(),
                        insert_text: s.to_string(),
                    });
                }
            }
        }

        // Default value
        if let Some(default) = prop.get("default") {
            let default_str = format_value(default);
            if default_str.to_lowercase().starts_with(&lower)
                && !items.iter().any(|i| i.label == default_str)
            {
                items.push(CompletionItem {
                    label: default_str.clone(),
                    detail: "default value".to_string(),
                    insert_text: default_str,
                });
            }
        }

        // Examples
        if let Some(examples) = prop.get("examples").and_then(Value::as_array) {
            for ex in examples {
                let ex_str = format_value(ex);
                if ex_str.to_lowercase().starts_with(&lower)
                    && !items.iter().any(|i| i.label == ex_str)
                {
                    items.push(CompletionItem {
                        label: ex_str.clone(),
                        detail: "example".to_string(),
                        insert_text: ex_str,
                    });
                }
            }
        }

        items
    }
}

/// Format a JSON value as a display string (without outer quotes for strings).
fn format_value(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completer() -> SettingsCompleter {
        SettingsCompleter::new()
    }

    // ── Key completion ──────────────────────────────────────────────────

    #[test]
    fn complete_key_empty_returns_all() {
        let items = completer().complete_key("");
        assert!(items.len() >= 10); // at least all known fields
    }

    #[test]
    fn complete_key_api_prefix() {
        let items = completer().complete_key("api");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"apiProvider"));
        assert!(labels.contains(&"apiBaseUrl"));
        assert!(labels.contains(&"apiKey"));
        assert!(!labels.contains(&"model"));
    }

    #[test]
    fn complete_key_case_insensitive() {
        let items = completer().complete_key("API");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"apiProvider"));
    }

    #[test]
    fn complete_key_exact_match() {
        let items = completer().complete_key("model");
        assert!(items.iter().any(|i| i.label == "model"));
    }

    #[test]
    fn complete_key_no_match() {
        let items = completer().complete_key("zzzzz");
        assert!(items.is_empty());
    }

    #[test]
    fn complete_key_has_detail() {
        let items = completer().complete_key("maxTokens");
        assert!(!items.is_empty());
        assert!(!items[0].detail.is_empty());
    }

    // ── Value completion ────────────────────────────────────────────────

    #[test]
    fn complete_value_enum_field() {
        let items = completer().complete_value("apiProvider", "");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"anthropic"));
        assert!(labels.contains(&"openai"));
    }

    #[test]
    fn complete_value_enum_filtered() {
        let items = completer().complete_value("apiProvider", "deep");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "deepseek");
    }

    #[test]
    fn complete_value_permission_mode() {
        let items = completer().complete_value("permissionMode", "");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"trustProject"));
        assert!(labels.contains(&"dangerously"));
    }

    #[test]
    fn complete_value_with_default() {
        let items = completer().complete_value("maxTokens", "");
        // Should include the default "4096"
        assert!(items.iter().any(|i| i.label == "4096"));
    }

    #[test]
    fn complete_value_with_examples() {
        let items = completer().complete_value("model", "");
        // Should include example models
        assert!(items.iter().any(|i| i.label.contains("claude")));
    }

    #[test]
    fn complete_value_unknown_key() {
        let items = completer().complete_value("nonexistent", "");
        assert!(items.is_empty());
    }

    #[test]
    fn complete_value_case_insensitive() {
        let items = completer().complete_value("apiProvider", "ANTH");
        assert!(items.iter().any(|i| i.label == "anthropic"));
    }

    #[test]
    fn complete_value_no_duplicates() {
        // theme has both enum and default "auto" — should not duplicate
        let items = completer().complete_value("theme", "");
        let auto_count = items.iter().filter(|i| i.label == "auto").count();
        assert_eq!(auto_count, 1);
    }

    // ── Misc ────────────────────────────────────────────────────────────

    #[test]
    fn format_value_string() {
        assert_eq!(format_value(&Value::String("test".into())), "test");
    }

    #[test]
    fn format_value_number() {
        assert_eq!(format_value(&serde_json::json!(42)), "42");
    }

    #[test]
    fn default_completer() {
        let c = SettingsCompleter::default();
        assert!(!c.complete_key("").is_empty());
    }

    #[test]
    fn completion_item_equality() {
        let a = CompletionItem {
            label: "test".into(),
            detail: "d".into(),
            insert_text: "test".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
