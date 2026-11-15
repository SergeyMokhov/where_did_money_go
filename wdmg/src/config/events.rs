//! Significant life events overlaid on the cash-flow chart.
//!
//! Users record milestones ("started new job", "bought a house") in an optional
//! `events.toml`. An event is either a **point** (just a `date`) drawn as a
//! vertical marker, or a **period** (a `date`..`end` range, e.g. a renovation or a
//! stretch of unemployment) drawn as a shaded band — so the financial effect of a
//! life event, instant or sustained, is visible at a glance. Events are pure
//! annotation — they never affect any computed number.
//!
//! Per "parse, don't validate": every date is checked to be a real ISO
//! `YYYY-MM-DD` calendar date here at the config boundary, a period's `end` is
//! required to be on/after its `date`, and the list is sorted by date, so
//! downstream code (and the report) can trust it.

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// The `events.toml` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsDoc {
    /// Schema version (currently always 1).
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The recorded events (TOML `[[event]]` tables).
    #[serde(default, rename = "event")]
    pub events: Vec<LifeEvent>,
}

fn default_schema_version() -> u32 {
    1
}

impl Default for EventsDoc {
    fn default() -> Self {
        Self {
            schema_version: 1,
            events: Vec::new(),
        }
    }
}

/// A milestone shown on the chart: a vertical marker (point) or, when `end` is
/// set, a shaded band spanning `date`..`end` (period).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeEvent {
    /// ISO calendar date `YYYY-MM-DD` — the point, or the start of a period.
    pub date: String,
    /// Optional ISO end date `YYYY-MM-DD`. When present (and on/after `date`), the
    /// event is a **period** drawn as a shaded band rather than a single line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// Short label shown next to the marker (e.g. "Started new job").
    pub label: String,
    /// Optional longer note shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl EventsDoc {
    /// Validate every event (real ISO date, non-empty label) and return the
    /// events sorted by date. Annotation-only, so validation is strict but the
    /// failure is a config error the user can fix, not a silent drop.
    pub fn validated(self) -> Result<Vec<LifeEvent>, ConfigError> {
        let mut events = self.events;
        for ev in &events {
            if ev.label.trim().is_empty() {
                return Err(ConfigError::Invalid {
                    context: format!("event on {}", ev.date),
                    message: "event label must not be empty".to_string(),
                });
            }
            let start = jiff::civil::Date::strptime("%Y-%m-%d", &ev.date).map_err(|source| {
                ConfigError::Invalid {
                    context: format!("event \"{}\"", ev.label),
                    message: format!(
                        "date \"{}\" is not a valid YYYY-MM-DD date: {source}",
                        ev.date
                    ),
                }
            })?;
            // A period must have a valid end on/after its start.
            if let Some(end) = &ev.end {
                let end_date = jiff::civil::Date::strptime("%Y-%m-%d", end).map_err(|source| {
                    ConfigError::Invalid {
                        context: format!("event \"{}\"", ev.label),
                        message: format!(
                            "end date \"{end}\" is not a valid YYYY-MM-DD date: {source}"
                        ),
                    }
                })?;
                if end_date < start {
                    return Err(ConfigError::Invalid {
                        context: format!("event \"{}\"", ev.label),
                        message: format!("end date \"{end}\" is before start date \"{}\"", ev.date),
                    });
                }
            }
        }
        // Deterministic order so analysis.json stays byte-identical across runs.
        events.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.label.cmp(&b.label)));
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts_events() {
        let doc: EventsDoc = toml::from_str(
            r#"
            schema_version = 1
            [[event]]
            date = "2024-06-01"
            label = "Moved cities"
            [[event]]
            date = "2023-01-15"
            label = "New job"
            note = "Bigger salary"
            "#,
        )
        .unwrap();
        let events = doc.validated().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].date, "2023-01-15"); // sorted ascending
        assert_eq!(events[0].note.as_deref(), Some("Bigger salary"));
    }

    #[test]
    fn rejects_bad_date() {
        let doc: EventsDoc = toml::from_str(
            r#"
            [[event]]
            date = "2024-13-99"
            label = "Nope"
            "#,
        )
        .unwrap();
        assert!(doc.validated().is_err());
    }

    #[test]
    fn rejects_empty_label() {
        let doc = EventsDoc {
            schema_version: 1,
            events: vec![LifeEvent {
                date: "2024-01-01".to_string(),
                end: None,
                label: "   ".to_string(),
                note: None,
            }],
        };
        assert!(doc.validated().is_err());
    }

    #[test]
    fn parses_period_event() {
        let doc: EventsDoc = toml::from_str(
            r#"
            [[event]]
            date  = "2024-03-01"
            end   = "2024-09-30"
            label = "Home renovation"
            "#,
        )
        .unwrap();
        let events = doc.validated().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].end.as_deref(), Some("2024-09-30"));
    }

    #[test]
    fn rejects_end_before_start() {
        let doc: EventsDoc = toml::from_str(
            r#"
            [[event]]
            date  = "2024-09-30"
            end   = "2024-03-01"
            label = "Backwards period"
            "#,
        )
        .unwrap();
        assert!(doc.validated().is_err());
    }

    #[test]
    fn rejects_bad_end_date() {
        let doc: EventsDoc = toml::from_str(
            r#"
            [[event]]
            date  = "2024-03-01"
            end   = "2024-13-40"
            label = "Bad end"
            "#,
        )
        .unwrap();
        assert!(doc.validated().is_err());
    }

    #[test]
    fn empty_doc_is_ok() {
        let events = EventsDoc::default().validated().unwrap();
        assert!(events.is_empty());
    }
}
