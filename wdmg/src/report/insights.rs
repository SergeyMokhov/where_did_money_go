//! The optional, LLM-authored `insights.json` document.
//!
//! This file is written by the host agent after reading `analysis.json`; it is
//! entirely optional and may be absent or partial. Per defensive-coding
//! ("parse, don't validate" at the boundary, and fail *open* for an optional
//! enrichment), every field is optional and unknown fields are ignored, so a
//! newer skill revision that adds keys never breaks an older binary.

use serde::{Deserialize, Serialize};

/// The narrative enrichment layer. Every field is optional: a missing
/// `insights.json`, or one with only some keys, still produces a valid report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Insights {
    /// A short prose summary of the period.
    pub summary: Option<String>,
    /// Actionable recommendations.
    pub recommendations: Vec<Recommendation>,
    /// Notes attached to specific anomalies (by free-form `ref`).
    pub anomaly_notes: Vec<AnomalyNote>,
    /// Free-form notes about trends.
    pub trend_notes: Vec<String>,
}

/// One recommendation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recommendation {
    /// Short title.
    pub title: Option<String>,
    /// Longer explanation.
    pub detail: Option<String>,
    /// Estimated monthly savings (decimal string, to match money conventions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub est_monthly_savings: Option<String>,
}

/// A note tied to an anomaly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnomalyNote {
    /// Free-form reference to the anomaly (e.g. "Groceries 2026-03").
    pub r#ref: Option<String>,
    /// The note text.
    pub note: Option<String>,
}

impl Insights {
    /// Parse insights from a JSON string. Tolerant: unknown keys are ignored and
    /// any missing key defaults.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// True when there is no narrative content worth rendering as sections.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.summary.is_none()
            && self.recommendations.is_empty()
            && self.anomaly_notes.is_empty()
            && self.trend_notes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_parses_to_default() {
        let i = Insights::from_json("{}").unwrap();
        assert!(i.is_empty());
    }

    #[test]
    fn partial_and_unknown_keys_tolerated() {
        let i = Insights::from_json(
            r#"{ "summary": "Spending up.", "future_field": 42,
                 "recommendations": [{ "title": "Cancel X" }] }"#,
        )
        .unwrap();
        assert_eq!(i.summary.as_deref(), Some("Spending up."));
        assert_eq!(i.recommendations.len(), 1);
        assert_eq!(i.recommendations[0].title.as_deref(), Some("Cancel X"));
        assert!(i.recommendations[0].est_monthly_savings.is_none());
        assert!(!i.is_empty());
    }

    #[test]
    fn ref_field_renamed() {
        let i = Insights::from_json(
            r#"{ "anomaly_notes": [{ "ref": "Gas 2026-03", "note": "trip" }] }"#,
        )
        .unwrap();
        assert_eq!(i.anomaly_notes[0].r#ref.as_deref(), Some("Gas 2026-03"));
    }
}
