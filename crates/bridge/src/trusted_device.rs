//! Trusted device registration and verification.
//!
//! Manages a local registry of trusted devices (IDE instances) that
//! are allowed to connect to the bridge without re-authentication.

use serde::{Deserialize, Serialize};

/// A registered trusted device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    /// Unique device identifier (fingerprint).
    pub device_id: String,
    /// Human-readable device name.
    pub name: String,
    /// When the device was first trusted (Unix epoch seconds).
    pub trusted_at: u64,
    /// When the device last connected (Unix epoch seconds).
    pub last_seen: u64,
    /// Whether the device is currently active.
    pub active: bool,
}

/// The trusted device registry.
pub struct TrustedDeviceRegistry {
    /// Known trusted devices.
    devices: Vec<TrustedDevice>,
}

impl TrustedDeviceRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    /// Load the registry from the config directory.
    pub fn load() -> crab_common::Result<Self> {
        todo!("TrustedDeviceRegistry::load — read from ~/.crab/trusted_devices.json")
    }

    /// Save the registry to the config directory.
    pub fn save(&self) -> crab_common::Result<()> {
        todo!("TrustedDeviceRegistry::save — write to ~/.crab/trusted_devices.json")
    }

    /// Register a new trusted device.
    pub fn register(&mut self, device_id: impl Into<String>, name: impl Into<String>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.devices.push(TrustedDevice {
            device_id: device_id.into(),
            name: name.into(),
            trusted_at: now,
            last_seen: now,
            active: true,
        });
    }

    /// Check whether a device ID is trusted.
    #[must_use]
    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.devices
            .iter()
            .any(|d| d.device_id == device_id && d.active)
    }

    /// Revoke trust for a device.
    pub fn revoke(&mut self, device_id: &str) -> bool {
        if let Some(device) = self.devices.iter_mut().find(|d| d.device_id == device_id) {
            device.active = false;
            true
        } else {
            false
        }
    }

    /// List all registered devices.
    #[must_use]
    pub fn devices(&self) -> &[TrustedDevice] {
        &self.devices
    }

    /// Number of active trusted devices.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.devices.iter().filter(|d| d.active).count()
    }

    /// Update the last-seen timestamp for a device.
    pub fn touch(&mut self, device_id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(device) = self.devices.iter_mut().find(|d| d.device_id == device_id) {
            device.last_seen = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = TrustedDeviceRegistry::new();
        assert!(reg.devices().is_empty());
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn register_and_check_trust() {
        let mut reg = TrustedDeviceRegistry::new();
        reg.register("dev_123", "My Laptop");
        assert!(reg.is_trusted("dev_123"));
        assert!(!reg.is_trusted("dev_unknown"));
        assert_eq!(reg.active_count(), 1);
    }

    #[test]
    fn revoke_trust() {
        let mut reg = TrustedDeviceRegistry::new();
        reg.register("dev_123", "My Laptop");
        assert!(reg.is_trusted("dev_123"));
        let revoked = reg.revoke("dev_123");
        assert!(revoked);
        assert!(!reg.is_trusted("dev_123"));
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn revoke_unknown_returns_false() {
        let mut reg = TrustedDeviceRegistry::new();
        assert!(!reg.revoke("nonexistent"));
    }

    #[test]
    fn trusted_device_serde() {
        let device = TrustedDevice {
            device_id: "dev_1".into(),
            name: "Test".into(),
            trusted_at: 1000,
            last_seen: 2000,
            active: true,
        };
        let json = serde_json::to_string(&device).unwrap();
        let parsed: TrustedDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_id, "dev_1");
        assert!(parsed.active);
    }

    #[test]
    fn touch_updates_last_seen() {
        let mut reg = TrustedDeviceRegistry::new();
        reg.register("dev_1", "Test");
        let before = reg.devices()[0].last_seen;
        // Touch should update (or at least not decrease) last_seen
        reg.touch("dev_1");
        let after = reg.devices()[0].last_seen;
        assert!(after >= before);
    }
}
