//! Lightweight internationalization: a [`Locale`] (language tag + writing
//! direction + number separators + date/time conventions), a message
//! [`Catalog`] (key → string with `{name}` interpolation), and locale-aware
//! number/currency/date/time formatting with CLDR-lite plural categories.
//! No ICU or heavy data — enough to localize a fenestra app's strings,
//! numbers, and dates and to pick a [`WritingDir`](crate::WritingDir) for the
//! theme. Translated *names* (months, days) are English by default; apps
//! override them per locale via the builders, typically fed from a
//! [`Catalog`].
//!
//! ```
//! use fenestra_core::{Catalog, Locale};
//!
//! let ar = Locale::new("ar");
//! assert!(ar.is_rtl());
//! assert_eq!(Locale::new("en-US").format_int(1_234_567), "1,234,567");
//! assert_eq!(Locale::new("de").format_currency(1234.5, "EUR"), "1.234,50 €");
//! assert_eq!(Locale::new("en").format_date(2026, 8, 25), "8/25/2026");
//! assert_eq!(Locale::new("en").format_time(14, 30, None), "2:30 PM");
//! assert_eq!(Locale::new("en").plural(1.0), fenestra_core::PluralCategory::One);
//!
//! let mut cat = Catalog::new();
//! cat.insert("greeting", "Hello, {name}!");
//! assert_eq!(cat.t("greeting", &[("name", "Ada")]), "Hello, Ada!");
//! assert_eq!(cat.t("missing", &[]), "missing"); // falls back to the key
//! ```

use std::collections::HashMap;

use crate::theme::WritingDir;

/// The calendar-day order a locale renders dates in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateOrder {
    /// Day first (`25/8/2026`) — most of the world.
    #[default]
    Dmy,
    /// Month first (`8/25/2026`) — the US default.
    Mdy,
    /// Year first, zero-padded ISO style (`2026-08-25`) — East Asia, ISO.
    Ymd,
}

/// The CLDR plural category a count falls into for this locale. Apps pick the
/// message form with it: `match loc.plural(n) { One => "1 photo", _ => "n
/// photos" }` — the categories cover every language's rule shape without
/// embedding per-language message data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralCategory {
    /// For languages (Arabic) where 0 gets its own word form.
    Zero,
    /// The singular form (`1 photo`, `1 foto`).
    One,
    /// For languages (Arabic) where 2 gets its own word form.
    Two,
    /// Paucal (`2–4 fotografie` in Czech, `2–4 фото` in Russian).
    Few,
    /// The "many" form Arabic and Slavic languages use for larger counts.
    Many,
    /// The catch-all form every language has.
    Other,
}

/// A locale: a BCP-47-ish language tag plus the writing direction, the
/// decimal / grouping separators used to format numbers, and the date/time
/// conventions (day order, 12/24-hour clock, first day of week). Construct
/// with [`Locale::new`] (which infers everything from the tag) or tune any
/// part with the builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    tag: String,
    primary: String,
    rtl: bool,
    decimal: char,
    grouping: char,
    date_order: DateOrder,
    time_24h: bool,
    first_day: u8,
    currency_suffix: bool,
    months: Option<Vec<String>>,
    days: Option<Vec<String>>,
}

/// Currency symbols this module knows. Unknown codes fall back to the code
/// itself (`1.234,50 XYZ`), which stays honest for exotic currencies.
const CURRENCY_SYMBOLS: &[(&str, &str)] = &[
    ("USD", "$"),
    ("EUR", "€"),
    ("GBP", "£"),
    ("JPY", "¥"),
    ("CNY", "¥"),
    ("KRW", "₩"),
    ("INR", "₹"),
    ("RUB", "₽"),
    ("CHF", "CHF"),
];

/// English month names — the default when a locale carries no overrides.
const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// English short day names, Monday = 0 — the default when a locale carries
/// no overrides.
const EN_DAYS_SHORT: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

impl Locale {
    /// Builds a locale from a language tag (`"en"`, `"en-US"`, `"ar"`,
    /// `"de-DE"`). The primary subtag decides the writing direction and
    /// sensible defaults for the number separators, date order, clock, first
    /// day of week, and currency-symbol placement. `"en"` leans US
    /// (`Mdy`, 12h, Sunday-first); override with the builders for other
    /// English-speaking regions.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let rtl = matches!(
            primary.as_str(),
            "ar" | "he" | "fa" | "ur" | "ps" | "sd" | "yi" | "dv" | "ckb"
        );
        // Comma-decimal locales (a pragmatic subset): most of continental Europe.
        let comma_decimal = matches!(
            primary.as_str(),
            "de" | "fr"
                | "es"
                | "it"
                | "pt"
                | "nl"
                | "pl"
                | "ru"
                | "tr"
                | "sv"
                | "da"
                | "fi"
                | "cs"
                | "el"
                | "hu"
                | "ro"
                | "uk"
        );
        let (decimal, grouping) = if comma_decimal {
            (',', '.')
        } else {
            ('.', ',')
        };
        let date_order = if matches!(primary.as_str(), "ja" | "zh" | "ko" | "hu" | "mn" | "lt") {
            DateOrder::Ymd
        } else if primary == "en" {
            DateOrder::Mdy
        } else {
            DateOrder::Dmy
        };
        // Most of the world writes 24-hour clocks; the US is the notable 12h
        // holdout among locales this module ships defaults for.
        let time_24h = primary != "en";
        // Sunday-first calendars: the US and East Asia. Elsewhere Monday-first.
        let first_day = if matches!(primary.as_str(), "en" | "ja" | "zh" | "ko") {
            6
        } else {
            0
        };
        Self {
            tag: tag.to_string(),
            primary: primary.clone(),
            rtl,
            decimal,
            grouping,
            date_order,
            time_24h,
            first_day,
            currency_suffix: comma_decimal,
            months: None,
            days: None,
        }
    }

    /// The full language tag this locale was built from.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Whether this locale is written right-to-left.
    #[must_use]
    pub fn is_rtl(&self) -> bool {
        self.rtl
    }

    /// The [`WritingDir`] for this locale — pair with
    /// [`Theme::with_direction`](crate::Theme::with_direction).
    #[must_use]
    pub fn direction(&self) -> WritingDir {
        if self.rtl {
            WritingDir::Rtl
        } else {
            WritingDir::Ltr
        }
    }

    /// Overrides the decimal and grouping separators (e.g. `(',', ' ')`).
    #[must_use]
    pub fn with_separators(mut self, decimal: char, grouping: char) -> Self {
        self.decimal = decimal;
        self.grouping = grouping;
        self
    }

    /// Overrides the date order (`DateOrder::Dmy` for a German-style locale
    /// built from an `"en"` tag, say).
    #[must_use]
    pub fn with_date_order(mut self, order: DateOrder) -> Self {
        self.date_order = order;
        self
    }

    /// Overrides the clock: `true` for 24-hour, `false` for 12-hour AM/PM.
    #[must_use]
    pub fn with_24h(mut self, h24: bool) -> Self {
        self.time_24h = h24;
        self
    }

    /// Overrides the first day of week: `0` = Monday … `6` = Sunday (ISO
    /// order, the same indexing [`Self::day_name_short`] uses).
    #[must_use]
    pub fn with_first_day(mut self, day: u8) -> Self {
        self.first_day = day % 7;
        self
    }

    /// Overrides the month names (January-first, 12 entries). Feed these from
    /// your translations; the English defaults stay until overridden.
    #[must_use]
    pub fn with_month_names(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let v: Vec<String> = names.into_iter().map(Into::into).collect();
        debug_assert!(v.len() == 12, "month names need exactly 12 entries");
        if v.len() == 12 {
            self.months = Some(v);
        }
        self
    }

    /// Overrides the short day names, Monday = 0 (7 entries).
    #[must_use]
    pub fn with_day_names(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let v: Vec<String> = names.into_iter().map(Into::into).collect();
        debug_assert!(v.len() == 7, "day names need exactly 7 entries");
        if v.len() == 7 {
            self.days = Some(v);
        }
        self
    }

    /// The full month name for `month` 1..=12 (`1` → `"January"`), honoring
    /// [`Self::with_month_names`].
    #[must_use]
    pub fn month_name(&self, month: u32) -> &str {
        let idx = (month.clamp(1, 12) - 1) as usize;
        match &self.months {
            Some(v) => v[idx].as_str(),
            None => EN_MONTHS[idx],
        }
    }

    /// The short day name for `weekday` 0..=7, Monday = 0 (`0` → `"Mo"`),
    /// honoring [`Self::with_day_names`]. Values wrap mod 7.
    #[must_use]
    pub fn day_name_short(&self, weekday: u32) -> &str {
        let idx = (weekday % 7) as usize;
        match &self.days {
            Some(v) => v[idx].as_str(),
            None => EN_DAYS_SHORT[idx],
        }
    }

    /// The first day of week: `0` = Monday … `6` = Sunday.
    #[must_use]
    pub fn first_day_of_week(&self) -> u8 {
        self.first_day
    }

    /// Formats an integer with this locale's grouping separator
    /// (`1234567` → `"1,234,567"` in `en`, `"1.234.567"` in `de`).
    #[must_use]
    pub fn format_int(&self, n: i64) -> String {
        let digits = n.unsigned_abs().to_string();
        let grouped = group_digits(&digits, self.grouping);
        if n < 0 {
            format!("-{grouped}")
        } else {
            grouped
        }
    }

    /// Formats a number to `decimals` places with this locale's grouping and
    /// decimal separators (`1234.5, 2` → `"1,234.50"` in `en`, `"1.234,50"` in
    /// `de`). Non-finite values render as `"—"`.
    #[must_use]
    pub fn format_f64(&self, x: f64, decimals: usize) -> String {
        if !x.is_finite() {
            return "—".to_string();
        }
        let sign = if x.is_sign_negative() { "-" } else { "" };
        let s = format!("{:.*}", decimals, x.abs());
        let (int_part, frac_part) = s.split_once('.').unwrap_or((s.as_str(), ""));
        let grouped = group_digits(int_part, self.grouping);
        if frac_part.is_empty() {
            format!("{sign}{grouped}")
        } else {
            format!("{sign}{grouped}{}{frac_part}", self.decimal)
        }
    }

    /// Formats an amount of currency with the symbol placed on the side the
    /// locale convention puts it (`1234.5, "EUR"` → `"€1,234.50"` in `en`,
    /// `"1.234,50 €"` in `de`). JPY and KRW render without decimals; unknown
    /// currency codes stand in for their own symbol. The separator before a
    /// trailing symbol is a non-breaking space.
    #[must_use]
    pub fn format_currency(&self, amount: f64, code: &str) -> String {
        let decimals = if matches!(code, "JPY" | "KRW") { 0 } else { 2 };
        let num = self.format_f64(amount, decimals);
        let symbol = CURRENCY_SYMBOLS
            .iter()
            .find(|(c, _)| *c == code)
            .map_or(code, |(_, s)| *s);
        if self.currency_suffix {
            format!("{num}\u{a0}{symbol}")
        } else {
            format!("{symbol}{num}")
        }
    }

    /// Formats a calendar date in the locale's day order (`(2026, 8, 25)` →
    /// `"8/25/2026"` in `en`, `"25/8/2026"` in `de`, `"2026-08-25"` in `ja`).
    /// The ISO-style year-first form is zero-padded; the others are not.
    #[must_use]
    pub fn format_date(&self, year: i32, month: u32, day: u32) -> String {
        match self.date_order {
            DateOrder::Dmy => format!("{day}/{month}/{year}"),
            DateOrder::Mdy => format!("{month}/{day}/{year}"),
            DateOrder::Ymd => format!("{year:04}-{month:02}-{day:02}"),
        }
    }

    /// Formats a time of day on the locale's clock (`(14, 30, None)` →
    /// `"14:30"` in `de`, `"2:30 PM"` in `en`).
    #[must_use]
    pub fn format_time(&self, hour: u32, minute: u32, second: Option<u32>) -> String {
        let hm = format!("{:02}:{:02}", hour % 24, minute % 60);
        if self.time_24h {
            match second {
                Some(s) => format!("{hm}:{:02}", s % 60),
                None => hm,
            }
        } else {
            let h = hour % 24;
            let h12 = if h == 0 {
                12
            } else if h > 12 {
                h - 12
            } else {
                h
            };
            let ap = if h < 12 { "AM" } else { "PM" };
            match second {
                Some(s) => format!("{h12}:{:02}:{:02} {ap}", minute % 60, s % 60),
                None => format!("{h12}:{:02} {ap}", minute % 60),
            }
        }
    }

    /// The CLDR-lite plural category for `n` in this locale — the pragmatic
    /// subset: Arabic's six forms, Slavic one/few/many, French's 0-and-1
    /// singular, CJK/SE-Asian other-only, and the one/other European
    /// default. Fractions land in `Other` (or `One` for French).
    #[must_use]
    pub fn plural(&self, n: f64) -> PluralCategory {
        let is_int = n.fract() == 0.0 && n.is_finite();
        let i = n.abs() as i64;
        let i10 = i % 10;
        let i100 = i % 100;
        match self.primary.as_str() {
            "ja" | "zh" | "ko" | "th" | "vi" | "id" | "ms" => PluralCategory::Other,
            "fr" => {
                if !is_int || i <= 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
            "ru" | "uk" => {
                if !is_int {
                    PluralCategory::Other
                } else if i10 == 1 && i100 != 11 {
                    PluralCategory::One
                } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                    PluralCategory::Few
                } else {
                    PluralCategory::Many
                }
            }
            "pl" => {
                if !is_int {
                    PluralCategory::Other
                } else if i == 1 {
                    PluralCategory::One
                } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                    PluralCategory::Few
                } else {
                    PluralCategory::Many
                }
            }
            "cs" | "sk" => {
                if !is_int {
                    PluralCategory::Many
                } else if i == 1 {
                    PluralCategory::One
                } else if (2..=4).contains(&i) {
                    PluralCategory::Few
                } else {
                    PluralCategory::Many
                }
            }
            "ar" => {
                if !is_int {
                    PluralCategory::Other
                } else if i == 0 {
                    PluralCategory::Zero
                } else if i == 1 {
                    PluralCategory::One
                } else if i == 2 {
                    PluralCategory::Two
                } else if (3..=10).contains(&i100) {
                    PluralCategory::Few
                } else if (11..=99).contains(&i100) {
                    PluralCategory::Many
                } else {
                    PluralCategory::Other
                }
            }
            _ => {
                if is_int && i == 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
        }
    }
}

/// Inserts `sep` every three digits from the right of a run of digit chars.
fn group_digits(digits: &str, sep: char) -> String {
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// A message catalog: keys mapped to translated strings with `{name}`
/// placeholder interpolation. A missing key falls back to the key itself, so a
/// view never renders blank.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    messages: HashMap<String, String>,
}

impl Catalog {
    /// An empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a catalog from `(key, message)` pairs.
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self {
            messages: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Adds or replaces one message.
    pub fn insert(&mut self, key: impl Into<String>, message: impl Into<String>) {
        self.messages.insert(key.into(), message.into());
    }

    /// The raw message for `key`, or `key` itself when absent.
    #[must_use]
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).map_or(key, String::as_str)
    }

    /// The message for `key` with each `{name}` placeholder replaced by its
    /// `args` value. Unmatched placeholders are left as written; a missing key
    /// falls back to the key (then still interpolated).
    #[must_use]
    pub fn t(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = self.get(key);
        if args.is_empty() || !template.contains('{') {
            return template.to_string();
        }
        let mut out = template.to_string();
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_from_tag() {
        assert!(Locale::new("ar").is_rtl());
        assert!(Locale::new("he-IL").is_rtl());
        assert!(!Locale::new("en-US").is_rtl());
        assert_eq!(Locale::new("fa").direction(), WritingDir::Rtl);
        assert_eq!(Locale::new("ja").direction(), WritingDir::Ltr);
    }

    #[test]
    fn integer_grouping_per_locale() {
        assert_eq!(Locale::new("en").format_int(1_234_567), "1,234,567");
        assert_eq!(Locale::new("de").format_int(1_234_567), "1.234.567");
        assert_eq!(Locale::new("en").format_int(-12), "-12");
        assert_eq!(Locale::new("en").format_int(0), "0");
        assert_eq!(Locale::new("en").format_int(999), "999");
    }

    #[test]
    fn decimal_formatting_per_locale() {
        assert_eq!(Locale::new("en").format_f64(1234.5, 2), "1,234.50");
        assert_eq!(Locale::new("de").format_f64(1234.5, 2), "1.234,50");
        assert_eq!(Locale::new("en").format_f64(-0.5, 1), "-0.5");
        assert_eq!(Locale::new("en").format_f64(42.0, 0), "42");
        assert_eq!(Locale::new("en").format_f64(f64::NAN, 2), "—");
    }

    #[test]
    fn separators_override() {
        let fr = Locale::new("fr").with_separators(',', ' ');
        assert_eq!(fr.format_f64(1234.5, 2), "1 234,50");
    }

    #[test]
    fn currency_per_locale() {
        assert_eq!(
            Locale::new("en").format_currency(1234.5, "USD"),
            "$1,234.50"
        );
        assert_eq!(
            Locale::new("de").format_currency(1234.5, "EUR"),
            "1.234,50 €"
        );
        assert_eq!(Locale::new("en").format_currency(9.0, "JPY"), "¥9");
        // Unknown codes stand in for their own symbol; placement stays
        // locale-driven (en prefixes, de suffixes).
        assert_eq!(Locale::new("en").format_currency(1.0, "XYZ"), "XYZ1.00");
        assert_eq!(Locale::new("de").format_currency(1.0, "XYZ"), "1,00 XYZ");
        // Negative amounts keep the sign on the number.
        assert_eq!(Locale::new("en").format_currency(-12.5, "USD"), "$-12.50");
    }

    #[test]
    fn date_orders_per_locale() {
        assert_eq!(Locale::new("en").format_date(2026, 8, 25), "8/25/2026");
        assert_eq!(Locale::new("de").format_date(2026, 8, 25), "25/8/2026");
        assert_eq!(Locale::new("ja").format_date(2026, 8, 25), "2026-08-25");
        // Builders override the inferred order.
        assert_eq!(
            Locale::new("en")
                .with_date_order(DateOrder::Dmy)
                .format_date(2026, 8, 25),
            "25/8/2026"
        );
    }

    #[test]
    fn time_clocks_per_locale() {
        assert_eq!(Locale::new("de").format_time(14, 30, None), "14:30");
        assert_eq!(Locale::new("de").format_time(9, 5, Some(7)), "09:05:07");
        assert_eq!(Locale::new("en").format_time(14, 30, None), "2:30 PM");
        assert_eq!(Locale::new("en").format_time(0, 15, None), "12:15 AM");
        assert_eq!(Locale::new("en").format_time(12, 0, None), "12:00 PM");
        // Builders flip the clock either way.
        assert_eq!(
            Locale::new("de").with_24h(false).format_time(14, 30, None),
            "2:30 PM"
        );
    }

    #[test]
    fn plural_categories_per_locale() {
        use PluralCategory as P;
        let en = Locale::new("en");
        assert_eq!(en.plural(1.0), P::One);
        assert_eq!(en.plural(2.0), P::Other);
        assert_eq!(en.plural(1.5), P::Other);

        let ru = Locale::new("ru");
        assert_eq!(ru.plural(1.0), P::One);
        assert_eq!(ru.plural(2.0), P::Few);
        assert_eq!(ru.plural(5.0), P::Many);
        assert_eq!(ru.plural(11.0), P::Many);
        assert_eq!(ru.plural(21.0), P::One);
        assert_eq!(ru.plural(111.0), P::Many);

        let ar = Locale::new("ar");
        assert_eq!(ar.plural(0.0), P::Zero);
        assert_eq!(ar.plural(1.0), P::One);
        assert_eq!(ar.plural(2.0), P::Two);
        assert_eq!(ar.plural(5.0), P::Few);
        assert_eq!(ar.plural(50.0), P::Many);
        assert_eq!(ar.plural(100.0), P::Other);

        let ja = Locale::new("ja");
        assert_eq!(ja.plural(1.0), P::Other);
        assert_eq!(ja.plural(100.0), P::Other);

        let fr = Locale::new("fr");
        assert_eq!(fr.plural(0.0), P::One);
        assert_eq!(fr.plural(1.0), P::One);
        assert_eq!(fr.plural(2.0), P::Other);
    }

    #[test]
    fn month_and_day_names_with_overrides() {
        let en = Locale::new("en");
        assert_eq!(en.month_name(1), "January");
        assert_eq!(en.month_name(12), "December");
        // Out-of-range months clamp instead of panicking.
        assert_eq!(en.month_name(0), "January");
        assert_eq!(en.day_name_short(0), "Mo");
        assert_eq!(en.day_name_short(6), "Su");
        assert_eq!(en.day_name_short(7), "Mo"); // wraps

        let de = Locale::new("de").with_month_names([
            "Januar",
            "Februar",
            "März",
            "April",
            "Mai",
            "Juni",
            "Juli",
            "August",
            "September",
            "Oktober",
            "November",
            "Dezember",
        ]);
        assert_eq!(de.month_name(3), "März");

        let sunday_first = Locale::new("en").with_first_day(6);
        assert_eq!(sunday_first.first_day_of_week(), 6);
    }

    #[test]
    fn catalog_interpolates_and_falls_back() {
        let cat = Catalog::from_pairs([("hi", "Hello, {name}!"), ("bye", "Goodbye")]);
        assert_eq!(cat.t("hi", &[("name", "Ada")]), "Hello, Ada!");
        assert_eq!(cat.t("bye", &[]), "Goodbye");
        // Missing key falls back to the key itself.
        assert_eq!(cat.t("unknown", &[]), "unknown");
        // Unmatched placeholder is left intact.
        assert_eq!(cat.t("hi", &[]), "Hello, {name}!");
    }
}
