//! Configuration for replay checkpoints and their maintenance.

use crate::config::{ConfigKey, ConfigScope};
use crate::{config_keys, config_scopes, BundlebaseError, ConfigProvider};
use std::time::Duration;

config_scopes!(checkpoint_scopes, {
    pub const CHECKPOINT_SCOPE: ConfigScope = ConfigScope::new("checkpoint");
});

config_keys!(checkpoint_keys, {
    pub const EVERY_CFG: ConfigKey = CHECKPOINT_SCOPE.define("every").with_default("50");
    pub const MAX_AGE_CFG: ConfigKey = CHECKPOINT_SCOPE.define("max_age").with_default("3d");
    pub const GRACE_CFG: ConfigKey = CHECKPOINT_SCOPE.define("grace").with_default("1h");
    pub const ENABLED_CFG: ConfigKey = CHECKPOINT_SCOPE
        .define("enabled")
        .runtime_only()
        .with_default("true");
});

/// Resolved checkpoint settings. An interval of zero disables automatic checkpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointConfig {
    pub every: u64,
    pub max_age: Duration,
    pub grace: Duration,
    pub enabled: bool,
}

impl CheckpointConfig {
    pub fn from_provider(config: &dyn ConfigProvider) -> Result<Self, BundlebaseError> {
        let every = config.get(&EVERY_CFG)?.unwrap_or_else(|| "50".to_string());
        let max_age = config
            .get(&MAX_AGE_CFG)?
            .unwrap_or_else(|| "3d".to_string());
        let grace = config.get(&GRACE_CFG)?.unwrap_or_else(|| "1h".to_string());
        let enabled = config
            .get(&ENABLED_CFG)?
            .unwrap_or_else(|| "true".to_string());
        Ok(Self {
            every: every
                .parse()
                .map_err(|e| format!("Invalid checkpoint.every '{}': {}", every, e))?,
            max_age: parse_duration(&max_age)
                .map_err(|e| format!("Invalid checkpoint.max_age: {}", e))?,
            grace: parse_duration(&grace)
                .map_err(|e| format!("Invalid checkpoint.grace: {}", e))?,
            enabled: enabled
                .parse()
                .map_err(|e| format!("Invalid checkpoint.enabled '{}': {}", enabled, e))?,
        })
    }
}

/// Parse an integer duration with an explicit seconds, minutes, hours, or days suffix.
pub fn parse_duration(value: &str) -> Result<Duration, BundlebaseError> {
    let value = value.trim();
    let (number, unit) = value.split_at(
        value
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(value.len()),
    );
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => {
            return Err(format!(
                "Invalid duration '{}'; expected an integer followed by s, m, h, or d",
                value
            )
            .into())
        }
    };
    let amount: u64 = number
        .parse()
        .map_err(|e| format!("Invalid duration '{}': {}", value, e))?;
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("Duration '{}' is too large", value))?;
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestConfig {
        every: Option<&'static str>,
        enabled: Option<&'static str>,
    }

    impl ConfigProvider for TestConfig {
        fn get_in_scope(
            &self,
            _scope: &crate::Scope,
            key: &ConfigKey,
        ) -> Result<Option<String>, BundlebaseError> {
            Ok(match key.key {
                "every" => self.every.map(str::to_string),
                "enabled" => self.enabled.map(str::to_string),
                _ => None,
            })
        }
    }

    #[test]
    fn defaults_and_disabled_automatic_checkpoints() -> Result<(), BundlebaseError> {
        let defaults = CheckpointConfig::from_provider(&TestConfig {
            every: None,
            enabled: None,
        })?;
        assert_eq!(defaults.every, 50);
        assert_eq!(defaults.max_age, Duration::from_secs(259200));
        assert_eq!(defaults.grace, Duration::from_secs(3600));
        assert!(defaults.enabled);
        let disabled = CheckpointConfig::from_provider(&TestConfig {
            every: Some("0"),
            enabled: Some("false"),
        })?;
        assert_eq!(disabled.every, 0);
        assert!(!disabled.enabled);
        assert!(CheckpointConfig::from_provider(&TestConfig {
            every: Some("invalid"),
            enabled: None
        })
        .is_err());
        assert!(CheckpointConfig::from_provider(&TestConfig {
            every: None,
            enabled: Some("invalid")
        })
        .is_err());
        Ok(())
    }

    #[test]
    fn duration_units_and_overflow() -> Result<(), BundlebaseError> {
        assert_eq!(parse_duration("3d")?, Duration::from_secs(259200));
        assert_eq!(parse_duration("1h")?, Duration::from_secs(3600));
        assert_eq!(parse_duration("2m")?, Duration::from_secs(120));
        assert_eq!(parse_duration("0s")?, Duration::ZERO);
        for invalid in ["", "1", "-1h", "1.5h", "day", "18446744073709551615d"] {
            assert!(parse_duration(invalid).is_err(), "{}", invalid);
        }
        Ok(())
    }
}
