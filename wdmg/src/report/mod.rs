//! HTML report generation.
//!
//! Renders a SINGLE self-contained `report.html` from an `analysis.json`
//! ([`Analysis`]) and an optional, LLM-authored `insights.json` ([`Insights`]).
//!
//! The output opens via `file://` with no network access: the vendored uPlot CSS
//! and JS are inlined at compile time via `include_str!`, and the merged data is
//! embedded as a JSON island (`<script type="application/json">`) that the small,
//! dependency-free glue script reads to draw the interactive cash-flow chart.
//!
//! ## Escaping discipline (per security: injection prevention)
//! askama auto-escapes every templated value, so analysis-derived text (merchant
//! names, categories, descriptions) and insights text cannot inject markup. Only
//! three blocks are emitted raw (`|safe`): the vendored CSS, the vendored JS, and
//! the JSON island. `serde_json` does NOT escape `<`/`>` by default, so [`render`]
//! post-processes the island to escape `</` as `<\/` (a valid JSON escape) so a
//! merchant/description containing `</script>` cannot break out of the
//! `<script type="application/json">` context.

pub mod insights;

use std::path::Path;

use askama::Template;
use serde::Serialize;

use crate::emit::Analysis;
use crate::error::AnalyticsError;

pub use insights::Insights;

/// Vendored uPlot CSS, inlined at compile time. Path is relative to this source
/// file (`src/report/`), reaching the crate's sibling `assets/vendor/` dir.
const UPLOT_CSS: &str = include_str!("../../../assets/vendor/uPlot.min.css");
/// Vendored uPlot JS (IIFE build), inlined at compile time.
const UPLOT_JS: &str = include_str!("../../../assets/vendor/uPlot.iife.min.js");

/// The merged document embedded in the report's JSON island: the analysis plus
/// the (possibly empty) insights, so the glue script has everything in one place.
#[derive(Debug, Serialize)]
struct ReportData<'a> {
    analysis: &'a Analysis,
    insights: &'a Insights,
}

/// The askama-rendered report template.
///
/// Typed sections (tables, bars, the transfer graph, breakdowns) are rendered by
/// askama with auto-escaping. The interactive cash-flow chart is drawn client-side
/// from the JSON island. `uplot_css`/`uplot_js`/`data_json` are emitted raw.
#[derive(Template)]
#[template(path = "report.html")]
struct ReportTemplate<'a> {
    analysis: &'a Analysis,
    insights: &'a Insights,
    /// Whether the narrative (insights) sections have any content.
    has_insights: bool,
    /// Inlined vendored uPlot CSS (rendered raw).
    uplot_css: &'a str,
    /// Inlined vendored uPlot JS (rendered raw).
    uplot_js: &'a str,
    /// The merged data, pre-serialized to JSON (rendered raw into the island).
    data_json: String,
}

/// Render the report to an HTML string.
///
/// `insights` is the (possibly empty) narrative layer; pass `&Insights::default()`
/// when none was supplied.
pub fn render(analysis: &Analysis, insights: &Insights) -> Result<String, AnalyticsError> {
    let data = ReportData { analysis, insights };
    // Compact JSON keeps the island small. serde_json does NOT escape `<`/`>` by
    // default, so a merchant/description containing the literal `</script>` would
    // otherwise terminate the surrounding <script> element. Escape `</` to `<\/`
    // (a valid JSON string escape that JSON.parse restores to `/`) so the payload
    // can never break out of the JSON island. (security: injection prevention)
    let data_json = serde_json::to_string(&data)
        .map_err(|source| AnalyticsError::Serialize {
            what: "report data island".to_string(),
            source,
        })?
        .replace("</", "<\\/");

    let tmpl = ReportTemplate {
        analysis,
        insights,
        has_insights: !insights.is_empty(),
        uplot_css: UPLOT_CSS,
        uplot_js: UPLOT_JS,
        data_json,
    };

    tmpl.render().map_err(|source| AnalyticsError::Serialize {
        what: "report.html template".to_string(),
        // askama's error isn't a serde_json::Error; wrap its message instead.
        source: serde_json::Error::io(std::io::Error::other(source.to_string())),
    })
}

/// Render and write `report.html` to `out_path`.
pub fn write_report(
    out_path: &Path,
    analysis: &Analysis,
    insights: &Insights,
) -> Result<(), AnalyticsError> {
    let html = render(analysis, insights)?;
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| AnalyticsError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }
    std::fs::write(out_path, html.as_bytes()).map_err(|source| AnalyticsError::Write {
        path: out_path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::pipeline;
    use std::io::Write;

    fn sample_analysis() -> Analysis {
        let cfg = Config::from_defaults().unwrap();
        let tmp = std::env::temp_dir().join(format!("wdmg-report-{}", std::process::id()));
        let input = tmp.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let csv = "\"Description\",\"Type\",\"Card Holder Name\",\"Date\",\"Time\",\"Amount\"\n\
             \"SHELL\",\"PURCHASE\",\"FRY\",\"01/05/2026\",\"10:00 AM\",\"-60.00\"\n\
             \"NETFLIX\",\"PURCHASE\",\"FRY\",\"02/06/2026\",\"10:00 AM\",\"-16.99\"\n\
             \"PAYMENT - THANK YOU\",\"PAYMENT\",\"FRY\",\"01/10/2026\",\"10:00 AM\",\"500.00\"\n";
        let mut f = std::fs::File::create(input.join("7001 xxxx xxxx 7003.CSV")).unwrap();
        f.write_all(csv.as_bytes()).unwrap();
        let outcome = pipeline::analyze(&cfg, &input).unwrap();
        std::fs::remove_dir_all(&tmp).ok();
        outcome.analysis
    }

    #[test]
    fn renders_self_contained_html_without_insights() {
        let analysis = sample_analysis();
        let html = render(&analysis, &Insights::default()).unwrap();

        // Self-contained: no network LOADS. (The vendored uPlot bundle carries its
        // MIT license banner, which contains a github URL in a JS comment — that is
        // retained per the license and is not a network load, so we assert against
        // actual loading constructs rather than the mere substring "https://".)
        assert!(!html.contains("http://"), "must not reference http://");
        assert!(
            !html.contains("<link"),
            "must not link external stylesheets"
        );
        assert!(
            !html.contains("src=\"http") && !html.contains("src='http"),
            "must not load external scripts/images"
        );
        assert!(
            !html.contains("href=\"http") && !html.contains("href='http"),
            "must not reference external hrefs"
        );
        assert!(!html.contains("@import"), "must not @import external css");
        assert!(
            !html.to_lowercase().contains("cdn"),
            "must not reference a CDN"
        );
        // The only absolute URL permitted is the single vendored license banner.
        assert_eq!(
            html.matches("https://").count(),
            1,
            "only the uPlot license URL"
        );

        // The chart container and the inlined uPlot global must be present.
        assert!(html.contains("id=\"cashflow-chart\""));
        assert!(html.contains("id=\"wdmg-data\""));
        assert!(html.contains("uPlot")); // from the inlined IIFE
        assert!(html.contains("system-ui")); // system font stack

        // Without insights, the recommendations call-to-action placeholder shows.
        assert!(html.to_lowercase().contains("insight"));
        assert!(html.len() > 50_000, "report should be non-trivial in size");
    }

    #[test]
    fn renders_recommendations_when_insights_present() {
        let analysis = sample_analysis();
        let insights = Insights::from_json(
            r#"{ "summary": "You saved more than usual.",
                 "recommendations": [
                   { "title": "Cancel Netflix", "detail": "Unused since Jan.",
                     "est_monthly_savings": "16.99" }
                 ] }"#,
        )
        .unwrap();
        let html = render(&analysis, &insights).unwrap();
        assert!(html.contains("Cancel Netflix"));
        assert!(html.contains("Unused since Jan."));
        assert!(html.contains("You saved more than usual."));
        assert!(html.contains("16.99"));
    }

    #[test]
    fn escapes_injected_markup_from_insights() {
        let analysis = sample_analysis();
        let insights =
            Insights::from_json(r#"{ "summary": "<script>alert(1)</script>" }"#).unwrap();
        let html = render(&analysis, &insights).unwrap();
        // The raw <script> from user/LLM text must be escaped, not emitted live.
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&#60;script&#62;") || html.contains("&lt;script&gt;"));
    }
}
