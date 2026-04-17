use std::collections::HashMap;

// ── Graceful degradation ──────────────────────────────────────────────

/// Feature that can be degraded (disabled) under error conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradableFeature {
    /// Context window auto-injection of relevant files.
    SmartContext,
    /// Memory retrieval and injection.
    MemoryRetrieval,
    /// Tool execution (fall back to text-only responses).
    ToolExecution,
    /// Streaming output (fall back to batch).
    Streaming,
    /// Multi-agent coordination.
    MultiAgent,
    /// Code navigation features.
    CodeNavigation,
}

impl std::fmt::Display for DegradableFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SmartContext => write!(f, "smart_context"),
            Self::MemoryRetrieval => write!(f, "memory_retrieval"),
            Self::ToolExecution => write!(f, "tool_execution"),
            Self::Streaming => write!(f, "streaming"),
            Self::MultiAgent => write!(f, "multi_agent"),
            Self::CodeNavigation => write!(f, "code_navigation"),
        }
    }
}

/// Priority level for a degradable feature (lower = shed first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeaturePriority(pub u8);

impl FeaturePriority {
    /// Core features that should almost never be disabled.
    pub const CORE: Self = Self(100);
    /// Important but not critical.
    pub const HIGH: Self = Self(75);
    /// Nice-to-have features.
    pub const MEDIUM: Self = Self(50);
    /// Optional enhancements.
    pub const LOW: Self = Self(25);
}

/// Manages graceful degradation: disables non-essential features when
/// errors accumulate to maintain basic functionality.
#[derive(Debug)]
pub struct GracefulDegradation {
    /// Feature -> (priority, enabled).
    features: HashMap<DegradableFeature, (FeaturePriority, bool)>,
    /// Current degradation level (0 = normal, higher = more degraded).
    degradation_level: u8,
    /// Maximum degradation level.
    max_level: u8,
}

impl GracefulDegradation {
    /// Create with default feature set and priorities.
    #[must_use]
    pub fn new() -> Self {
        let mut features = HashMap::new();
        features.insert(
            DegradableFeature::SmartContext,
            (FeaturePriority::LOW, true),
        );
        features.insert(
            DegradableFeature::MemoryRetrieval,
            (FeaturePriority::MEDIUM, true),
        );
        features.insert(
            DegradableFeature::CodeNavigation,
            (FeaturePriority::LOW, true),
        );
        features.insert(DegradableFeature::Streaming, (FeaturePriority::HIGH, true));
        features.insert(
            DegradableFeature::MultiAgent,
            (FeaturePriority::MEDIUM, true),
        );
        features.insert(
            DegradableFeature::ToolExecution,
            (FeaturePriority::CORE, true),
        );

        Self {
            features,
            degradation_level: 0,
            max_level: 4,
        }
    }

    /// Check if a feature is currently enabled.
    #[must_use]
    pub fn is_enabled(&self, feature: DegradableFeature) -> bool {
        self.features
            .get(&feature)
            .is_some_and(|(_, enabled)| *enabled)
    }

    /// Increase degradation level, disabling lowest-priority features first.
    ///
    /// Returns the list of features that were disabled in this step.
    pub fn degrade(&mut self) -> Vec<DegradableFeature> {
        if self.degradation_level >= self.max_level {
            return Vec::new();
        }

        self.degradation_level += 1;
        let threshold = self.priority_threshold();

        let mut disabled = Vec::new();
        for (feature, (priority, enabled)) in &mut self.features {
            if *enabled && priority.0 < threshold {
                *enabled = false;
                disabled.push(*feature);
            }
        }

        disabled
    }

    /// Decrease degradation level, re-enabling features.
    ///
    /// Returns the list of features that were re-enabled.
    pub fn recover(&mut self) -> Vec<DegradableFeature> {
        if self.degradation_level == 0 {
            return Vec::new();
        }

        self.degradation_level -= 1;
        let threshold = self.priority_threshold();

        let mut enabled = Vec::new();
        for (feature, (priority, is_enabled)) in &mut self.features {
            if !*is_enabled && priority.0 >= threshold {
                *is_enabled = true;
                enabled.push(*feature);
            }
        }

        enabled
    }

    /// Reset to full functionality.
    pub fn reset(&mut self) {
        self.degradation_level = 0;
        for (_, enabled) in self.features.values_mut() {
            *enabled = true;
        }
    }

    /// Get the current degradation level (0 = normal).
    #[must_use]
    pub fn level(&self) -> u8 {
        self.degradation_level
    }

    /// Get the list of currently disabled features.
    #[must_use]
    pub fn disabled_features(&self) -> Vec<DegradableFeature> {
        self.features
            .iter()
            .filter(|(_, (_, enabled))| !enabled)
            .map(|(f, _)| *f)
            .collect()
    }

    /// Get the list of currently enabled features.
    #[must_use]
    pub fn enabled_features(&self) -> Vec<DegradableFeature> {
        self.features
            .iter()
            .filter(|(_, (_, enabled))| *enabled)
            .map(|(f, _)| *f)
            .collect()
    }

    /// Manually disable a specific feature.
    pub fn disable_feature(&mut self, feature: DegradableFeature) {
        if let Some((_, enabled)) = self.features.get_mut(&feature) {
            *enabled = false;
        }
    }

    /// Manually enable a specific feature.
    pub fn enable_feature(&mut self, feature: DegradableFeature) {
        if let Some((_, enabled)) = self.features.get_mut(&feature) {
            *enabled = true;
        }
    }

    /// Compute the priority threshold for the current degradation level.
    /// Features with priority below this threshold are disabled.
    fn priority_threshold(&self) -> u8 {
        // Level 0: threshold 0 (nothing disabled)
        // Level 1: threshold 30 (LOW disabled)
        // Level 2: threshold 55 (LOW + MEDIUM disabled)
        // Level 3: threshold 80 (LOW + MEDIUM + HIGH disabled)
        // Level 4: threshold 101 (everything disabled)
        match self.degradation_level {
            0 => 0,
            1 => 30,
            2 => 55,
            3 => 80,
            _ => 101,
        }
    }
}

impl Default for GracefulDegradation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::category::ErrorClassifier;
    use super::super::strategy::{RecoveryAction, RecoveryStrategy};
    use super::*;

    // ── DegradableFeature ──────────────────────────────────────────

    #[test]
    fn degradable_feature_display() {
        assert_eq!(DegradableFeature::SmartContext.to_string(), "smart_context");
        assert_eq!(
            DegradableFeature::ToolExecution.to_string(),
            "tool_execution"
        );
        assert_eq!(DegradableFeature::Streaming.to_string(), "streaming");
    }

    // ── FeaturePriority ────────────────────────────────────────────

    #[test]
    fn priority_ordering() {
        assert!(FeaturePriority::LOW < FeaturePriority::MEDIUM);
        assert!(FeaturePriority::MEDIUM < FeaturePriority::HIGH);
        assert!(FeaturePriority::HIGH < FeaturePriority::CORE);
    }

    // ── GracefulDegradation ────────────────────────────────────────

    #[test]
    fn degradation_starts_normal() {
        let gd = GracefulDegradation::new();
        assert_eq!(gd.level(), 0);
        assert!(gd.is_enabled(DegradableFeature::SmartContext));
        assert!(gd.is_enabled(DegradableFeature::ToolExecution));
        assert!(gd.disabled_features().is_empty());
    }

    #[test]
    fn degrade_level_1_disables_low_priority() {
        let mut gd = GracefulDegradation::new();
        let disabled = gd.degrade();
        assert_eq!(gd.level(), 1);
        // LOW priority features (SmartContext, CodeNavigation) should be disabled
        assert!(!gd.is_enabled(DegradableFeature::SmartContext));
        assert!(!gd.is_enabled(DegradableFeature::CodeNavigation));
        // MEDIUM and above still enabled
        assert!(gd.is_enabled(DegradableFeature::MemoryRetrieval));
        assert!(gd.is_enabled(DegradableFeature::ToolExecution));
        assert!(!disabled.is_empty());
    }

    #[test]
    fn degrade_level_2_disables_medium_priority() {
        let mut gd = GracefulDegradation::new();
        gd.degrade(); // level 1
        gd.degrade(); // level 2
        assert_eq!(gd.level(), 2);
        assert!(!gd.is_enabled(DegradableFeature::MemoryRetrieval));
        assert!(!gd.is_enabled(DegradableFeature::MultiAgent));
        // HIGH still enabled
        assert!(gd.is_enabled(DegradableFeature::Streaming));
        assert!(gd.is_enabled(DegradableFeature::ToolExecution));
    }

    #[test]
    fn degrade_level_3_disables_high_priority() {
        let mut gd = GracefulDegradation::new();
        gd.degrade();
        gd.degrade();
        gd.degrade();
        assert_eq!(gd.level(), 3);
        assert!(!gd.is_enabled(DegradableFeature::Streaming));
        // CORE still enabled
        assert!(gd.is_enabled(DegradableFeature::ToolExecution));
    }

    #[test]
    fn degrade_level_4_disables_everything() {
        let mut gd = GracefulDegradation::new();
        for _ in 0..4 {
            gd.degrade();
        }
        assert_eq!(gd.level(), 4);
        assert!(!gd.is_enabled(DegradableFeature::ToolExecution));
        assert!(gd.enabled_features().is_empty());
    }

    #[test]
    fn degrade_past_max_is_noop() {
        let mut gd = GracefulDegradation::new();
        for _ in 0..10 {
            gd.degrade();
        }
        assert_eq!(gd.level(), 4); // capped at max
    }

    #[test]
    fn recover_re_enables_features() {
        let mut gd = GracefulDegradation::new();
        gd.degrade(); // level 1
        assert!(!gd.is_enabled(DegradableFeature::SmartContext));

        let restored = gd.recover(); // back to level 0
        assert_eq!(gd.level(), 0);
        assert!(gd.is_enabled(DegradableFeature::SmartContext));
        assert!(!restored.is_empty());
    }

    #[test]
    fn recover_at_zero_is_noop() {
        let mut gd = GracefulDegradation::new();
        let restored = gd.recover();
        assert!(restored.is_empty());
        assert_eq!(gd.level(), 0);
    }

    #[test]
    fn reset_restores_all() {
        let mut gd = GracefulDegradation::new();
        gd.degrade();
        gd.degrade();
        gd.degrade();

        gd.reset();
        assert_eq!(gd.level(), 0);
        assert!(gd.disabled_features().is_empty());
        assert_eq!(gd.enabled_features().len(), 6);
    }

    #[test]
    fn manual_disable_enable() {
        let mut gd = GracefulDegradation::new();
        assert!(gd.is_enabled(DegradableFeature::Streaming));

        gd.disable_feature(DegradableFeature::Streaming);
        assert!(!gd.is_enabled(DegradableFeature::Streaming));

        gd.enable_feature(DegradableFeature::Streaming);
        assert!(gd.is_enabled(DegradableFeature::Streaming));
    }

    #[test]
    fn disabled_and_enabled_features_consistent() {
        let mut gd = GracefulDegradation::new();
        let total = gd.enabled_features().len() + gd.disabled_features().len();
        assert_eq!(total, 6);

        gd.degrade();
        let total = gd.enabled_features().len() + gd.disabled_features().len();
        assert_eq!(total, 6);
    }

    // ── Integration: classify + strategy ───────────────────────────

    #[test]
    fn end_to_end_classify_and_recover_transient() {
        let cat = ErrorClassifier::classify("503 Service Unavailable");
        let strategy = RecoveryStrategy::default();
        let action = strategy.recommend(cat);
        assert!(matches!(action, RecoveryAction::Retry { .. }));
    }

    #[test]
    fn end_to_end_classify_and_recover_auth() {
        let cat = ErrorClassifier::classify("Invalid API key");
        let strategy = RecoveryStrategy::default();
        let action = strategy.recommend(cat);
        assert!(matches!(action, RecoveryAction::AskUser { .. }));
    }

    #[test]
    fn end_to_end_classify_and_recover_permanent() {
        let cat = ErrorClassifier::classify("400 Bad Request: malformed JSON");
        let strategy = RecoveryStrategy::default();
        let action = strategy.recommend(cat);
        // "malformed" matches Permanent before Transient patterns are checked
        assert!(matches!(action, RecoveryAction::Abort { .. }));
    }

    #[test]
    fn end_to_end_status_and_recover() {
        let cat = ErrorClassifier::classify_status(429);
        let strategy = RecoveryStrategy::default();
        let action = strategy.recommend_with_attempts(cat, 0);
        assert!(matches!(action, RecoveryAction::Retry { .. }));
    }
}
