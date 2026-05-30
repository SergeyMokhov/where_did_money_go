//! Analysis settings — chiefly the **persona profile** that tunes which
//! "unusual activity" detectors run, plus a couple of jurisdiction parameters.
//!
//! Different users fear different things: a consumer wants fraud + waste; a
//! business owner or public figure also wants forensic/compliance checks (Benford,
//! near-threshold structuring, round-number transfers) that would be noise to an
//! ordinary household. The profile gates those. Everything is optional — an absent
//! `settings.toml` yields the conservative `consumer` defaults.

use rust_decimal::dec;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Who the analysis is for; selects which detectors run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisProfile {
    /// Average consumer: fraud + waste detectors only (the default).
    #[default]
    Consumer,
    /// Tech-savvy prosumer: same detector set as consumer (subscription-heavy).
    Prosumer,
    /// Business owner: adds forensic/compliance detectors (Benford, structuring,
    /// round-number transfers).
    Business,
    /// Public figure: forensic/compliance detectors with disclosure emphasis.
    Public,
}

impl AnalysisProfile {
    /// Whether the forensic/compliance detectors (Benford, structuring,
    /// round-number-to-individual) are relevant to this profile. The engine now
    /// computes them regardless (so the report can switch lenses live); this is the
    /// **default visibility** the report opens with for the configured profile.
    #[must_use]
    pub fn includes_forensic(self) -> bool {
        matches!(self, AnalysisProfile::Business | AnalysisProfile::Public)
    }

    /// Stable lowercase tag (matches the `settings.toml` value and the report's
    /// lens selector).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AnalysisProfile::Consumer => "consumer",
            AnalysisProfile::Prosumer => "prosumer",
            AnalysisProfile::Business => "business",
            AnalysisProfile::Public => "public",
        }
    }
}

/// The `settings.toml` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsDoc {
    /// Persona profile (defaults to `consumer`).
    pub analysis_profile: AnalysisProfile,
    /// Cash/transfer reporting threshold for the structuring detector, as a decimal
    /// string. Defaults to 10000 (Canada FINTRAC large-cash report / US CTR).
    pub structuring_threshold: String,
}

impl Default for SettingsDoc {
    fn default() -> Self {
        Self {
            analysis_profile: AnalysisProfile::Consumer,
            structuring_threshold: "10000".to_string(),
        }
    }
}

/// Validated analysis settings consumed by the analytics phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisSettings {
    /// Persona profile.
    pub profile: AnalysisProfile,
    /// Structuring reporting threshold (exact decimal).
    pub structuring_threshold: Decimal,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            profile: AnalysisProfile::Consumer,
            structuring_threshold: dec!(10000),
        }
    }
}

impl SettingsDoc {
    /// Parse/validate into [`AnalysisSettings`]. A malformed threshold falls back to
    /// the $10,000 default rather than failing the whole analysis (it's a tuning
    /// knob, not core data).
    #[must_use]
    pub fn validated(&self) -> AnalysisSettings {
        let structuring_threshold = self
            .structuring_threshold
            .parse::<Decimal>()
            .unwrap_or_else(|_| dec!(10000));
        AnalysisSettings {
            profile: self.analysis_profile,
            structuring_threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_consumer() {
        let s = SettingsDoc::default().validated();
        assert_eq!(s.profile, AnalysisProfile::Consumer);
        assert!(!s.profile.includes_forensic());
        assert_eq!(s.structuring_threshold, dec!(10000));
    }

    #[test]
    fn parses_profile_and_threshold() {
        let doc: SettingsDoc =
            toml::from_str("analysis_profile = \"public\"\nstructuring_threshold = \"15000\"\n")
                .unwrap();
        let s = doc.validated();
        assert_eq!(s.profile, AnalysisProfile::Public);
        assert!(s.profile.includes_forensic());
        assert_eq!(s.structuring_threshold, dec!(15000));
    }

    #[test]
    fn business_includes_forensic() {
        assert!(AnalysisProfile::Business.includes_forensic());
        assert!(!AnalysisProfile::Prosumer.includes_forensic());
    }
}
