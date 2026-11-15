//! Change-point (level-shift) engine over a daily cash-flow series.
//!
//! Pure and deterministic: no I/O, no clock, no randomness, no hashing — safe to
//! run identically natively (the `change_point` anomaly) and in WebAssembly (the
//! browser chart), so the anomaly list and the chart always agree.
//!
//! # Method
//! All detection works on **per-day rates** of blocks (cents/day), so blocks of
//! different lengths (calendar months) are comparable.
//! 1. **Blocks.** Non-overlapping blocks; partial blocks are excluded from
//!    detection. Default: `in` → the median gap between nonzero income days
//!    rounded up to 7/14/28 days; `net`/`out` → calendar months when there are
//!    ≥ 12 full months, else 7 days. Another scheme (7/14/28 days, calendar
//!    month) replaces the default only if it is materially quieter
//!    (< [`SCHEME_SWITCH`] × the default's noise score) — see `choose_scheme`.
//! 2. **Robustify.** Hampel-style local winsorization: each block rate is
//!    clamped to its rolling median (window 2m+1) ± 3·robust σ, so a splurge
//!    shorter than a level can't create one while genuine shifts survive.
//!    Noise σ = max(`1.4826·MAD(Δ)/√2`, `SD(Δ)/√2`) of the clipped series' first
//!    differences Δ (insensitive to level shifts; see `diff_sigma`). σ = 0 only
//!    for a genuinely flat series ⇒ no change points (fail closed).
//! 3. **Segment.** PELT (Killick, Fearnhead & Eckley 2012) with an L2 cost, a
//!    minimum segment of `max(2, ceil(min_days/block_days))` blocks and penalty
//!    `β = c·σ²·ln(n)` ([`PENALTY_STANDARD`] = 4, [`PENALTY_SENSITIVE`] = 2).
//!    Adjacent segments whose means differ by less than [`MERGE_STANDARD`] /
//!    [`MERGE_SENSITIVE`] σ — or by less than [`MATERIALITY`] of the median
//!    |block rate| — are then merged (smallest gap first).
//! 4. **Describe.** Per segment: level (length-weighted winsorized rate,
//!    cents/day), a 95% CI of the mean (within-segment robust σ), XmR natural
//!    limits for display buckets (Wheeler's median-moving-range variant,
//!    `center ± 3.145·median mR` on per-day bucket rates), a Wilson interval on
//!    the share of unusual buckets and (net) negative weeks. For the last
//!    segment: a Theil–Sen trend with Sen's rank-based 95% CI, and a `tentative`
//!    check on the too-short-to-confirm tail.
//!
//! Deviations from the original design and why (each tuned/verified on the
//! playground household and on synthetic noise sweeps):
//! - Month blocks for `net`/`out` and the adaptive scheme: with calendar-anchored
//!   paydays and bills, weekly blocks carry σ as large as a doubling of spend.
//! - Local (Hampel) instead of global winsorization: global `median ± 3·MAD`
//!   clipping flattens real shifts when the series is near-constant (salaries).
//! - σ includes the clipped SD: MAD alone is ~37% efficient and under-estimates
//!   σ from ~36 monthly blocks, splitting pure noise.
//! - Minimum 2 blocks per segment and a 5% materiality floor on merges.
//!
//! Display buckets use the same boundaries as the browser: calendar months;
//! 7-/14-day buckets anchored on Monday 1970-01-05; or single days.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Longest accepted series: 40 years of days.
pub const MAX_DAYS: usize = 40 * 366;
/// Largest accepted |daily value| in cents ($1 trillion) — keeps every i64 sum
/// over [`MAX_DAYS`] far from overflow (1e14 · 14 640 < 9.2e18).
pub const MAX_ABS_DAILY_CENTS: i64 = 100_000_000_000_000;
/// Default minimum segment length in days.
pub const DEFAULT_MIN_DAYS: i64 = 14;
/// PELT penalty multiplier `c` in `β = c·σ²·ln(n)`, standard sensitivity.
pub const PENALTY_STANDARD: f64 = 4.0;
/// PELT penalty multiplier, sensitive.
pub const PENALTY_SENSITIVE: f64 = 2.0;
/// Merge adjacent segments whose means differ by less than this many σ (standard).
pub const MERGE_STANDARD: f64 = 1.0;
/// Merge threshold in σ, sensitive.
pub const MERGE_SENSITIVE: f64 = 0.5;

/// Materiality floor for the merge step, as a fraction of the median |block sum|:
/// adjacent segments closer than this are merged even when σ is tiny. (Deviation
/// from the pure σ-relative rule: with very regular data σ can be ~1% of the
/// level, and a statistically "real" 1% shift is not a regime change anyone
/// should be told about.)
pub const MATERIALITY: f64 = 0.05;

/// Switch away from the default block scheme only when a candidate's noise score
/// is below this fraction of the default's (see `choose_scheme`).
pub const SCHEME_SWITCH: f64 = 0.5;
/// Minimum full blocks for an alternative scheme to be scored (a σ estimate from
/// fewer differences is too noisy to justify switching).
const SCHEME_MIN_BLOCKS: usize = 12;

/// MAD → σ consistency constant for normal data.
const MAD_TO_SIGMA: f64 = 1.4826;
/// IQR → σ consistency constant for normal data.
const IQR_TO_SIGMA: f64 = 1.349;
/// Winsorization half-width, in robust σ.
const WINSOR_K: f64 = 3.0;
/// Wheeler's scaling for natural limits from the *median* moving range.
const XMR_MEDIAN_K: f64 = 3.145;
/// Wheeler's scaling from the *average* moving range (fallback when median mR = 0).
const XMR_MEAN_K: f64 = 2.66;
/// Two-sided 95% normal quantile.
const Z95: f64 = 1.96;
/// Monday 1970-01-05 as a day number (days since 1970-01-01): the week anchor.
const WEEK_ANCHOR: i64 = 4;

/// Errors from validating a change-point request at the boundary.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChangePointError {
    /// The start date did not parse as `YYYY-MM-DD`.
    #[error("invalid start date {0:?}: expected YYYY-MM-DD")]
    InvalidDate(String),
    /// No daily values were supplied.
    #[error("daily series is empty")]
    Empty,
    /// The series is longer than [`MAX_DAYS`].
    #[error("daily series has {days} days; the maximum is {max} (40 years)")]
    TooLong {
        /// Supplied length.
        days: usize,
        /// Maximum accepted.
        max: usize,
    },
    /// A value exceeded [`MAX_ABS_DAILY_CENTS`] in magnitude.
    #[error("daily[{index}] = {value} is out of range (|value| must be <= {MAX_ABS_DAILY_CENTS})")]
    OutOfRange {
        /// Offending index.
        index: usize,
        /// Offending value.
        value: i64,
    },
    /// `in`/`out` are magnitudes and must not be negative.
    #[error(
        "daily[{index}] = {value} is negative, but metric {metric:?} is a non-negative magnitude"
    )]
    Negative {
        /// Offending index.
        index: usize,
        /// Offending value.
        value: i64,
        /// The metric.
        metric: Metric,
    },
    /// The series runs past the supported calendar range.
    #[error("series end date is outside the supported calendar range")]
    DateRange,
}

/// Which flow the series measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Signed net (in − out). Higher is good.
    Net,
    /// Inflow magnitude. Higher is good.
    In,
    /// Outflow magnitude. Lower is good.
    Out,
}

impl Metric {
    /// Whether a change of `delta` is good news for this metric.
    fn good(self, delta: f64) -> bool {
        match self {
            Metric::Net | Metric::In => delta > 0.0,
            Metric::Out => delta < 0.0,
        }
    }
}

/// Detection sensitivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Obvious regime shifts only.
    #[default]
    Standard,
    /// Smaller/shorter shifts too.
    Sensitive,
}

impl Sensitivity {
    fn penalty(self) -> f64 {
        match self {
            Sensitivity::Standard => PENALTY_STANDARD,
            Sensitivity::Sensitive => PENALTY_SENSITIVE,
        }
    }
    fn merge(self) -> f64 {
        match self {
            Sensitivity::Standard => MERGE_STANDARD,
            Sensitivity::Sensitive => MERGE_SENSITIVE,
        }
    }
}

/// Display-bucket granularity (boundaries match the browser chart exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// One bucket per day.
    Day,
    /// 7-day buckets anchored on Monday 1970-01-05.
    Week,
    /// 14-day buckets anchored on Monday 1970-01-05.
    Biweek,
    /// Calendar months.
    #[default]
    Month,
}

/// Detection options. `min_days` is clamped to `7..=365` when used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Sensitivity preset.
    pub sensitivity: Sensitivity,
    /// Minimum segment length in days (clamped to `7..=365`).
    pub min_days: i64,
    /// Display bucket granularity.
    pub bucket: Granularity,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sensitivity: Sensitivity::Standard,
            min_days: DEFAULT_MIN_DAYS,
            bucket: Granularity::Month,
        }
    }
}

/// A validated daily series: one i64 cents value per consecutive day from
/// `start` (zero-filled days included). Constructed only via [`DailySeries::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailySeries {
    start: Date,
    start_day: i64,
    daily: Vec<i64>,
    metric: Metric,
}

impl DailySeries {
    /// Validate and wrap a daily series ("parse, don't validate").
    pub fn new(start: Date, daily: Vec<i64>, metric: Metric) -> Result<Self, ChangePointError> {
        if daily.is_empty() {
            return Err(ChangePointError::Empty);
        }
        if daily.len() > MAX_DAYS {
            return Err(ChangePointError::TooLong {
                days: daily.len(),
                max: MAX_DAYS,
            });
        }
        for (index, &value) in daily.iter().enumerate() {
            if value.checked_abs().is_none_or(|a| a > MAX_ABS_DAILY_CENTS) {
                return Err(ChangePointError::OutOfRange { index, value });
            }
            if value < 0 && metric != Metric::Net {
                return Err(ChangePointError::Negative {
                    index,
                    value,
                    metric,
                });
            }
        }
        let start_day = day_number(start);
        // Every date in the series must be representable (cheap: check the end).
        let len = i64::try_from(daily.len()).map_err(|_| ChangePointError::DateRange)?;
        date_of(start_day + len - 1)?;
        Ok(Self {
            start,
            start_day,
            daily,
            metric,
        })
    }

    /// First day of the series.
    #[must_use]
    pub fn start(&self) -> Date {
        self.start
    }

    /// The daily values (cents).
    #[must_use]
    pub fn daily(&self) -> &[i64] {
        &self.daily
    }

    /// The metric.
    #[must_use]
    pub fn metric(&self) -> Metric {
        self.metric
    }

    /// Date of day offset `i` (validated in range at construction).
    fn date_at(&self, i: usize) -> Result<Date, ChangePointError> {
        let i = i64::try_from(i).map_err(|_| ChangePointError::DateRange)?;
        date_of(self.start_day + i)
    }
}

/// The JSON request accepted by the WebAssembly `changepoints` export.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// `YYYY-MM-DD` of `daily[0]`.
    pub start: String,
    /// Daily cents, zero-filled.
    pub daily: Vec<i64>,
    /// `net` | `in` | `out`.
    pub metric: Metric,
    /// `standard` (default) | `sensitive`.
    #[serde(default)]
    pub sensitivity: Sensitivity,
    /// Minimum segment length in days (default 14, clamped to 7..=365).
    #[serde(default = "default_min_days")]
    pub min_days: i64,
    /// `day` | `week` | `biweek` | `month` (default).
    #[serde(default)]
    pub bucket: Granularity,
}

fn default_min_days() -> i64 {
    DEFAULT_MIN_DAYS
}

impl Request {
    /// Validate the request and run the engine.
    pub fn run(self) -> Result<ChangePoints, ChangePointError> {
        let start: Date = self
            .start
            .trim()
            .parse()
            .map_err(|_| ChangePointError::InvalidDate(self.start.clone()))?;
        let series = DailySeries::new(start, self.daily, self.metric)?;
        detect(
            &series,
            Options {
                sensitivity: self.sensitivity,
                min_days: self.min_days,
                bucket: self.bucket,
            },
        )
    }
}

/// `k` successes out of `n` with a Wilson 95% score interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Proportion {
    /// Successes.
    pub k: u32,
    /// Trials.
    pub n: u32,
    /// Lower bound of the 95% interval.
    pub lo: f64,
    /// Upper bound of the 95% interval.
    pub hi: f64,
}

/// A stretch of days at one level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// First day.
    pub start: Date,
    /// Last day (inclusive). The last segment runs to the end of the series.
    pub end: Date,
    /// Days covered.
    pub days: u32,
    /// Mean level, cents per day (from winsorized full blocks).
    pub level_per_day: f64,
    /// 95% CI of the level, cents/day (`None` when no full block exists).
    pub ci95: Option<[f64; 2]>,
    /// Unusual full display buckets in this segment.
    pub unusual_buckets: Proportion,
    /// Metric `net` only: full Monday-anchored weeks in the segment with a
    /// negative net.
    pub neg_weeks: Option<Proportion>,
}

/// A confirmed level change between two segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// First day of the new level.
    pub date: Date,
    /// New level − old level, cents/day.
    pub delta_per_day: f64,
    /// Whether the change is good news for the metric.
    pub good: bool,
}

/// Whether a full bucket fell outside its segment's natural limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unusual {
    /// Above the upper natural limit.
    High,
    /// Below the lower natural limit.
    Low,
}

/// One display bucket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayBucket {
    /// Nominal bucket start (may precede the data for a partial first bucket).
    pub start: Date,
    /// Days of the bucket covered by data.
    pub days: u32,
    /// Exact sum of the covered days, cents.
    pub value: i64,
    /// `level_per_day · days` of the bucket's segment.
    pub expected: f64,
    /// Lower natural limit (cents, for `days`), if determinable.
    pub lo: Option<f64>,
    /// Upper natural limit, if determinable.
    pub hi: Option<f64>,
    /// The bucket is not fully inside the data range.
    pub partial: bool,
    /// High/low flag (never set on partial buckets).
    pub unusual: Option<Unusual>,
    /// Index into `segments` (by the bucket's covered midpoint).
    pub segment: usize,
}

/// A possible new level forming in the too-short-to-confirm tail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tentative {
    /// First day of the tail.
    pub start: Date,
    /// Tail length in days.
    pub days: u32,
    /// Tail mean, cents/day (winsorization bounds applied).
    pub level_per_day: f64,
    /// Tail level − last segment level, cents/day.
    pub delta_per_day: f64,
    /// Whether the move is good news for the metric.
    pub good: bool,
}

/// Direction of the last segment's trend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// CI entirely above zero.
    Rising,
    /// CI entirely below zero.
    Falling,
    /// CI spans zero.
    Flat,
}

/// Theil–Sen trend within the last segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trend {
    /// Start of the last segment.
    pub since: Date,
    /// The last segment's level, cents/day.
    pub level_per_day: f64,
    /// Slope, cents/day per day.
    pub slope: f64,
    /// Sen's rank-based 95% CI of the slope.
    pub slope_ci95: [f64; 2],
    /// Rising / falling / flat (flat when the CI spans 0).
    pub direction: Direction,
}

/// How detection blocks were formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Fixed `block_days`-day blocks from the first day.
    Fixed,
    /// Calendar months (`block_days` is the nominal 30).
    CalendarMonth,
}

/// Engine output (serialized as the `changepoints` JSON response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangePoints {
    /// Detection block length in days (nominal 30 for calendar months).
    pub block_days: u32,
    /// Fixed-length or calendar-month blocks.
    pub block_kind: BlockKind,
    /// Robust noise σ of block per-day rates, cents/day (0 ⇒ no detection).
    pub sigma: f64,
    /// Segments tiling the whole series, in order.
    pub segments: Vec<Segment>,
    /// Level changes (one per segment boundary).
    pub changes: Vec<Change>,
    /// Display buckets at the requested granularity.
    pub buckets: Vec<DisplayBucket>,
    /// A possible new level in the unconfirmable tail, if any.
    pub tentative: Option<Tentative>,
    /// Trend of the last segment, if it has ≥ 3 full blocks.
    pub trend: Option<Trend>,
}

// ---------------------------------------------------------------------------
// Calendar helpers (proleptic Gregorian day numbers; 0 = 1970-01-01)
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 (Hinnant's `days_from_civil`).
fn day_number(d: Date) -> i64 {
    let y = i64::from(d.year()) - i64::from(d.month() <= 2);
    let m = i64::from(d.month());
    let day = i64::from(d.day());
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`day_number`] (Hinnant's `civil_from_days`), range-checked by jiff.
fn date_of(z: i64) -> Result<Date, ChangePointError> {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let y = i16::try_from(y).map_err(|_| ChangePointError::DateRange)?;
    let m = i8::try_from(m).map_err(|_| ChangePointError::DateRange)?;
    let d = i8::try_from(d).map_err(|_| ChangePointError::DateRange)?;
    Date::new(y, m, d).map_err(|_| ChangePointError::DateRange)
}

/// Display bucket `[start_day, end_day)` containing day number `dn`.
fn bucket_bounds(g: Granularity, dn: i64) -> Result<(i64, i64), ChangePointError> {
    Ok(match g {
        Granularity::Day => (dn, dn + 1),
        Granularity::Week | Granularity::Biweek => {
            let len = if g == Granularity::Week { 7 } else { 14 };
            let s = WEEK_ANCHOR + (dn - WEEK_ANCHOR).div_euclid(len) * len;
            (s, s + len)
        }
        Granularity::Month => {
            let d = date_of(dn)?;
            let first = day_number(d.first_of_month());
            (first, first + i64::from(d.days_in_month()))
        }
    })
}

// ---------------------------------------------------------------------------
// Robust statistics (f64; deterministic, NaN-free on finite input)
// ---------------------------------------------------------------------------

fn sorted(xs: &[f64]) -> Vec<f64> {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// Median (mean of the two middle values for even n); 0 for empty input.
fn median(xs: &[f64]) -> f64 {
    let v = sorted(xs);
    let n = v.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => v[n / 2],
        _ => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Median absolute deviation (unscaled).
fn mad(xs: &[f64]) -> f64 {
    let m = median(xs);
    let dev: Vec<f64> = xs.iter().map(|x| (x - m).abs()).collect();
    median(&dev)
}

/// Linear-interpolated quantile (type 7) of a non-empty slice.
fn quantile(xs: &[f64], q: f64) -> f64 {
    let v = sorted(xs);
    if v.is_empty() {
        return 0.0;
    }
    let pos = q * (v.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Robust noise σ from first differences (insensitive to level shifts):
/// `1.4826·MAD(diffs)/√2`, falling back to `IQR(diffs)/1.349/√2`. May be 0.
fn robust_diff_sigma(x: &[f64]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let d: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
    let s = MAD_TO_SIGMA * mad(&d) / std::f64::consts::SQRT_2;
    if s > 0.0 {
        return s;
    }
    (quantile(&d, 0.75) - quantile(&d, 0.25)) / IQR_TO_SIGMA / std::f64::consts::SQRT_2
}

/// Noise σ for the penalty (cents/day): the larger of [`robust_diff_sigma`] and
/// the sample SD of the differences `/√2`, computed on the already
/// Hampel-clipped series.
///
/// Deviation from the pure MAD recipe, for two reasons: (1) MAD is only ~37%
/// efficient, so with the ~36 monthly blocks of a 3-year history it often
/// underestimates σ by 20–40% and PELT then splits pure noise (9% false changes
/// on white noise at c = 4, vs 1.5% with this estimator); the clipped SD is
/// efficient and, after clipping, still robust. (2) When > 75% of differences
/// are exactly 0 (fixed salaries) the robust scales are 0 even though the series
/// is not flat — a clean raise must still be found. Only a genuinely flat series
/// yields 0 (fail closed).
fn diff_sigma(x: &[f64]) -> f64 {
    let robust = robust_diff_sigma(x);
    if x.len() < 3 {
        return robust;
    }
    let d: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
    let m = mean(&d);
    let var = d.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (d.len() - 1) as f64;
    robust.max(var.sqrt() / std::f64::consts::SQRT_2)
}

/// Hampel-style local winsorization: clamp each value to the median of a
/// `2h+1` window (shifted inward at the edges) ± `WINSOR_K`·robust σ. A spike of
/// ≤ `h` blocks is clipped, while any run of ≥ `h+1` blocks at a new level is a
/// window majority and survives — unlike global `median ± 3·MAD` clipping, which
/// flattens genuine shifts when the series is nearly constant (MAD ≈ 0).
fn hampel(x: &[f64], h: usize) -> Vec<f64> {
    let n = x.len();
    let w = 2 * h + 1;
    if n < 3 {
        return x.to_vec();
    }
    let s = WINSOR_K * robust_diff_sigma(x);
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(h).min(n.saturating_sub(w));
            let hi = (lo + w).min(n);
            let med = median(&x[lo..hi]);
            x[i].clamp(med - s, med + s)
        })
        .collect()
}

/// Half-width of XmR natural limits for per-day rates, from consecutive rates:
/// `3.145·median(mR)`, falling back to `2.66·mean(mR)` when the median is 0.
/// `None` with fewer than two rates.
fn natural_halfwidth(rates: &[f64]) -> Option<f64> {
    if rates.len() < 2 {
        return None;
    }
    let mr: Vec<f64> = rates.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let med = median(&mr);
    if med > 0.0 {
        Some(XMR_MEDIAN_K * med)
    } else {
        Some(XMR_MEAN_K * mean(&mr))
    }
}

/// Wilson 95% score interval for `k` of `n` (`[0, 1]` when `n == 0`).
#[must_use]
pub fn wilson(k: u32, n: u32) -> Proportion {
    if n == 0 {
        return Proportion {
            k,
            n,
            lo: 0.0,
            hi: 1.0,
        };
    }
    let nf = f64::from(n);
    let p = f64::from(k.min(n)) / nf;
    let z2 = Z95 * Z95;
    let denom = 1.0 + z2 / nf;
    let center = (p + z2 / (2.0 * nf)) / denom;
    let half = Z95 * (p * (1.0 - p) / nf + z2 / (4.0 * nf * nf)).sqrt() / denom;
    Proportion {
        k,
        n,
        lo: (center - half).max(0.0),
        hi: (center + half).min(1.0),
    }
}

/// Theil–Sen slope of `(x, y)` with Sen's rank-based 95% CI. `None` for n < 3.
fn theil_sen(x: &[f64], y: &[f64]) -> Option<(f64, [f64; 2])> {
    let n = x.len().min(y.len());
    if n < 3 {
        return None;
    }
    let mut slopes = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in i + 1..n {
            let dx = x[j] - x[i];
            if dx != 0.0 {
                slopes.push((y[j] - y[i]) / dx);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_by(f64::total_cmp);
    let slope = median(&slopes);
    // Sen (1968): C = z·sqrt(Var(S)), Var(S) = n(n−1)(2n+5)/18 (no tie correction).
    // Lower = the M1-th, upper = the (M2+1)-th ordered slope (1-based).
    let nf = n as f64;
    let big_n = slopes.len() as f64;
    let c = Z95 * (nf * (nf - 1.0) * (2.0 * nf + 5.0) / 18.0).sqrt();
    let m1 = ((big_n - c) / 2.0).round();
    let m2 = ((big_n + c) / 2.0).round();
    let last = slopes.len() - 1;
    let lo_idx = if m1 < 1.0 {
        0
    } else {
        (m1 as usize - 1).min(last)
    };
    let hi_idx = if m2 < 0.0 { 0 } else { (m2 as usize).min(last) };
    Some((slope, [slopes[lo_idx], slopes[hi_idx]]))
}

// ---------------------------------------------------------------------------
// PELT
// ---------------------------------------------------------------------------

/// L2 segment cost with prefix sums of centred data.
struct L2Cost {
    s1: Vec<f64>,
    s2: Vec<f64>,
}

impl L2Cost {
    fn new(x: &[f64]) -> Self {
        let c = mean(x);
        let mut s1 = Vec::with_capacity(x.len() + 1);
        let mut s2 = Vec::with_capacity(x.len() + 1);
        s1.push(0.0);
        s2.push(0.0);
        let (mut a, mut b) = (0.0, 0.0);
        for v in x {
            let d = v - c;
            a += d;
            b += d * d;
            s1.push(a);
            s2.push(b);
        }
        Self { s1, s2 }
    }

    /// Sum of squared deviations from the mean over `x[a..b]`.
    fn cost(&self, a: usize, b: usize) -> f64 {
        let k = (b - a) as f64;
        let s = self.s1[b] - self.s1[a];
        ((self.s2[b] - self.s2[a]) - s * s / k).max(0.0)
    }
}

/// PELT optimal segmentation of `x` with penalty `beta` per change and minimum
/// segment length `m` (≥ 1). Returns sorted change positions (segment starts,
/// excluding 0).
///
/// Pruning with a minimum segment length: a candidate `s` beaten at time `t`
/// (`F(s)+C(s,t) > F(t)`) is only provably dominated for `T ≥ t+m` (when `t`
/// itself becomes admissible), so it is retired at `t+m`, not immediately.
fn pelt(x: &[f64], beta: f64, m: usize) -> Vec<usize> {
    let n = x.len();
    let m = m.max(1);
    if n < 2 * m {
        return Vec::new();
    }
    let cost = L2Cost::new(x);
    let mut f = vec![f64::INFINITY; n + 1];
    let mut last = vec![0usize; n + 1];
    f[0] = -beta;
    // (candidate, time at which it was first beaten)
    let mut cands: Vec<(usize, Option<usize>)> = Vec::new();
    for t in m..=n {
        let s_new = t - m;
        if s_new == 0 || s_new >= m {
            cands.push((s_new, None));
        }
        cands.retain(|&(_, beaten)| beaten.is_none_or(|b| t < b + m));
        let mut best = f64::INFINITY;
        let mut arg = 0;
        for &(s, _) in &cands {
            let v = f[s] + cost.cost(s, t) + beta;
            if v < best {
                best = v;
                arg = s;
            }
        }
        f[t] = best;
        last[t] = arg;
        for c in &mut cands {
            if c.1.is_none() && f[c.0] + cost.cost(c.0, t) > best {
                c.1 = Some(t);
            }
        }
    }
    let mut cps = Vec::new();
    let mut t = n;
    while t > 0 {
        let s = last[t];
        if s > 0 {
            cps.push(s);
        }
        t = s;
    }
    cps.reverse();
    cps
}

/// Merge adjacent segments (given by change positions) whose means differ by
/// less than `thr`, smallest gap first, until none remain.
fn merge_close(x: &[f64], mut cps: Vec<usize>, thr: f64) -> Vec<usize> {
    loop {
        let mut bounds = Vec::with_capacity(cps.len() + 2);
        bounds.push(0);
        bounds.extend_from_slice(&cps);
        bounds.push(x.len());
        let means: Vec<f64> = bounds.windows(2).map(|w| mean(&x[w[0]..w[1]])).collect();
        let mut best: Option<(usize, f64)> = None;
        for (i, w) in means.windows(2).enumerate() {
            let gap = (w[1] - w[0]).abs();
            if gap < thr && best.is_none_or(|(_, g)| gap < g) {
                best = Some((i, gap));
            }
        }
        match best {
            Some((i, _)) => {
                cps.remove(i);
            }
            None => return cps,
        }
    }
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// How the series is cut into detection blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    /// Fixed-length blocks from the first day.
    Fixed(usize),
    /// Calendar months fully inside the data.
    Month,
}

impl Scheme {
    /// Nominal block length in days.
    fn nominal(self) -> usize {
        match self {
            Scheme::Fixed(d) => d,
            Scheme::Month => 30,
        }
    }
}

/// Full blocks `(start offset, length)` of `scheme` inside the series. Partial
/// leading/trailing calendar months and a trailing partial fixed block are
/// excluded from detection.
fn make_blocks(
    series: &DailySeries,
    scheme: Scheme,
) -> Result<Vec<(usize, usize)>, ChangePointError> {
    let n = series.daily.len();
    match scheme {
        Scheme::Fixed(bd) => Ok((0..n / bd).map(|b| (b * bd, bd)).collect()),
        Scheme::Month => {
            let mut out = Vec::new();
            let mut d = 0usize;
            while d < n {
                let (bs, be) = bucket_bounds(Granularity::Month, series.start_day + d as i64)?;
                let s = bs - series.start_day;
                let e = (be - series.start_day) as usize;
                if s >= 0 && e <= n {
                    out.push((s as usize, e - s as usize));
                }
                d = e;
            }
            Ok(out)
        }
    }
}

/// Per-day rate of each block (cents/day).
fn block_rates(daily: &[i64], blocks: &[(usize, usize)]) -> Vec<f64> {
    blocks
        .iter()
        .map(|&(s, l)| daily[s..s + l].iter().sum::<i64>() as f64 / l as f64)
        .collect()
}

/// The default detection blocks.
///
/// - `in`: the median gap between nonzero income days rounded up to 7/14/28
///   days (so paydays don't read as level changes).
/// - `net`/`out`: calendar months when the series has ≥ [`SCHEME_MIN_BLOCKS`]
///   full months, else 7 days. (Deviation from 7-day blocks: household outflows
///   are dominated by calendar-anchored bills and bursty discretionary weeks —
///   on the playground's weekly series the noise σ is as large as a doubling of
///   spend, so a sustained 2× escalation was invisible at any sane penalty, while
///   monthly blocks find it at every penalty tried.)
fn default_scheme(series: &DailySeries) -> Result<Scheme, ChangePointError> {
    if series.metric != Metric::In {
        let months = make_blocks(series, Scheme::Month)?.len();
        return Ok(if months >= SCHEME_MIN_BLOCKS {
            Scheme::Month
        } else {
            Scheme::Fixed(7)
        });
    }
    let nonzero: Vec<usize> = series
        .daily
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0)
        .map(|(i, _)| i)
        .collect();
    if nonzero.len() < 2 {
        return Ok(Scheme::Fixed(7));
    }
    let gaps: Vec<f64> = nonzero.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    let g = median(&gaps).ceil();
    Ok(Scheme::Fixed(if g <= 7.0 {
        7
    } else if g <= 14.0 {
        14
    } else {
        28
    }))
}

/// A scheme's noise score (lower = quieter), or `None` with too few blocks.
///
/// - `net`/`out`: σ of the per-day-rate differences × √(block days). White noise
///   scores the same at any block length; a scheme scores lower when it absorbs
///   structure such as paydays or monthly bills.
/// - `in`: the share of consecutive blocks whose *count of income days* differs.
///   Amount-free and immune to one-off spikes, it is ~0 exactly when blocks follow
///   the payday calendar (biweekly pay in 14-day blocks, 1st/15th pay in calendar
///   months) and high when paydays drift across fixed blocks. (Amount-based σ
///   cannot tell 28-day from month blocks for fixed salaries: both are ~0.)
fn scheme_score(series: &DailySeries, scheme: Scheme) -> Result<Option<f64>, ChangePointError> {
    let blocks = make_blocks(series, scheme)?;
    if blocks.len() < SCHEME_MIN_BLOCKS {
        return Ok(None);
    }
    if series.metric == Metric::In {
        let counts: Vec<usize> = blocks
            .iter()
            .map(|&(s, l)| series.daily[s..s + l].iter().filter(|v| **v != 0).count())
            .collect();
        let changed = counts.windows(2).filter(|w| w[0] != w[1]).count();
        return Ok(Some(changed as f64 / (counts.len() - 1) as f64));
    }
    let rates = block_rates(&series.daily, &blocks);
    Ok(Some(diff_sigma(&rates) * (scheme.nominal() as f64).sqrt()))
}

/// Pick the detection blocks: the default ([`default_scheme`]) unless another candidate
/// (7/14/28 days or calendar months) is materially quieter
/// (< [`SCHEME_SWITCH`] × the default's score) — e.g. calendar-anchored paydays
/// (1st/15th) land 0–3 times in any fixed block, which reads as huge noise.
/// Ties among alternatives go to the shorter block (more resolution).
fn choose_scheme(series: &DailySeries) -> Result<Scheme, ChangePointError> {
    let default = default_scheme(series)?;
    let Some(def_score) = scheme_score(series, default)? else {
        return Ok(default);
    };
    let mut best = (default, def_score);
    for cand in [
        Scheme::Fixed(7),
        Scheme::Fixed(14),
        Scheme::Fixed(28),
        Scheme::Month,
    ] {
        if cand == default {
            continue;
        }
        if let Some(sc) = scheme_score(series, cand)? {
            if sc < SCHEME_SWITCH * def_score && (best.0 == default || sc < best.1) {
                best = (cand, sc);
            }
        }
    }
    Ok(best.0)
}

/// Run change-point detection and describe the result.
pub fn detect(series: &DailySeries, opts: Options) -> Result<ChangePoints, ChangePointError> {
    let metric = series.metric;
    let daily = &series.daily;
    let n_days = daily.len();
    let scheme = choose_scheme(series)?;
    let nominal = scheme.nominal();
    let min_days = opts.min_days.clamp(7, 365) as usize;
    // At least 2 blocks: a single (Hampel-clipped) block is never a "level".
    let m = min_days.div_ceil(nominal).max(2);

    // 1. Blocks → per-day rates.
    let blocks = make_blocks(series, scheme)?;
    let n_blocks = blocks.len();
    let raw_rates = block_rates(daily, &blocks);

    // 2. Robustify (local winsorization) and estimate σ (cents/day).
    //    Half-window h = m: runs of ≤ m blocks are a minority of the 2m+1 window
    //    and get clipped. A burst of L days can touch ceil(L/block)+1 blocks (a
    //    one-week splurge straddling two weekly blocks), so h = m−1 would let it
    //    pose as an m-block level; a genuine level must persist ≥ m+1 blocks.
    let rates = hampel(&raw_rates, m);
    let sigma = diff_sigma(&rates);

    // 3. Segment (block indices).
    let cps = if sigma > 0.0 && n_blocks >= 2 * m {
        let beta = opts.sensitivity.penalty() * sigma * sigma * (n_blocks as f64).ln();
        let raw = pelt(&rates, beta, m);
        let abs: Vec<f64> = rates.iter().map(|v| v.abs()).collect();
        let thr = (opts.sensitivity.merge() * sigma).max(MATERIALITY * median(&abs));
        merge_close(&rates, raw, thr)
    } else {
        Vec::new()
    };

    // Segments tile [0, n_days): boundaries at block starts; the first segment
    // absorbs any leading partial month, the last any trailing partial block.
    let mut bounds = vec![0];
    bounds.extend_from_slice(&cps);
    bounds.push(n_blocks);
    let n_segs = bounds.len() - 1;
    let seg_day_range = |i: usize| -> (usize, usize) {
        let a = if i == 0 { 0 } else { blocks[bounds[i]].0 };
        let b = if i + 1 == n_segs {
            n_days
        } else {
            blocks[bounds[i + 1]].0
        };
        (a, b)
    };

    // Per-segment level (length-weighted winsorized rate) and 95% CI.
    let mut levels = Vec::with_capacity(n_segs);
    let mut cis = Vec::with_capacity(n_segs);
    for i in 0..n_segs {
        let (lo, hi) = (bounds[i], bounds[i + 1]);
        if lo == hi {
            // No full block at all (series shorter than one block): raw mean.
            let (a, b) = seg_day_range(i);
            let total: i64 = daily[a..b].iter().sum();
            levels.push(total as f64 / (b - a) as f64);
            cis.push(None);
            continue;
        }
        let rs = &rates[lo..hi];
        let days: usize = blocks[lo..hi].iter().map(|b| b.1).sum();
        let level = rs
            .iter()
            .zip(&blocks[lo..hi])
            .map(|(r, b)| r * b.1 as f64)
            .sum::<f64>()
            / days as f64;
        let mut s = MAD_TO_SIGMA * mad(rs);
        if s <= 0.0 {
            s = sigma;
        }
        let half = Z95 * s / (rs.len() as f64).sqrt();
        levels.push(level);
        cis.push(Some([level - half, level + half]));
    }

    // Display buckets.
    let seg_of_day = |d: usize| -> usize {
        (0..n_segs)
            .find(|&i| {
                let (a, b) = seg_day_range(i);
                d >= a && d < b
            })
            .unwrap_or(n_segs - 1)
    };
    let mut buckets = Vec::new();
    let mut d = 0usize;
    while d < n_days {
        let dn = series.start_day + d as i64;
        let (bs, be) = bucket_bounds(opts.bucket, dn)?;
        let cover_end = ((be - series.start_day) as usize).min(n_days);
        let days = cover_end - d;
        let value: i64 = daily[d..cover_end].iter().sum();
        let partial = (days as i64) < be - bs;
        let segment = seg_of_day(d + (days - 1) / 2);
        buckets.push(DisplayBucket {
            start: date_of(bs)?,
            days: days as u32,
            value,
            expected: levels[segment] * days as f64,
            lo: None,
            hi: None,
            partial,
            unusual: None,
            segment,
        });
        d = cover_end;
    }

    // XmR natural limits per segment, from its full buckets' per-day rates.
    let floor = if metric == Metric::Net {
        f64::NEG_INFINITY
    } else {
        0.0
    };
    let mut unusual_counts = vec![(0u32, 0u32); n_segs];
    for (s, counts) in unusual_counts.iter_mut().enumerate() {
        let full: Vec<f64> = buckets
            .iter()
            .filter(|b| b.segment == s && !b.partial)
            .map(|b| b.value as f64 / f64::from(b.days))
            .collect();
        counts.1 = full.len() as u32;
        let Some(hw) = natural_halfwidth(&full) else {
            continue;
        };
        let level = levels[s];
        for b in buckets.iter_mut().filter(|b| b.segment == s) {
            let days = f64::from(b.days);
            let lo = ((level - hw) * days).max(floor);
            let hi = (level + hw) * days;
            b.lo = Some(lo);
            b.hi = Some(hi);
            if !b.partial {
                let v = b.value as f64;
                b.unusual = if v > hi {
                    Some(Unusual::High)
                } else if v < lo {
                    Some(Unusual::Low)
                } else {
                    None
                };
                counts.0 += u32::from(b.unusual.is_some());
            }
        }
    }

    // Segments.
    let mut segments = Vec::with_capacity(n_segs);
    for i in 0..n_segs {
        let (a, b) = seg_day_range(i);
        segments.push(Segment {
            start: series.date_at(a)?,
            end: series.date_at(b - 1)?,
            days: (b - a) as u32,
            level_per_day: levels[i],
            ci95: cis[i],
            unusual_buckets: wilson(unusual_counts[i].0, unusual_counts[i].1),
            neg_weeks: (metric == Metric::Net).then(|| negative_weeks(series, a, b)),
        });
    }

    let mut changes = Vec::with_capacity(n_segs.saturating_sub(1));
    for i in 1..n_segs {
        let delta = levels[i] - levels[i - 1];
        changes.push(Change {
            date: series.date_at(seg_day_range(i).0)?,
            delta_per_day: delta,
            good: metric.good(delta),
        });
    }

    // Trend of the last segment (Theil–Sen over its winsorized block rates,
    // x = block midpoint in days).
    let last = n_segs - 1;
    let (lo, hi) = (bounds[last], bounds[last + 1]);
    let xs: Vec<f64> = blocks[lo..hi]
        .iter()
        .map(|&(s, l)| s as f64 + l as f64 / 2.0)
        .collect();
    let trend = match theil_sen(&xs, &rates[lo..hi]) {
        Some((slope, ci)) => Some(Trend {
            since: series.date_at(seg_day_range(last).0)?,
            level_per_day: levels[last],
            slope,
            slope_ci95: ci,
            direction: if ci[0] > 0.0 {
                Direction::Rising
            } else if ci[1] < 0.0 {
                Direction::Falling
            } else {
                Direction::Flat
            },
        }),
        None => None,
    };

    // Fail closed on a degenerate (flat) series, as for confirmed changes.
    let tentative = if sigma > 0.0 {
        tentative(series, &blocks, &rates, nominal, m, bounds[last])?
    } else {
        None
    };

    Ok(ChangePoints {
        block_days: nominal as u32,
        block_kind: match scheme {
            Scheme::Fixed(_) => BlockKind::Fixed,
            Scheme::Month => BlockKind::CalendarMonth,
        },
        sigma,
        segments,
        changes,
        buckets,
        tentative,
        trend,
    })
}

/// Full Monday-anchored weeks inside day range `[a, b)` with a negative sum.
fn negative_weeks(series: &DailySeries, a: usize, b: usize) -> Proportion {
    let first_dn = series.start_day + a as i64;
    // Offset of the first Monday on/after day `a`.
    let off = (WEEK_ANCHOR - first_dn).rem_euclid(7) as usize;
    let (mut k, mut n) = (0u32, 0u32);
    let mut w = a + off;
    while w + 7 <= b {
        n += 1;
        k += u32::from(series.daily[w..w + 7].iter().sum::<i64>() < 0);
        w += 7;
    }
    wilson(k, n)
}

/// The "current trend" check on the tail too short to confirm as a segment: the
/// last `m−1` full blocks plus any trailing partial days. Reported when the tail
/// spans at least one nominal block, the last segment keeps ≥ max(m, 2) blocks
/// before it, and the tail's per-day mean (winsorized rates for its full blocks,
/// raw for the partial days) lies outside the reference blocks' XmR natural
/// limits scaled to the tail length (`∝ √(block/tail)`).
fn tentative(
    series: &DailySeries,
    blocks: &[(usize, usize)],
    rates: &[f64],
    nominal: usize,
    m: usize,
    last_seg_block: usize,
) -> Result<Option<Tentative>, ChangePointError> {
    let n_days = series.daily.len();
    let n_blocks = blocks.len();
    let tail_block = n_blocks.saturating_sub(m - 1).max(last_seg_block);
    let reference = &rates[last_seg_block..tail_block];
    if reference.len() < m.max(2) {
        return Ok(None);
    }
    let tail_start = match blocks.get(tail_block) {
        Some(b) => b.0,
        None => blocks[n_blocks - 1].0 + blocks[n_blocks - 1].1,
    };
    let tail_len = n_days - tail_start;
    if tail_len < nominal {
        return Ok(None);
    }
    let Some(hw) = natural_halfwidth(reference) else {
        return Ok(None);
    };
    let level = mean(reference);
    let full: f64 = blocks[tail_block..]
        .iter()
        .zip(&rates[tail_block..])
        .map(|(b, r)| r * b.1 as f64)
        .sum();
    let partial_from = blocks[n_blocks - 1].0 + blocks[n_blocks - 1].1;
    let partial: i64 = series.daily[partial_from.max(tail_start)..].iter().sum();
    let tail_mean = (full + partial as f64) / tail_len as f64;
    let hw = hw * (nominal as f64 / tail_len as f64).sqrt();
    let delta = tail_mean - level;
    // Outside the natural limits *and* material (same floor as the merge step).
    if delta.abs() <= hw || delta.abs() < MATERIALITY * level.abs() {
        return Ok(None);
    }
    Ok(Some(Tentative {
        start: series.date_at(tail_start)?,
        days: tail_len as u32,
        level_per_day: tail_mean,
        delta_per_day: delta,
        good: series.metric.good(delta),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use proptest::prelude::*;

    /// Deterministic pseudo-noise (LCG) so tests need no RNG crate state.
    fn noise(seed: u64, n: usize, amp: i64) -> Vec<i64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                ((s >> 33) as i64 % (2 * amp + 1)) - amp
            })
            .collect()
    }

    fn run(daily: Vec<i64>, metric: Metric, opts: Options) -> ChangePoints {
        let s = DailySeries::new(date(2024, 1, 3), daily, metric).unwrap();
        detect(&s, opts).unwrap()
    }

    #[test]
    fn day_numbers_roundtrip_and_anchor() {
        assert_eq!(day_number(date(1970, 1, 1)), 0);
        assert_eq!(day_number(date(1970, 1, 5)), WEEK_ANCHOR);
        assert_eq!(date(1970, 1, 5).weekday(), jiff::civil::Weekday::Monday);
        for d in [date(2000, 2, 29), date(1969, 12, 31), date(2999, 7, 15)] {
            assert_eq!(date_of(day_number(d)).unwrap(), d);
        }
    }

    #[test]
    fn detects_spending_step_up_as_bad() {
        // 20 weeks at ~$50/day then 20 weeks at ~$90/day.
        let mut daily: Vec<i64> = noise(1, 280, 1500).iter().map(|e| 5000 + e).collect();
        for (i, v) in noise(2, 140, 1500).iter().enumerate() {
            daily[140 + i] = 9000 + v;
        }
        let r = run(daily, Metric::Out, Options::default());
        assert_eq!(r.changes.len(), 1, "{:?}", r.changes);
        let c = &r.changes[0];
        assert_eq!(c.date, date(2024, 1, 3) + jiff::Span::new().days(140));
        assert!(c.delta_per_day > 3000.0 && !c.good);
        assert_eq!(r.segments.len(), 2);
        assert!((r.segments[0].level_per_day - 5000.0).abs() < 300.0);
        let ci = r.segments[1].ci95.unwrap();
        assert!(ci[0] < r.segments[1].level_per_day && r.segments[1].level_per_day < ci[1]);
    }

    #[test]
    fn income_drop_is_bad_and_net_rise_is_good() {
        let mut daily = vec![0i64; 364];
        for i in (0..364).step_by(14) {
            daily[i] = if i < 182 { 300_000 } else { 150_000 };
        }
        let r = run(daily, Metric::In, Options::default());
        assert_eq!(r.block_days, 14);
        assert_eq!(r.changes.len(), 1, "{:?}", r.changes);
        assert!(!r.changes[0].good);

        let mut net: Vec<i64> = noise(3, 280, 2000).iter().map(|e| -1000 + e).collect();
        for v in net.iter_mut().skip(140) {
            *v += 4000;
        }
        let r = run(net, Metric::Net, Options::default());
        assert_eq!(r.changes.len(), 1);
        assert!(r.changes[0].good);
        assert!(r.segments.iter().all(|s| s.neg_weeks.is_some()));
    }

    #[test]
    fn steady_noise_has_no_change() {
        let daily: Vec<i64> = noise(7, 700, 3000).iter().map(|e| 6000 + e).collect();
        let r = run(daily, Metric::Out, Options::default());
        assert!(r.changes.is_empty(), "{:?}", r.changes);
        assert_eq!(r.segments.len(), 1);
    }

    #[test]
    fn single_splurge_does_not_create_a_level() {
        let mut daily: Vec<i64> = noise(9, 365, 1500).iter().map(|e| 5000 + e).collect();
        daily[180] += 500_000; // a $5,000 one-off
        for s in [Sensitivity::Standard, Sensitivity::Sensitive] {
            let r = run(
                daily.clone(),
                Metric::Out,
                Options {
                    sensitivity: s,
                    ..Options::default()
                },
            );
            assert!(r.changes.is_empty(), "{s:?}: {:?}", r.changes);
        }
    }

    #[test]
    fn zero_series_fails_closed() {
        let r = run(vec![0; 200], Metric::Out, Options::default());
        assert_eq!(r.sigma, 0.0);
        assert!(r.changes.is_empty() && r.tentative.is_none());
        assert_eq!(r.segments.len(), 1);
    }

    #[test]
    fn short_series_is_one_segment() {
        let r = run(vec![100, 200, 300], Metric::Out, Options::default());
        assert_eq!(r.segments.len(), 1);
        assert_eq!(r.segments[0].level_per_day, 200.0);
        assert!(r.segments[0].ci95.is_none() && r.trend.is_none());
    }

    #[test]
    fn income_block_rounds_up_paydays() {
        let mut monthly = vec![0i64; 400];
        for i in (0..400).step_by(30) {
            monthly[i] = 100;
        }
        let s = DailySeries::new(date(2024, 1, 1), monthly, Metric::In).unwrap();
        assert_eq!(default_scheme(&s).unwrap(), Scheme::Fixed(28));
        let mut weekly = vec![0i64; 400];
        for i in (0..400).step_by(7) {
            weekly[i] = 100;
        }
        let s = DailySeries::new(date(2024, 1, 1), weekly, Metric::In).unwrap();
        assert_eq!(default_scheme(&s).unwrap(), Scheme::Fixed(7));
    }

    /// Pay on the 1st and 15th lands 1–3 times in any fixed block; the engine must
    /// switch to calendar months and then see only the real raise.
    #[test]
    fn semi_monthly_pay_uses_calendar_months() {
        let start = date(2024, 1, 1);
        let n = 3 * 365;
        let daily: Vec<i64> = (0..n)
            .map(|i| {
                let d = date_of(day_number(start) + i as i64).unwrap();
                let pay = if d.year() >= 2025 && d.month() >= 7 || d.year() == 2026 {
                    250_000
                } else {
                    200_000
                };
                if d.day() == 1 || d.day() == 15 {
                    pay
                } else {
                    0
                }
            })
            .collect();
        let s = DailySeries::new(start, daily, Metric::In).unwrap();
        let r = detect(&s, Options::default()).unwrap();
        assert_eq!(r.block_kind, BlockKind::CalendarMonth);
        assert_eq!(r.changes.len(), 1, "{:?}", r.changes);
        assert_eq!(r.changes[0].date, date(2025, 7, 1));
        assert!(r.changes[0].good);
    }

    /// Multi-year spending uses calendar-month blocks and finds a doubling.
    #[test]
    fn long_spending_series_uses_months() {
        let mut daily: Vec<i64> = noise(21, 730, 4000).iter().map(|e| 6000 + e).collect();
        for v in daily.iter_mut().skip(366) {
            *v *= 2;
        }
        let s = DailySeries::new(date(2024, 1, 1), daily, Metric::Out).unwrap();
        let r = detect(&s, Options::default()).unwrap();
        assert_eq!((r.block_kind, r.block_days), (BlockKind::CalendarMonth, 30));
        assert_eq!(r.changes.len(), 1, "{:?}", r.changes);
        assert_eq!(r.changes[0].date, date(2025, 1, 1));
        assert!(!r.changes[0].good);
    }

    #[test]
    fn week_buckets_are_monday_anchored() {
        // 2024-01-03 is a Wednesday: first week bucket starts Monday 2024-01-01.
        let r = run(
            vec![1; 20],
            Metric::Out,
            Options {
                bucket: Granularity::Week,
                ..Options::default()
            },
        );
        let b = &r.buckets;
        assert_eq!(b[0].start, date(2024, 1, 1));
        assert_eq!((b[0].days, b[0].partial), (5, true));
        assert_eq!(b[1].start, date(2024, 1, 8));
        assert_eq!((b[1].days, b[1].partial), (7, false));
        assert_eq!(b.last().unwrap().days, 1);
        let bi = run(
            vec![1; 30],
            Metric::Out,
            Options {
                bucket: Granularity::Biweek,
                ..Options::default()
            },
        );
        // The first biweek is anchor-aligned and contains the first data day.
        let first = day_number(bi.buckets[0].start);
        assert_eq!((first - WEEK_ANCHOR).rem_euclid(14), 0);
        assert!(first <= day_number(date(2024, 1, 3)));
        assert!(first + 14 > day_number(date(2024, 1, 3)));
    }

    #[test]
    fn month_buckets_are_calendar_months() {
        let r = run(vec![2; 60], Metric::Out, Options::default());
        let b = &r.buckets;
        assert_eq!(b[0].start, date(2024, 1, 1));
        assert_eq!((b[0].days, b[0].value, b[0].partial), (29, 58, true));
        assert_eq!(b[1].start, date(2024, 2, 1));
        assert_eq!((b[1].days, b[1].partial), (29, false)); // leap February
        assert_eq!(b[2].start, date(2024, 3, 1));
        assert!(b.iter().all(|x| x.unusual.is_none() || !x.partial));
    }

    #[test]
    fn flags_unusual_full_buckets_only() {
        let mut daily: Vec<i64> = noise(11, 364, 500).iter().map(|e| 5000 + e).collect();
        for v in &mut daily[100..107] {
            *v += 20_000;
        }
        let r = run(
            daily,
            Metric::Out,
            Options {
                bucket: Granularity::Week,
                ..Options::default()
            },
        );
        let high: Vec<_> = r
            .buckets
            .iter()
            .filter(|b| b.unusual == Some(Unusual::High))
            .collect();
        assert!(!high.is_empty());
        assert!(r
            .buckets
            .iter()
            .all(|b| !(b.partial && b.unusual.is_some())));
        assert!(r.segments[0].unusual_buckets.k >= 1);
    }

    #[test]
    fn tentative_spots_a_forming_level() {
        let mut daily: Vec<i64> = noise(13, 300, 300).iter().map(|e| 5000 + e).collect();
        // Last 10 days jump to ~$200/day: too short to confirm (min 14 days).
        for v in daily.iter_mut().skip(290) {
            *v = 20_000;
        }
        let r = run(daily, Metric::Out, Options::default());
        let t = r.tentative.expect("tentative level");
        assert!(t.delta_per_day > 0.0 && !t.good);
        assert!(t.days >= 7 && t.days < 14);
    }

    #[test]
    fn trend_detects_a_ramp() {
        let daily: Vec<i64> = (0..364).map(|i| 3000 + 10 * i).collect();
        // A smooth ramp may be cut into steps; whatever the last segment, its
        // trend must not be falling.
        let r = run(daily, Metric::Out, Options::default());
        let t = r.trend.expect("trend");
        assert_ne!(t.direction, Direction::Falling);
        let (slope, ci) =
            theil_sen(&[0.0, 1.0, 2.0, 3.0, 4.0], &[1.0, 3.0, 5.0, 7.0, 9.0]).unwrap();
        assert_eq!(slope, 2.0);
        assert!(ci[0] <= 2.0 && ci[1] >= 2.0);
    }

    #[test]
    fn wilson_known_values() {
        let p = wilson(0, 10);
        assert_eq!(p.lo, 0.0);
        assert!((p.hi - 0.2775).abs() < 1e-3);
        let p = wilson(5, 10);
        assert!((p.lo - 0.2366).abs() < 1e-3 && (p.hi - 0.7634).abs() < 1e-3);
        assert_eq!(wilson(0, 0).hi, 1.0);
    }

    #[test]
    fn request_validation() {
        let bad = |j: &str| {
            serde_json::from_str::<Request>(j)
                .unwrap()
                .run()
                .unwrap_err()
        };
        assert!(matches!(
            bad(r#"{"start":"2024-13-01","daily":[1],"metric":"out"}"#),
            ChangePointError::InvalidDate(_)
        ));
        assert_eq!(
            bad(r#"{"start":"2024-01-01","daily":[],"metric":"out"}"#),
            ChangePointError::Empty
        );
        assert!(matches!(
            bad(r#"{"start":"2024-01-01","daily":[-5],"metric":"in"}"#),
            ChangePointError::Negative { .. }
        ));
        assert!(matches!(
            bad(r#"{"start":"9999-12-30","daily":[1,2,3],"metric":"net"}"#),
            ChangePointError::DateRange
        ));
        let long = format!(
            r#"{{"start":"2024-01-01","daily":[{}],"metric":"net"}}"#,
            vec!["0"; MAX_DAYS + 1].join(",")
        );
        assert!(matches!(bad(&long), ChangePointError::TooLong { .. }));
        // min_days is clamped, not rejected.
        let ok: Request = serde_json::from_str(
            r#"{"start":"2024-01-01","daily":[1,2,3],"metric":"net","min_days":-3,"bucket":"day"}"#,
        )
        .unwrap();
        assert_eq!(ok.run().unwrap().buckets.len(), 3);
        assert!(
            serde_json::from_str::<Request>(r#"{"start":"x","daily":[],"metric":"up"}"#).is_err()
        );
    }

    /// Exact optimal partitioning (no pruning) — the PELT oracle.
    fn optimal_partition(x: &[f64], beta: f64, m: usize) -> Vec<usize> {
        let n = x.len();
        if n < 2 * m {
            return Vec::new();
        }
        let cost = L2Cost::new(x);
        let mut f = vec![f64::INFINITY; n + 1];
        let mut last = vec![0; n + 1];
        f[0] = -beta;
        for t in m..=n {
            for s in (0..=t - m).filter(|&s| s == 0 || s >= m) {
                let v = f[s] + cost.cost(s, t) + beta;
                if v < f[t] {
                    f[t] = v;
                    last[t] = s;
                }
            }
        }
        let mut cps = Vec::new();
        let mut t = n;
        while t > 0 {
            if last[t] > 0 {
                cps.push(last[t]);
            }
            t = last[t];
        }
        cps.reverse();
        cps
    }

    proptest! {
        #[test]
        fn pelt_matches_optimal_partitioning(
            x in prop::collection::vec(-1000i32..1000, 0..60),
            m in 1usize..5,
            beta in 1.0f64..2_000_000.0,
        ) {
            // Integer-valued input with a non-integer penalty: ties are improbable.
            let x: Vec<f64> = x.into_iter().map(f64::from).collect();
            prop_assert_eq!(pelt(&x, beta, m), optimal_partition(&x, beta, m));
        }

        #[test]
        fn output_invariants_hold(
            daily in prop::collection::vec(0i64..50_000, 1..800),
            metric in prop_oneof![Just(Metric::Net), Just(Metric::In), Just(Metric::Out)],
            sensitive in any::<bool>(),
            min_days in -10i64..500,
            g in prop_oneof![
                Just(Granularity::Day), Just(Granularity::Week),
                Just(Granularity::Biweek), Just(Granularity::Month)
            ],
            start_off in 0i64..3000,
        ) {
            let start = date_of(day_number(date(2020, 1, 1)) + start_off).unwrap();
            let total: i64 = daily.iter().sum();
            let n = daily.len();
            let s = DailySeries::new(start, daily, metric).unwrap();
            let opts = Options {
                sensitivity: if sensitive { Sensitivity::Sensitive } else { Sensitivity::Standard },
                min_days,
                bucket: g,
            };
            let r = detect(&s, opts).unwrap();
            // Deterministic.
            prop_assert_eq!(&r, &detect(&s, opts).unwrap());
            // Segments tile the series exactly.
            prop_assert_eq!(r.segments[0].start, start);
            prop_assert_eq!(day_number(r.segments.last().unwrap().end) - day_number(start) + 1, n as i64);
            prop_assert_eq!(r.segments.iter().map(|x| x.days as usize).sum::<usize>(), n);
            for w in r.segments.windows(2) {
                prop_assert_eq!(day_number(w[0].end) + 1, day_number(w[1].start));
            }
            prop_assert_eq!(r.changes.len(), r.segments.len() - 1);
            // Buckets partition the data exactly; only the ends may be partial.
            prop_assert_eq!(r.buckets.iter().map(|b| b.value).sum::<i64>(), total);
            prop_assert_eq!(r.buckets.iter().map(|b| b.days as usize).sum::<usize>(), n);
            let nb = r.buckets.len();
            for (i, b) in r.buckets.iter().enumerate() {
                prop_assert!(!b.partial || i == 0 || i + 1 == nb);
                prop_assert!(!(b.partial && b.unusual.is_some()));
                prop_assert!(b.segment < r.segments.len());
                prop_assert!(b.expected.is_finite());
            }
            for seg in &r.segments {
                let p = seg.unusual_buckets;
                prop_assert!(p.k <= p.n && p.lo >= 0.0 && p.hi <= 1.0 && p.lo <= p.hi);
                prop_assert!(seg.level_per_day.is_finite());
            }
            // No NaN/inf leaks (serde_json would silently emit them as null).
            prop_assert!(r.sigma.is_finite());
            for b in &r.buckets {
                prop_assert!(b.lo.is_none_or(f64::is_finite) && b.hi.is_none_or(f64::is_finite));
            }
            for seg in &r.segments {
                prop_assert!(seg.ci95.is_none_or(|c| c[0].is_finite() && c[1].is_finite()));
            }
        }

        #[test]
        fn wilson_brackets_the_estimate(n in 1u32..10_000, k_frac in 0.0f64..=1.0) {
            let k = (f64::from(n) * k_frac).floor() as u32;
            let p = wilson(k, n);
            let phat = f64::from(k) / f64::from(n);
            prop_assert!(p.lo <= phat + 1e-12 && phat <= p.hi + 1e-12);
            prop_assert!(p.lo >= 0.0 && p.hi <= 1.0);
        }
    }
}
