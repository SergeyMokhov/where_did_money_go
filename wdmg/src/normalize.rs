//! Merchant-string normalization.
//!
//! Turns a raw description into a deterministic, uppercased merchant key used for
//! categorization and dedup. Pure & deterministic so dedup keys are stable across
//! runs (per memory: dedup key stability is property-tested).
//!
//! Pipeline:
//! 1. strip a configured code prefix (e.g. `[CW]`) via the profile regex,
//! 2. trim trailing fixed-width whitespace,
//! 3. collapse internal whitespace to single spaces,
//! 4. uppercase,
//! 5. strip trailing location/store tails: ` <CITY> <2-letter PROV>`, `*<token>`,
//!    `#<digits>`, and trailing bare store numbers.

use std::sync::OnceLock;

use regex::Regex;

use crate::config::CompiledProfile;

/// Lazily-compiled tail-stripping regexes. These are static patterns (not user
/// config) so compiling them once and reusing is safe and fast.
struct TailRegexes {
    /// `*TOKEN` suffix (e.g. `AMAZON*SK6KP45A3` -> `AMAZON`).
    star_token: Regex,
    /// `#12345` suffix (e.g. `MCDONALD'S #40515` -> `MCDONALD'S`).
    hash_number: Regex,
    /// Trailing ` CITY PROV` where PROV is a 2-letter Canadian province code.
    city_prov: Regex,
    /// Trailing bare store number when preceded by a space (e.g. `... C81432`).
    trailing_store_number: Regex,
    /// A long (account-like) digit run jammed against a *following* letter
    /// (`6881530CANADA` -> `6881530 CANADA`).
    digits_then_letter: Regex,
    /// A long digit run jammed against a *preceding* letter (`ABC6881530`).
    letter_then_digits: Regex,
}

fn tail_regexes() -> &'static TailRegexes {
    static RE: OnceLock<TailRegexes> = OnceLock::new();
    RE.get_or_init(|| {
        // INVARIANT: these literals are valid regexes, exercised by unit tests;
        // expect() here is on a provably-correct constant pattern.
        TailRegexes {
            star_token: Regex::new(r"\*[A-Z0-9]+\s*$").expect("static regex"),
            hash_number: Regex::new(r"\s*#\d+\s*$").expect("static regex"),
            // Strip a trailing ` <CITY> <PROV>` tail. CITY is a single token so we
            // never swallow the merchant name itself (e.g. "TIM HORTONS NEWNEWYORK ON"
            // must keep "TIM HORTONS", stripping only "NEWNEWYORK ON"). Under-stripping
            // a rare multi-word city is the safe failure direction.
            city_prov: Regex::new(
                r"\s+[A-Z][A-Z.'-]*\s+(?:AB|BC|MB|NB|NL|NS|NT|NU|ON|PE|QC|SK|YT)\s*$",
            )
            .expect("static regex"),
            // A trailing alphanumeric store code like `C81432` after the merchant.
            trailing_store_number: Regex::new(r"\s+[A-Z]?\d{3,}\s*$").expect("static regex"),
            // Account-number boundary normalization (>=5 digits, so years/small
            // codes are untouched): a biller account number sometimes appears
            // jammed against an adjacent word and sometimes spaced, e.g.
            // "...TO 6881530CANADA" vs "...TO 6881530 CANADA" — the SAME payee.
            // Inserting a boundary space collapses both to one merchant. The
            // number is kept (so account-number category rules still match), only
            // re-spaced. Applied AFTER the tail strips so it can't defeat the
            // trailing-store-number strip.
            digits_then_letter: Regex::new(r"(\d{5,})([A-Z])").expect("static regex"),
            letter_then_digits: Regex::new(r"([A-Z])(\d{5,})").expect("static regex"),
        }
    })
}

/// Normalize a raw description into a merchant key.
///
/// `profile` supplies the optional code-prefix strip and trim mode. The remaining
/// steps are universal and deterministic.
#[must_use]
pub fn normalize_merchant(raw: &str, profile: &CompiledProfile) -> String {
    let mut s = raw.to_string();

    // 1. Strip configured code prefix (e.g. "[CW]").
    if let Some(re) = &profile.strip_codes {
        s = re.replace(&s, "").into_owned();
    }

    // 2. Trim trailing whitespace (fixed-width padding) when requested, else trim both ends.
    s = s.trim().to_string();

    // 3. Collapse internal whitespace.
    s = collapse_ws(&s);

    // 4. Uppercase.
    s = s.to_uppercase();

    // 5. Strip trailing location/store tails. Order matters: city+prov before bare
    //    store numbers, then star/hash tokens.
    let re = tail_regexes();
    s = re.city_prov.replace(&s, "").into_owned();
    s = re.trailing_store_number.replace(&s, "").into_owned();
    s = re.star_token.replace(&s, "").into_owned();
    s = re.hash_number.replace(&s, "").into_owned();

    // 6. Re-space account-like digit runs jammed against letters, so the same
    //    payee written with/without a separating space collapses to one merchant.
    s = re.digits_then_letter.replace_all(&s, "$1 $2").into_owned();
    s = re.letter_then_digits.replace_all(&s, "$1 $2").into_owned();
    // The boundary insertion can expose a now-trailing store number (e.g.
    // "STORE 12345"); collapse any double spaces it introduced.
    s = collapse_ws(&s);

    s.trim().to_string()
}

/// Collapse runs of any whitespace into single ASCII spaces.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn profile(id: &str) -> CompiledProfile {
        Config::from_defaults()
            .unwrap()
            .profiles
            .into_iter()
            .find(|p| p.profile.id == id)
            .unwrap()
    }

    #[test]
    fn strips_city_and_province() {
        let p = profile("bank_ledger");
        assert_eq!(
            normalize_merchant("SHELL C81432 NEWNEWYORK ON", &p),
            "SHELL"
        );
    }

    #[test]
    fn strips_star_token() {
        let p = profile("bank_ledger");
        assert_eq!(normalize_merchant("Amazon*SK6KP45A3", &p), "AMAZON");
    }

    #[test]
    fn strips_hash_number_keeps_apostrophe() {
        let p = profile("bank_ledger");
        assert_eq!(
            normalize_merchant("MCDONALD'S #40515 NEWNEWYORK ON", &p),
            "MCDONALD'S"
        );
    }

    #[test]
    fn strips_code_prefix_and_trims_fixed_width() {
        let p = profile("first_bank_card");
        let raw = "[CW]INTERAC ETRNSFR SENT     FLOWERS                  20260011519VKD69W        ";
        let out = normalize_merchant(raw, &p);
        assert!(
            out.starts_with("INTERAC ETRNSFR SENT FLOWERS"),
            "got {out:?}"
        );
        assert!(!out.contains("[CW]"));
        // Internal runs collapsed.
        assert!(!out.contains("  "));
    }

    #[test]
    fn account_number_spacing_variants_collapse() {
        // The same biller sometimes appears jammed ("6881530CANADA") and
        // sometimes spaced ("6881530 CANADA") — both must normalize identically,
        // and the account number is preserved (not stripped) so rules can match.
        let p = profile("bank_ledger");
        let a = normalize_merchant("EFT Withdrawal to 6881530CANADA", &p);
        let b = normalize_merchant("EFT Withdrawal to 6881530 CANADA", &p);
        assert_eq!(a, b, "spacing variants must collapse");
        assert_eq!(a, "EFT WITHDRAWAL TO 6881530 CANADA");
    }

    #[test]
    fn short_digit_runs_untouched() {
        // <5-digit runs (years, small codes) are not re-spaced (e.g. 1PASSWORD).
        let p = profile("bank_ledger");
        assert_eq!(normalize_merchant("1PASSWORD", &p), "1PASSWORD");
    }

    #[test]
    fn deterministic() {
        let p = profile("bank_ledger");
        let raw = "SHELL C81432 NEWNEWYORK ON";
        assert_eq!(normalize_merchant(raw, &p), normalize_merchant(raw, &p));
    }
}
