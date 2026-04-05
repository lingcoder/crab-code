//! Settings validation framework.
//!
//! Provides composable validators that check a `serde_json::Value` settings
//! object and return a list of [`ValidationError`]s with path, message, and
//! severity.

use serde_json::Value;

// ── Types ───────────────────────────────────────────────────────────────

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => f.write_str("error"),
            Self::Warning => f.write_str("warning"),
            Self::Info => f.write_str("info"),
        }
    }
}

/// A single validation finding.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// JSON path to the offending field (e.g. `"apiProvider"`).
    pub path: String,
    /// Human-readable description of the problem.
    pub message: String,
    /// How serious the finding is.
    pub severity: Severity,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.severity, self.path, self.message)
    }
}

/// Aggregated validation result.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub errors: Vec<ValidationError>,
}

impl ValidationResult {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.errors.iter().any(|e| e.severity == Severity::Error)
    }

    #[must_use]
    pub fn error_count(&self) -> usize {
        self.errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .count()
    }

    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.errors
            .iter()
            .filter(|e| e.severity == Severity::Warning)
            .count()
    }
}

// ── Validator trait ─────────────────────────────────────────────────────

/// A validator that checks a settings JSON value.
pub trait SettingsValidator: Send + Sync {
    fn validate(&self, settings: &Value) -> Vec<ValidationError>;
}

// ── Built-in validators ─────────────────────────────────────────────────

/// Checks that certain fields are present and non-null.
pub struct RequiredFieldValidator {
    fields: Vec<&'static str>,
}

impl RequiredFieldValidator {
    #[must_use]
    pub fn new(fields: Vec<&'static str>) -> Self {
        Self { fields }
    }
}

impl SettingsValidator for RequiredFieldValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        self.fields
            .iter()
            .filter(|field| {
                settings
                    .get(**field)
                    .is_none_or(Value::is_null)
            })
            .map(|field| ValidationError {
                path: (*field).to_string(),
                message: "required field is missing or null".to_string(),
                severity: Severity::Error,
            })
            .collect()
    }
}

/// Checks that fields have the expected JSON type.
pub struct TypeValidator {
    rules: Vec<(&'static str, ExpectedType)>,
}

/// Expected JSON type for a field.
#[derive(Debug, Clone, Copy)]
pub enum ExpectedType {
    String,
    Number,
    Bool,
    Object,
    Array,
}

impl std::fmt::Display for ExpectedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String => f.write_str("string"),
            Self::Number => f.write_str("number"),
            Self::Bool => f.write_str("boolean"),
            Self::Object => f.write_str("object"),
            Self::Array => f.write_str("array"),
        }
    }
}

impl TypeValidator {
    #[must_use]
    pub fn new(rules: Vec<(&'static str, ExpectedType)>) -> Self {
        Self { rules }
    }
}

impl SettingsValidator for TypeValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        self.rules
            .iter()
            .filter_map(|(field, expected)| {
                let Some(val) = settings.get(*field) else {
                    return None; // missing fields handled by RequiredFieldValidator
                };
                if val.is_null() {
                    return None; // null is acceptable (means "not set")
                }
                let ok = match expected {
                    ExpectedType::String => val.is_string(),
                    ExpectedType::Number => val.is_number(),
                    ExpectedType::Bool => val.is_boolean(),
                    ExpectedType::Object => val.is_object(),
                    ExpectedType::Array => val.is_array(),
                };
                if ok {
                    None
                } else {
                    Some(ValidationError {
                        path: (*field).to_string(),
                        message: format!("expected type {expected}"),
                        severity: Severity::Error,
                    })
                }
            })
            .collect()
    }
}

/// Checks that a numeric field is within a range.
pub struct RangeValidator {
    field: &'static str,
    min: Option<f64>,
    max: Option<f64>,
}

impl RangeValidator {
    #[must_use]
    pub fn new(field: &'static str, min: Option<f64>, max: Option<f64>) -> Self {
        Self { field, min, max }
    }
}

impl SettingsValidator for RangeValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        let Some(val) = settings.get(self.field) else {
            return Vec::new();
        };
        let Some(num) = val.as_f64() else {
            return Vec::new(); // type checking is handled by TypeValidator
        };
        let mut errs = Vec::new();
        if let Some(min) = self.min
            && num < min {
                errs.push(ValidationError {
                    path: self.field.to_string(),
                    message: format!("value {num} is below minimum {min}"),
                    severity: Severity::Error,
                });
            }
        if let Some(max) = self.max
            && num > max {
                errs.push(ValidationError {
                    path: self.field.to_string(),
                    message: format!("value {num} exceeds maximum {max}"),
                    severity: Severity::Error,
                });
            }
        errs
    }
}

/// Checks that a string field's value is one of the allowed values.
pub struct EnumValidator {
    field: &'static str,
    allowed: Vec<&'static str>,
}

impl EnumValidator {
    #[must_use]
    pub fn new(field: &'static str, allowed: Vec<&'static str>) -> Self {
        Self { field, allowed }
    }
}

impl SettingsValidator for EnumValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        let Some(val) = settings.get(self.field) else {
            return Vec::new();
        };
        let Some(s) = val.as_str() else {
            return Vec::new();
        };
        if self.allowed.contains(&s) {
            Vec::new()
        } else {
            vec![ValidationError {
                path: self.field.to_string(),
                message: format!(
                    "invalid value \"{s}\", expected one of: {}",
                    self.allowed.join(", ")
                ),
                severity: Severity::Error,
            }]
        }
    }
}

/// Checks that a string field looks like a valid URL.
pub struct UrlValidator {
    field: &'static str,
}

impl UrlValidator {
    #[must_use]
    pub fn new(field: &'static str) -> Self {
        Self { field }
    }
}

impl SettingsValidator for UrlValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        let Some(val) = settings.get(self.field) else {
            return Vec::new();
        };
        let Some(s) = val.as_str() else {
            return Vec::new();
        };
        if s.starts_with("http://") || s.starts_with("https://") {
            Vec::new()
        } else {
            vec![ValidationError {
                path: self.field.to_string(),
                message: "URL must start with http:// or https://".to_string(),
                severity: Severity::Warning,
            }]
        }
    }
}

/// Combines multiple validators.
pub struct CompositeValidator {
    validators: Vec<Box<dyn SettingsValidator>>,
}

impl CompositeValidator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            validators: Vec::new(),
        }
    }

    /// Add a validator to the composite.
    pub fn add(&mut self, validator: Box<dyn SettingsValidator>) {
        self.validators.push(validator);
    }

    /// Builder-style add.
    #[must_use]
    pub fn with(mut self, validator: Box<dyn SettingsValidator>) -> Self {
        self.validators.push(validator);
        self
    }
}

impl Default for CompositeValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsValidator for CompositeValidator {
    fn validate(&self, settings: &Value) -> Vec<ValidationError> {
        self.validators
            .iter()
            .flat_map(|v| v.validate(settings))
            .collect()
    }
}

// ── Convenience ─────────────────────────────────────────────────────────

/// Build the default settings validator with all built-in rules.
#[must_use]
pub fn default_validator() -> CompositeValidator {
    CompositeValidator::new()
        .with(Box::new(TypeValidator::new(vec![
            ("apiProvider", ExpectedType::String),
            ("apiBaseUrl", ExpectedType::String),
            ("apiKey", ExpectedType::String),
            ("model", ExpectedType::String),
            ("smallModel", ExpectedType::String),
            ("maxTokens", ExpectedType::Number),
            ("permissionMode", ExpectedType::String),
            ("systemPrompt", ExpectedType::String),
            ("mcpServers", ExpectedType::Object),
            ("hooks", ExpectedType::Object),
            ("theme", ExpectedType::String),
        ])))
        .with(Box::new(RangeValidator::new("maxTokens", Some(1.0), Some(1_000_000.0))))
        .with(Box::new(EnumValidator::new(
            "permissionMode",
            vec!["default", "trustProject", "dangerously"],
        )))
        .with(Box::new(EnumValidator::new(
            "apiProvider",
            vec!["anthropic", "openai", "deepseek", "ollama", "vllm", "bedrock", "vertex"],
        )))
        .with(Box::new(UrlValidator::new("apiBaseUrl")))
}

/// Validate settings using all default rules.
#[must_use]
pub fn validate_settings(settings: &Value) -> ValidationResult {
    let validator = default_validator();
    let errors = validator.validate(settings);
    ValidationResult { errors }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Error.to_string(), "error");
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(Severity::Info.to_string(), "info");
    }

    #[test]
    fn validation_error_display() {
        let e = ValidationError {
            path: "model".to_string(),
            message: "missing".to_string(),
            severity: Severity::Error,
        };
        assert_eq!(e.to_string(), "[error] model: missing");
    }

    #[test]
    fn validation_result_empty_is_valid() {
        let r = ValidationResult::default();
        assert!(r.is_valid());
        assert_eq!(r.error_count(), 0);
        assert_eq!(r.warning_count(), 0);
    }

    #[test]
    fn validation_result_with_warning_is_valid() {
        let r = ValidationResult {
            errors: vec![ValidationError {
                path: "x".into(),
                message: "warn".into(),
                severity: Severity::Warning,
            }],
        };
        assert!(r.is_valid());
        assert_eq!(r.warning_count(), 1);
    }

    #[test]
    fn validation_result_with_error_is_invalid() {
        let r = ValidationResult {
            errors: vec![ValidationError {
                path: "x".into(),
                message: "bad".into(),
                severity: Severity::Error,
            }],
        };
        assert!(!r.is_valid());
        assert_eq!(r.error_count(), 1);
    }

    // ── RequiredFieldValidator ───────────────────────────────────────────

    #[test]
    fn required_field_present() {
        let v = RequiredFieldValidator::new(vec!["model"]);
        let s = json!({"model": "gpt-4o"});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn required_field_missing() {
        let v = RequiredFieldValidator::new(vec!["model", "apiKey"]);
        let s = json!({"theme": "dark"});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 2);
        assert!(errs.iter().all(|e| e.severity == Severity::Error));
    }

    #[test]
    fn required_field_null_counts_as_missing() {
        let v = RequiredFieldValidator::new(vec!["model"]);
        let s = json!({"model": null});
        assert_eq!(v.validate(&s).len(), 1);
    }

    // ── TypeValidator ───────────────────────────────────────────────────

    #[test]
    fn type_validator_correct_types() {
        let v = TypeValidator::new(vec![
            ("name", ExpectedType::String),
            ("count", ExpectedType::Number),
            ("active", ExpectedType::Bool),
            ("meta", ExpectedType::Object),
            ("tags", ExpectedType::Array),
        ]);
        let s = json!({
            "name": "test",
            "count": 42,
            "active": true,
            "meta": {},
            "tags": ["a"]
        });
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn type_validator_wrong_type() {
        let v = TypeValidator::new(vec![("maxTokens", ExpectedType::Number)]);
        let s = json!({"maxTokens": "not a number"});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("number"));
    }

    #[test]
    fn type_validator_missing_field_ignored() {
        let v = TypeValidator::new(vec![("model", ExpectedType::String)]);
        let s = json!({});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn type_validator_null_field_ignored() {
        let v = TypeValidator::new(vec![("model", ExpectedType::String)]);
        let s = json!({"model": null});
        assert!(v.validate(&s).is_empty());
    }

    // ── RangeValidator ──────────────────────────────────────────────────

    #[test]
    fn range_within_bounds() {
        let v = RangeValidator::new("maxTokens", Some(1.0), Some(100_000.0));
        let s = json!({"maxTokens": 4096});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn range_below_min() {
        let v = RangeValidator::new("maxTokens", Some(1.0), None);
        let s = json!({"maxTokens": 0});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("below minimum"));
    }

    #[test]
    fn range_above_max() {
        let v = RangeValidator::new("maxTokens", None, Some(100.0));
        let s = json!({"maxTokens": 999});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("exceeds maximum"));
    }

    #[test]
    fn range_missing_field_ignored() {
        let v = RangeValidator::new("maxTokens", Some(1.0), Some(100.0));
        let s = json!({});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn range_non_numeric_ignored() {
        let v = RangeValidator::new("maxTokens", Some(1.0), Some(100.0));
        let s = json!({"maxTokens": "abc"});
        assert!(v.validate(&s).is_empty());
    }

    // ── EnumValidator ───────────────────────────────────────────────────

    #[test]
    fn enum_valid_value() {
        let v = EnumValidator::new("permissionMode", vec!["default", "dangerously"]);
        let s = json!({"permissionMode": "default"});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn enum_invalid_value() {
        let v = EnumValidator::new("permissionMode", vec!["default", "dangerously"]);
        let s = json!({"permissionMode": "yolo"});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("yolo"));
        assert!(errs[0].message.contains("default"));
    }

    #[test]
    fn enum_missing_field_ignored() {
        let v = EnumValidator::new("permissionMode", vec!["default"]);
        let s = json!({});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn enum_non_string_ignored() {
        let v = EnumValidator::new("permissionMode", vec!["default"]);
        let s = json!({"permissionMode": 42});
        assert!(v.validate(&s).is_empty());
    }

    // ── UrlValidator ────────────────────────────────────────────────────

    #[test]
    fn url_valid_http() {
        let v = UrlValidator::new("apiBaseUrl");
        let s = json!({"apiBaseUrl": "http://localhost:8080"});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn url_valid_https() {
        let v = UrlValidator::new("apiBaseUrl");
        let s = json!({"apiBaseUrl": "https://api.example.com"});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn url_invalid_scheme() {
        let v = UrlValidator::new("apiBaseUrl");
        let s = json!({"apiBaseUrl": "ftp://server"});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].severity, Severity::Warning);
    }

    #[test]
    fn url_missing_ignored() {
        let v = UrlValidator::new("apiBaseUrl");
        let s = json!({});
        assert!(v.validate(&s).is_empty());
    }

    // ── CompositeValidator ──────────────────────────────────────────────

    #[test]
    fn composite_empty_is_valid() {
        let v = CompositeValidator::new();
        let s = json!({});
        assert!(v.validate(&s).is_empty());
    }

    #[test]
    fn composite_aggregates_errors() {
        let v = CompositeValidator::new()
            .with(Box::new(RequiredFieldValidator::new(vec!["model"])))
            .with(Box::new(EnumValidator::new(
                "permissionMode",
                vec!["default"],
            )));
        let s = json!({"permissionMode": "yolo"});
        let errs = v.validate(&s);
        assert_eq!(errs.len(), 2); // model missing + invalid enum
    }

    #[test]
    fn composite_add_method() {
        let mut v = CompositeValidator::new();
        v.add(Box::new(RequiredFieldValidator::new(vec!["model"])));
        let s = json!({});
        assert_eq!(v.validate(&s).len(), 1);
    }

    // ── Default validator ───────────────────────────────────────────────

    #[test]
    fn default_validator_valid_settings() {
        let s = json!({
            "apiProvider": "anthropic",
            "model": "claude-3",
            "maxTokens": 4096,
            "permissionMode": "default",
            "apiBaseUrl": "https://api.anthropic.com"
        });
        let result = validate_settings(&s);
        assert!(result.is_valid());
    }

    #[test]
    fn default_validator_invalid_provider() {
        let s = json!({"apiProvider": "unknown-provider"});
        let result = validate_settings(&s);
        assert!(!result.is_valid());
    }

    #[test]
    fn default_validator_invalid_permission_mode() {
        let s = json!({"permissionMode": "yolo"});
        let result = validate_settings(&s);
        assert!(!result.is_valid());
    }

    #[test]
    fn default_validator_max_tokens_zero() {
        let s = json!({"maxTokens": 0});
        let result = validate_settings(&s);
        assert!(!result.is_valid());
    }

    #[test]
    fn default_validator_bad_url() {
        let s = json!({"apiBaseUrl": "not-a-url"});
        let result = validate_settings(&s);
        assert_eq!(result.warning_count(), 1);
    }

    #[test]
    fn default_validator_wrong_type_max_tokens() {
        let s = json!({"maxTokens": "big"});
        let result = validate_settings(&s);
        assert!(!result.is_valid());
    }

    #[test]
    fn default_validator_empty_settings_is_valid() {
        let s = json!({});
        let result = validate_settings(&s);
        assert!(result.is_valid()); // all fields optional
    }

    #[test]
    fn expected_type_display() {
        assert_eq!(ExpectedType::String.to_string(), "string");
        assert_eq!(ExpectedType::Number.to_string(), "number");
        assert_eq!(ExpectedType::Bool.to_string(), "boolean");
        assert_eq!(ExpectedType::Object.to_string(), "object");
        assert_eq!(ExpectedType::Array.to_string(), "array");
    }
}
