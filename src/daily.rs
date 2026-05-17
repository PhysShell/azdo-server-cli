//! Pure date/path core for the `daily` wiki workflow.
//!
//! The `daily` workflow creates today's standup page at
//! `{root}/{yyyy}/Q{n}/{dd.MM.yyyy}` seeded with the content of the most
//! recent existing page. Two facts are computed *independently* here:
//!
//! - the **target** path — a pure function of today's date alone, so a new
//!   quarter or year folder is never a special case: it simply falls out of
//!   `quarter(today)` / `today.year`;
//! - the **source** — [`pick_latest`] scans every existing page path and
//!   returns the one whose `dd.MM.yyyy` leaf is the chronological maximum,
//!   **ordered by date, not lexically** (`01.01.2026` is later than
//!   `31.12.2025`, which a string compare gets backwards).
//!
//! This invariant-bearing logic lives in Rust precisely so property-based
//! tests can pin it; the workflow *shape* (sequence, the don't-overwrite
//! guard, the messages) stays in the editable `.rhai` script. Nothing here
//! touches the network or the `Ops` seam.

#![allow(
    dead_code,
    reason = "the daily date/path core lands ahead of its S9 wiki consumer"
)]

/// A proleptic-Gregorian calendar date. `derive`d ordering compares
/// `(y, m, d)` in that field order, which equals chronological order for the
/// normalised dates this module produces — that is what makes
/// [`pick_latest`] correct across quarter and year boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Ymd {
    pub(crate) y: i32,
    pub(crate) m: u32,
    pub(crate) d: u32,
}

/// Gregorian leap-year rule.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "modulo by non-zero integer literals cannot panic or wrap"
)]
const fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in `m` (1..=12) for year `y`; `0` for an out-of-range month so
/// callers that forget to validate get a rejecting answer, not a panic.
const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Calendar quarter (1..=4) for month `m` (1..=12); months outside that
/// range are clamped into `Q4`, which never matters because every `Ymd`
/// reaching this function has been validated.
pub(crate) const fn quarter(m: u32) -> u32 {
    match m {
        1..=3 => 1,
        4..=6 => 2,
        7..=9 => 3,
        _ => 4,
    }
}

/// Serial day number relative to 1970-01-01 (Howard Hinnant's algorithm).
/// Total over the validated domain (`0001..=9999`); the `i64` intermediates
/// cannot overflow for any year that fits in `i32`.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "bounded date domain: i64 intermediates cannot overflow for any i32 year"
)]
fn days_from_civil(date: Ymd) -> i64 {
    let m = i64::from(date.m);
    let d = i64::from(date.d);
    let y = if date.m <= 2 {
        i64::from(date.y) - 1
    } else {
        i64::from(date.y)
    };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]. Not `const` only because `i32`/`u32`
/// `try_from` is not `const` on the pinned toolchain; the conversions
/// cannot actually fail for any input this module feeds it.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "bounded date domain: i64 intermediates cannot overflow for any reachable serial day"
)]
fn civil_from_days(serial: i64) -> Ymd {
    let z = serial + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Ymd {
        y: i32::try_from(y).unwrap_or(0),
        m: u32::try_from(m).unwrap_or(0),
        d: u32::try_from(d).unwrap_or(0),
    }
}

/// Today's date in UTC. UTC (not local) is a deliberate spike simplification:
/// a standup page is created during the working day, far from a midnight
/// boundary where the offset could flip the date; revisiting with a
/// configured offset is a later concern.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "seconds-to-days division is by a non-zero constant"
)]
pub(crate) fn today_utc() -> Ymd {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    civil_from_days(days)
}

/// Render as `dd.MM.yyyy` with zero-padded day/month and a 4-digit year.
/// The format is a contract: [`parse_date`] is its exact inverse over
/// `0001-01-01..=9999-12-31`.
pub(crate) fn fmt_date(date: Ymd) -> String {
    format!("{:02}.{:02}.{:04}", date.d, date.m, date.y)
}

/// Parse a strict `dd.MM.yyyy` string, rejecting anything that is not a
/// real calendar date (wrong field count, non-numeric parts, month outside
/// 1..=12, day outside the month's length, year outside 1..=9999). Returns
/// `None` rather than a best-effort guess so a malformed leaf is simply not
/// considered a candidate by [`pick_latest`].
pub(crate) fn parse_date(s: &str) -> Option<Ymd> {
    let mut parts = s.split('.');
    let d = parts.next()?.parse::<u32>().ok()?;
    let m = parts.next()?.parse::<u32>().ok()?;
    let y = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return None;
    }
    if d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(Ymd { y, m, d })
}

/// The target wiki path for `date` under `root`:
/// `{root}/{year}/Q{quarter}/{dd.MM.yyyy}`. A single trailing `/` on `root`
/// is tolerated so the configured `daily_path` may be written either way.
pub(crate) fn daily_page_path(root: &str, date: Ymd) -> String {
    let root = root.strip_suffix('/').unwrap_or(root);
    format!("{root}/{}/Q{}/{}", date.y, quarter(date.m), fmt_date(date))
}

/// The final `/`-separated segment of `path` (the page's own name).
fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Among `paths`, return the one whose leaf parses as the chronologically
/// **latest** `dd.MM.yyyy`. Paths whose leaf is not a valid date are
/// ignored; `None` means no path carried a usable date. Selection is by
/// parsed date (`Ymd`'s ordering), never by string order, so it is correct
/// across quarter and year rollovers. On the (in practice impossible)
/// tie of two equal dates, the last one encountered wins.
pub(crate) fn pick_latest(paths: &[String]) -> Option<String> {
    paths
        .iter()
        .filter_map(|p| parse_date(leaf(p)).map(|d| (d, p)))
        .max_by_key(|&(d, _)| d)
        .map(|(_, p)| p.clone())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::absolute_paths,
    clippy::arithmetic_side_effects,
    reason = "tests legitimately panic on bad fixtures; proptest macro emits absolute paths"
)]
mod tests {
    use proptest::prelude::*;

    use super::{
        daily_page_path, days_in_month, fmt_date, leaf, parse_date, pick_latest, quarter,
        today_utc, Ymd,
    };

    /// A strategy yielding only real calendar dates with a 4-digit year.
    fn any_ymd() -> impl Strategy<Value = Ymd> {
        (1000_i32..=9999, 1_u32..=12)
            .prop_flat_map(|(y, m)| (Just(y), Just(m), 1_u32..=days_in_month(y, m)))
            .prop_map(|(y, m, d)| Ymd { y, m, d })
    }

    #[test]
    fn quarter_boundaries_are_exact() {
        let expected = [
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 2),
            (5, 2),
            (6, 2),
            (7, 3),
            (8, 3),
            (9, 3),
            (10, 4),
            (11, 4),
            (12, 4),
        ];
        for (m, q) in expected {
            assert_eq!(quarter(m), q, "month {m} must be in Q{q}");
        }
    }

    #[test]
    fn pick_latest_orders_by_date_not_string() {
        // Lexically "01..." < "31...", but 01.01.2026 is *later* than
        // 31.12.2025. A string compare gets this backwards.
        let paths = vec![
            "Daily/2025/Q4/31.12.2025".to_owned(),
            "Daily/2026/Q1/01.01.2026".to_owned(),
        ];
        assert_eq!(
            pick_latest(&paths).as_deref(),
            Some("Daily/2026/Q1/01.01.2026"),
            "the later calendar date wins even though its path sorts first",
        );
    }

    #[test]
    fn pick_latest_crosses_quarter_rollover() {
        // Source is the last page of Q3; today would land in Q4. The two
        // are unrelated and `pick_latest` must still find the Q3 page.
        let paths = vec![
            "D/2026/Q1/05.02.2026".to_owned(),
            "D/2026/Q3/30.09.2026".to_owned(),
            "D/2026/Q2/14.05.2026".to_owned(),
        ];
        assert_eq!(pick_latest(&paths).as_deref(), Some("D/2026/Q3/30.09.2026"),);
    }

    #[test]
    fn pick_latest_ignores_non_date_leaves_and_empty() {
        assert_eq!(pick_latest(&[]), None, "no paths -> no latest");
        let junk = vec![
            "D/2026/Q1".to_owned(),
            "D/readme".to_owned(),
            "D/2026/Q1/notadate".to_owned(),
        ];
        assert_eq!(
            pick_latest(&junk),
            None,
            "leaves that are not dd.MM.yyyy are not candidates",
        );
    }

    #[test]
    fn parse_date_rejects_impossible_dates() {
        assert_eq!(parse_date("29.02.2025"), None, "2025 is not a leap year");
        assert_eq!(
            parse_date("29.02.2024"),
            Some(Ymd {
                y: 2024,
                m: 2,
                d: 29
            }),
            "2024 is a leap year",
        );
        assert_eq!(parse_date("31.04.2026"), None, "April has 30 days");
        assert_eq!(parse_date("00.01.2026"), None, "day 0 is invalid");
        assert_eq!(parse_date("01.13.2026"), None, "month 13 is invalid");
        assert_eq!(parse_date("1.1.2026.5"), None, "too many fields");
        assert_eq!(parse_date("01.01.0000"), None, "year 0 is out of range");
    }

    #[test]
    fn leaf_is_the_last_segment() {
        assert_eq!(leaf("a/b/c"), "c");
        assert_eq!(leaf("solo"), "solo", "a path with no slash is its own leaf");
    }

    #[test]
    fn today_utc_is_a_real_date() {
        let t = today_utc();
        assert!(
            (1970..=9999).contains(&t.y) && (1..=12).contains(&t.m),
            "today must be a sane calendar date, got {t:?}",
        );
        assert!(
            (1..=days_in_month(t.y, t.m)).contains(&t.d),
            "today's day must be valid for its month, got {t:?}",
        );
    }

    proptest! {
        /// The serial-day mapping round-trips for every real date: it is a
        /// total bijection over the validated domain.
        #[test]
        fn civil_serial_roundtrips(date in any_ymd()) {
            let serial = super::days_from_civil(date);
            prop_assert_eq!(
                super::civil_from_days(serial),
                date,
                "civil_from_days . days_from_civil must be identity",
            );
        }

        /// Serial day is strictly monotonic in the calendar order, which is
        /// the property `pick_latest` ultimately relies on.
        #[test]
        fn serial_is_monotonic(a in any_ymd(), b in any_ymd()) {
            let sa = super::days_from_civil(a);
            let sb = super::days_from_civil(b);
            prop_assert_eq!(
                a.cmp(&b),
                sa.cmp(&sb),
                "Ymd order must match serial-day order ({:?} vs {:?})",
                a,
                b,
            );
        }

        /// `fmt_date` and `parse_date` are exact inverses.
        #[test]
        fn fmt_parse_roundtrips(date in any_ymd()) {
            prop_assert_eq!(
                parse_date(&fmt_date(date)),
                Some(date),
                "parse_date . fmt_date must be identity",
            );
        }

        /// The target path always has the documented shape, and its leaf
        /// parses back to the originating date.
        #[test]
        fn daily_path_shape_and_roundtrip(date in any_ymd()) {
            let path = daily_page_path("Daily/Standup", date);
            prop_assert!(
                path.contains(&format!("/{}/Q{}/", date.y, quarter(date.m))),
                "path must carry /year/Qn/, got {path}",
            );
            prop_assert_eq!(
                parse_date(leaf(&path)),
                Some(date),
                "the leaf of a target path must parse back to its date",
            );
        }

        /// `pick_latest` returns the chronological maximum regardless of the
        /// order paths are given in, and regardless of how their strings
        /// sort — exercised over sets that straddle quarter/year boundaries.
        #[test]
        fn pick_latest_is_the_global_max(
            dates in proptest::collection::btree_set(any_ymd(), 1..16)
                .prop_map(|s| s.into_iter().collect::<Vec<_>>())
                .prop_shuffle(),
        ) {
            let expected = dates
                .iter()
                .max()
                .copied()
                .expect("the set has at least one date");
            let paths: Vec<String> = dates
                .iter()
                .map(|d| daily_page_path("Daily", *d))
                .collect();
            prop_assert_eq!(
                pick_latest(&paths),
                Some(daily_page_path("Daily", expected)),
                "pick_latest must return the chronologically newest path",
            );
        }
    }
}
