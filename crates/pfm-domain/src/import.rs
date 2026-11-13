//! CSV import domain types and pure parsing heuristics.
//!
//! This module is intentionally free of I/O, serialization, and framework
//! dependencies (only `std` + `chrono` + `rrule`). All types here can be
//! unit-tested without a database or HTTP server.

use chrono::NaiveDate;

// ─── Enums ──────────────────────────────────────────────────────────────────────

/// Supported date formats for CSV import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    /// `dd/mm/yyyy` (or `dd-mm-yyyy`, `dd.mm.yyyy`).
    Dmy,
    /// `yyyy-mm-dd` (ISO 8601).
    Ymd,
    /// `mm/dd/yyyy` (or `mm-dd-yyyy`, `mm.dd.yyyy`).
    Mdy,
}

impl DateFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dmy => "dmy",
            Self::Ymd => "ymd",
            Self::Mdy => "mdy",
        }
    }

    pub fn try_from(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "dmy" | "dd/mm/yyyy" | "dd-mm-yyyy" => Some(Self::Dmy),
            "ymd" | "yyyy-mm-dd" => Some(Self::Ymd),
            "mdy" | "mm/dd/yyyy" | "mm-dd-yyyy" => Some(Self::Mdy),
            _ => None,
        }
    }

    /// The `str::parse_from_str` format string for `chrono::NaiveDate`.
    fn chrono_fmt(self) -> &'static str {
        match self {
            Self::Dmy => "%d-%m-%Y",
            Self::Ymd => "%Y-%m-%d",
            Self::Mdy => "%m-%d-%Y",
        }
    }
}

/// Decimal separator style used by the bank's CSV export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalSep {
    /// Italian / European style: `1234,56` (comma = decimal, dot = thousands).
    Comma,
    /// US / international style: `1234.56` (dot = decimal, comma = thousands).
    Dot,
}

impl DecimalSep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Comma => ",",
            Self::Dot => ".",
        }
    }

    pub fn try_from(s: &str) -> Option<Self> {
        match s.trim() {
            "," => Some(Self::Comma),
            "." => Some(Self::Dot),
            _ => None,
        }
    }
}

/// The logical role of a CSV column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnRole {
    /// The column holds the transaction date.
    Date,
    /// The column holds the signed amount.
    Amount,
    /// The column holds the free-text description / merchant name.
    Description,
    /// The column is not used by the import.
    Other,
}

impl ColumnRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Amount => "amount",
            Self::Description => "description",
            Self::Other => "other",
        }
    }

    pub fn try_from(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "date" => Some(Self::Date),
            "amount" | "importo" | "valore" => Some(Self::Amount),
            "description" | "descrizione" | "causale" => Some(Self::Description),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

// ─── Structs ───────────────────────────────────────────────────────────────────

/// A single parsed row from a CSV import.
#[derive(Debug, Clone)]
pub struct ImportRow {
    /// The business date of the movement.
    pub date: NaiveDate,
    /// Signed amount in cents (negative = out, positive = in).
    pub cents: i64,
    /// Free-text description (merchant name, etc.).
    pub description: Option<String>,
}

/// A deduplication key: same date + amount + description on the same account.
/// Rows matching an existing row in the database are flagged as duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DedupKey {
    pub date: NaiveDate,
    pub cents: i64,
    pub description: Option<String>,
}

impl DedupKey {
    pub fn from_row(row: &ImportRow) -> Self {
        Self {
            date: row.date,
            cents: row.cents,
            description: row.description.clone(),
        }
    }
}

/// A saved import profile for a specific bank + account.
#[derive(Debug, Clone)]
pub struct ImportProfile {
    pub id: String,
    pub account_id: String,
    pub bank_name: String,
    /// The CSV field delimiter (e.g. `;` or `,`).
    pub delimiter: char,
    pub date_format: DateFormat,
    pub decimal_sep: DecimalSep,
    /// Explicit column mapping (index → role). `None` means auto-detect.
    pub col_map: Option<Vec<ColumnRole>>,
    /// Whether to auto-assign categories (future feature, always `false` now).
    pub auto_cat: bool,
    /// When `true`, duplicate rows (same date+amount+description) are
    /// silently skipped on commit rather than reported.
    pub skip_dup: bool,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
}

// ─── Parsing functions ─────────────────────────────────────────────────────────

/// Parse a date string in the given format. Normalizes `/` and `.` separators
/// to `-` before parsing, so both `dd/mm/yyyy` and `dd-mm-yyyy` work.
pub fn parse_date_str(s: &str, fmt: DateFormat) -> Result<NaiveDate, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty date string".into());
    }
    // Normalize separators: / and . → -
    let normalized: String = s
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect();

    NaiveDate::parse_from_str(&normalized, fmt.chrono_fmt())
        .map_err(|e| format!("invalid date '{s}': {e}"))
}

/// Parse an amount string to signed cents.
///
/// Handles:
/// - Optional leading `-` or `+` sign.
/// - Thousands separator (the *other* character from the decimal separator).
/// - Decimal separator (comma or dot, per the `sep` parameter).
/// - Integer-only amounts (no decimal part → treated as whole units).
///
/// Examples:
/// - `parse_amount_str("45,50", Comma)` → `4550`
/// - `parse_amount_str("-1.234,56", Comma)` → `-123456`
/// - `parse_amount_str("45.50", Dot)` → `4550`
/// - `parse_amount_str("1234", Dot)` → `123400`
pub fn parse_amount_str(s: &str, sep: DecimalSep) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty amount string".into());
    }

    let (dec_char, thou_char) = match sep {
        DecimalSep::Comma => (',', '.'),
        DecimalSep::Dot => ('.', ','),
    };

    // Strip any leading currency symbols (€ $ £ etc.) – keep only digits,
    // separators, and an optional leading sign.
    let cleaned: String = s
        .chars()
        .filter(|c| {
            c.is_ascii_digit() || *c == '-' || *c == '+' || *c == dec_char || *c == thou_char
        })
        .collect();

    if cleaned.is_empty() {
        return Err(format!("no digits in amount '{s}'"));
    }

    // Remove thousands separators.
    let no_thou: String = cleaned.chars().filter(|c| *c != thou_char).collect();

    // Replace decimal separator with '.' for uniform parsing.
    let normalized: String = no_thou
        .chars()
        .map(|c| if c == dec_char { '.' } else { c })
        .collect();

    // Determine sign.
    let (sign, abs_str) = if let Some(rest) = normalized.strip_prefix('-') {
        (-1i64, rest)
    } else if let Some(rest) = normalized.strip_prefix('+') {
        (1i64, rest)
    } else {
        (1i64, normalized.as_str())
    };

    // Split into integer and fractional parts.
    let (int_part, frac_part) = match abs_str.find('.') {
        Some(pos) => (&abs_str[..pos], &abs_str[pos + 1..]),
        None => (abs_str, ""),
    };

    if int_part.is_empty() {
        return Err(format!("missing integer part in '{s}'"));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("invalid integer part '{int_part}' in '{s}'"));
    }

    let int_val: i64 = int_part
        .parse()
        .map_err(|e: std::num::ParseIntError| format!("integer overflow in '{s}': {e}"))?;

    // Fractional part: take at most 2 digits (cents).
    let frac_chars: Vec<char> = frac_part.chars().take(2).collect();
    let frac_str: String = if frac_chars.is_empty() {
        "0".to_string()
    } else {
        let mut f = String::with_capacity(2);
        for c in &frac_chars {
            if !c.is_ascii_digit() {
                return Err(format!("invalid fractional part in '{s}'"));
            }
            f.push(*c);
        }
        while f.len() < 2 {
            f.push('0');
        }
        f
    };

    let frac_val: i64 = frac_str.parse().unwrap_or(0);
    Ok(sign * (int_val * 100 + frac_val))
}

// ─── Column-detection heuristic ────────────────────────────────────────────────

/// Detect column roles by cross-referencing header names **and** the actual
/// content of the first data row.
///
/// The heuristic works in three passes:
///
/// 1. **Header + content**: for each column, if the header name suggests a
///    role (contains "data"/"date", "importo"/"amount", "descrizione"/"description")
///    **and** the first data-row value is parseable in the expected format,
///    confirm the role. This prevents misidentification (e.g. "Data operazione"
///    where "operazione" might suggest a text column, but the content is a date).
///
/// 2. **Content only**: for columns not resolved in pass 1, try to identify
///    the role purely from the content (parseable as date → Date, parseable as
///    number → Amount).
///
/// 3. **Description fallback**: if no description column was identified, assign
///    the role to the first remaining text column that is neither a date nor an
///    amount.
///
/// Returns `None` if the heuristic is not confident (i.e. it could not identify
/// both a date column and an amount column). In that case the caller should fall
/// back to the user's manual `col_map` configuration.
///
/// # Parameters
/// - `headers`: the CSV header row (column names).
/// - `first_row`: the first data row (values).
/// - `date_fmt`: the expected date format for content verification.
pub fn detect_columns(
    headers: &[String],
    first_row: &[String],
    date_fmt: DateFormat,
) -> Option<Vec<ColumnRole>> {
    let n = headers.len().min(first_row.len());
    if n < 2 {
        return None;
    }

    let mut roles = vec![ColumnRole::Other; n];
    let mut has_date = false;
    let mut has_amount = false;
    let mut has_desc = false;

    // Pass 1: header name + content verification.
    for i in 0..n {
        let header = headers[i].to_lowercase();
        let content = first_row[i].trim();

        let content_is_date = parse_date_str(content, date_fmt).is_ok();
        let content_is_amount_raw = parse_amount_str(content, DecimalSep::Comma).is_ok()
            || parse_amount_str(content, DecimalSep::Dot).is_ok();
        // A value that parses as both a date and an amount is a date
        // (e.g. "15/01/2024" strips to "15012024" which is a valid integer).
        let content_is_amount = content_is_amount_raw && !content_is_date;

        let header_is_date =
            header.contains("data") || header.contains("date") || header.contains("giorno");
        let header_is_amount = header.contains("importo")
            || header.contains("amount")
            || header.contains("valore")
            || header.contains("somma");
        let header_is_desc = header.contains("descrizione")
            || header.contains("description")
            || header.contains("causale")
            || header.contains("memo")
            || header.contains("detail");

        if !has_date && header_is_date && content_is_date {
            roles[i] = ColumnRole::Date;
            has_date = true;
        } else if !has_amount && header_is_amount && content_is_amount {
            roles[i] = ColumnRole::Amount;
            has_amount = true;
        } else if !has_desc && header_is_desc && !content_is_date && !content_is_amount {
            roles[i] = ColumnRole::Description;
            has_desc = true;
        }
    }

    // Pass 2: content-only identification for unresolved columns.
    for i in 0..n {
        if roles[i] != ColumnRole::Other {
            continue;
        }
        let content = first_row[i].trim();
        if content.is_empty() {
            continue;
        }
        let content_is_date = parse_date_str(content, date_fmt).is_ok();
        let content_is_amount_raw = parse_amount_str(content, DecimalSep::Comma).is_ok()
            || parse_amount_str(content, DecimalSep::Dot).is_ok();
        let content_is_amount = content_is_amount_raw && !content_is_date;

        if !has_date && content_is_date {
            roles[i] = ColumnRole::Date;
            has_date = true;
        } else if !has_amount && content_is_amount {
            roles[i] = ColumnRole::Amount;
            has_amount = true;
        }
    }

    // Pass 3: description fallback – first remaining non-empty text column.
    if !has_desc {
        let mut found_desc = false;
        for i in 0..n {
            if roles[i] != ColumnRole::Other {
                continue;
            }
            let content = first_row[i].trim();
            if content.is_empty() {
                continue;
            }
            let content_is_date = parse_date_str(content, date_fmt).is_ok();
            let content_is_amount_raw = parse_amount_str(content, DecimalSep::Comma).is_ok()
                || parse_amount_str(content, DecimalSep::Dot).is_ok();
            let content_is_amount = content_is_amount_raw && !content_is_date;
            if !content_is_date && !content_is_amount {
                roles[i] = ColumnRole::Description;
                found_desc = true;
                break;
            }
        }
        let _ = found_desc;
    }

    // We need at minimum a date and an amount column.
    if has_date && has_amount {
        Some(roles)
    } else {
        None
    }
}

/// Parse a full CSV text into `ImportRow`s using the given configuration.
///
/// - `csv_text`: the raw CSV content (header row + data rows).
/// - `delimiter`: the field separator character.
/// - `date_fmt`: the date format for the date column.
/// - `dec_sep`: the decimal separator for amount parsing.
/// - `col_map`: explicit column mapping (index → role). If `None`, the
///   column roles are auto-detected via [`detect_columns`].
///
/// Returns a `Vec<ImportRow>` on success, or an error string if parsing fails
/// (e.g. no header, unparseable date/amount, no date+amount columns found).
pub fn parse_csv(
    csv_text: &str,
    delimiter: char,
    date_fmt: DateFormat,
    dec_sep: DecimalSep,
    col_map: Option<&[ColumnRole]>,
) -> Result<Vec<ImportRow>, String> {
    let lines: Vec<&str> = csv_text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.len() < 2 {
        return Err("CSV must have a header row and at least one data row".into());
    }

    // Split header and data rows.
    let headers: Vec<String> = split_csv_line(lines[0], delimiter);
    let data_lines = &lines[1..];

    // Determine column roles.
    let roles: Vec<ColumnRole> = if let Some(map) = col_map {
        // Explicit mapping: pad with Other if the map is shorter than headers.
        let mut r: Vec<ColumnRole> = map.to_vec();
        while r.len() < headers.len() {
            r.push(ColumnRole::Other);
        }
        r
    } else {
        // Auto-detect: use the first data row for content verification.
        let first_fields: Vec<String> = split_csv_line(data_lines[0], delimiter);
        detect_columns(&headers, &first_fields, date_fmt).ok_or_else(|| {
            "auto-detection failed: could not identify date and amount columns. \
                 Please provide an explicit col_map."
                .to_string()
        })?
    };

    // Find the indices of the required columns.
    let date_idx = roles
        .iter()
        .position(|r| *r == ColumnRole::Date)
        .ok_or("no date column identified")?;
    let amount_idx = roles
        .iter()
        .position(|r| *r == ColumnRole::Amount)
        .ok_or("no amount column identified")?;
    let desc_idx = roles.iter().position(|r| *r == ColumnRole::Description);

    // Parse each data row.
    let mut rows = Vec::with_capacity(data_lines.len());
    for (line_no, line) in data_lines.iter().enumerate() {
        let fields = split_csv_line(line, delimiter);

        // Safety: skip rows with too few fields.
        if fields.len() <= date_idx.max(amount_idx) {
            continue;
        }

        let date = match parse_date_str(&fields[date_idx], date_fmt) {
            Ok(d) => d,
            Err(e) => return Err(format!("line {} (line_no={}): {e}", line_no + 2, line_no)),
        };

        let cents = match parse_amount_str(&fields[amount_idx], dec_sep) {
            Ok(c) => c,
            Err(e) => return Err(format!("line {} (line_no={}): {e}", line_no + 2, line_no)),
        };

        let description = desc_idx.and_then(|i| {
            if i < fields.len() {
                let d = fields[i].trim().to_string();
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            } else {
                None
            }
        });

        rows.push(ImportRow {
            date,
            cents,
            description,
        });
    }

    Ok(rows)
}

/// Split a CSV line by the given delimiter. Strips surrounding quotes from
/// each field. Does not handle embedded delimiters inside quoted fields
/// (sufficient for typical bank exports).
fn split_csv_line(line: &str, delimiter: char) -> Vec<String> {
    line.split(delimiter)
        .map(|f| {
            let trimmed = f.trim();
            // Strip surrounding quotes if present.
            if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
                trimmed[1..trimmed.len() - 1].to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect()
}

// ─── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_date_str ──

    #[test]
    fn parse_date_dmy_slash() {
        let d = parse_date_str("15/01/2024", DateFormat::Dmy).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_dmy_dash() {
        let d = parse_date_str("15-01-2024", DateFormat::Dmy).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_dmy_dot() {
        let d = parse_date_str("15.01.2024", DateFormat::Dmy).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_ymd() {
        let d = parse_date_str("2024-01-15", DateFormat::Ymd).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_mdy_slash() {
        let d = parse_date_str("01/15/2024", DateFormat::Mdy).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_mdy_dash() {
        let d = parse_date_str("01-15-2024", DateFormat::Mdy).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn parse_date_invalid() {
        assert!(parse_date_str("not-a-date", DateFormat::Dmy).is_err());
        assert!(parse_date_str("", DateFormat::Dmy).is_err());
        // 25-month is invalid
        assert!(parse_date_str("15/25/2024", DateFormat::Dmy).is_err());
    }

    // ── parse_amount_str ──

    #[test]
    fn parse_amount_comma_simple() {
        assert_eq!(parse_amount_str("45,50", DecimalSep::Comma).unwrap(), 4550);
    }

    #[test]
    fn parse_amount_comma_negative() {
        assert_eq!(
            parse_amount_str("-45,50", DecimalSep::Comma).unwrap(),
            -4550
        );
    }

    #[test]
    fn parse_amount_comma_thousands() {
        assert_eq!(
            parse_amount_str("1.234,56", DecimalSep::Comma).unwrap(),
            123456
        );
    }

    #[test]
    fn parse_amount_comma_integer_only() {
        assert_eq!(parse_amount_str("1234", DecimalSep::Comma).unwrap(), 123400);
    }

    #[test]
    fn parse_amount_dot_simple() {
        assert_eq!(parse_amount_str("45.50", DecimalSep::Dot).unwrap(), 4550);
    }

    #[test]
    fn parse_amount_dot_negative() {
        assert_eq!(parse_amount_str("-45.50", DecimalSep::Dot).unwrap(), -4550);
    }

    #[test]
    fn parse_amount_dot_thousands() {
        assert_eq!(
            parse_amount_str("1,234.56", DecimalSep::Dot).unwrap(),
            123456
        );
    }

    #[test]
    fn parse_amount_dot_integer_only() {
        assert_eq!(parse_amount_str("1234", DecimalSep::Dot).unwrap(), 123400);
    }

    #[test]
    fn parse_amount_empty() {
        assert!(parse_amount_str("", DecimalSep::Comma).is_err());
        assert!(parse_amount_str("  ", DecimalSep::Comma).is_err());
    }

    // ── detect_columns ──

    /// The critical real-world case: "Data operazione;Data valuta;Descrizione;Importo"
    /// The header "Data operazione" contains "operazione" which a name-only
    /// heuristic might misidentify. The content check (first data row has a
    /// parseable date) disambiguates correctly.
    #[test]
    fn detect_columns_real_world_unicredit() {
        let headers = vec![
            "Data operazione".into(),
            "Data valuta".into(),
            "Descrizione".into(),
            "Importo".into(),
        ];
        let first_row = vec![
            "15/01/2024".into(),
            "15/01/2024".into(),
            "Supermercato SPA".into(),
            "-45,50".into(),
        ];

        let roles = detect_columns(&headers, &first_row, DateFormat::Dmy).unwrap();

        assert_eq!(roles[0], ColumnRole::Date);
        assert_eq!(roles[1], ColumnRole::Other); // second date → not used
        assert_eq!(roles[2], ColumnRole::Description);
        assert_eq!(roles[3], ColumnRole::Amount);
    }

    #[test]
    fn detect_columns_ymd_format() {
        let headers = vec!["Date".into(), "Description".into(), "Amount".into()];
        let first_row = vec!["2024-01-15".into(), "Grocery Store".into(), "-12.34".into()];

        let roles = detect_columns(&headers, &first_row, DateFormat::Ymd).unwrap();
        assert_eq!(roles[0], ColumnRole::Date);
        assert_eq!(roles[1], ColumnRole::Description);
        assert_eq!(roles[2], ColumnRole::Amount);
    }

    #[test]
    fn detect_columns_mdy_format() {
        let headers = vec!["Date".into(), "Description".into(), "Amount".into()];
        let first_row = vec!["01/15/2024".into(), "Coffee Shop".into(), "4.50".into()];

        let roles = detect_columns(&headers, &first_row, DateFormat::Mdy).unwrap();
        assert_eq!(roles[0], ColumnRole::Date);
        assert_eq!(roles[1], ColumnRole::Description);
        assert_eq!(roles[2], ColumnRole::Amount);
    }

    #[test]
    fn detect_columns_header_misleading_content_saves() {
        // Header says "Data operazione" but content is NOT a date (it's a ref number).
        // The content check prevents misidentification; the second date column
        // "Data valuta" is correctly identified instead.
        let headers = vec![
            "Data operazione".into(),
            "Data valuta".into(),
            "Causale".into(),
            "Importo".into(),
        ];
        let first_row = vec![
            "OP20240115".into(), // NOT a date
            "15/01/2024".into(), // IS a date
            "Pagamento".into(),
            "-100,00".into(),
        ];

        let roles = detect_columns(&headers, &first_row, DateFormat::Dmy).unwrap();
        // "Data operazione" has non-date content → not marked as Date.
        // "Data valuta" has date content → correctly marked as Date.
        assert_eq!(roles[1], ColumnRole::Date);
        assert_eq!(roles[3], ColumnRole::Amount);
    }

    #[test]
    fn detect_columns_insufficient_columns() {
        // Only 1 column → can't have both date and amount.
        let headers = vec!["Data".into()];
        let first_row = vec!["15/01/2024".into()];
        assert!(detect_columns(&headers, &first_row, DateFormat::Dmy).is_none());
    }

    #[test]
    fn detect_columns_no_date_no_amount() {
        let headers = vec!["Foo".into(), "Bar".into()];
        let first_row = vec!["hello".into(), "world".into()];
        assert!(detect_columns(&headers, &first_row, DateFormat::Dmy).is_none());
    }

    // ── parse_csv (end-to-end) ──

    #[test]
    fn parse_csv_unicredit_auto_detect() {
        let csv = "Data operazione;Data valuta;Descrizione;Importo\n\
                   15/01/2024;15/01/2024;Supermercato SPA;-45,50\n\
                   16/01/2024;16/01/2024;Stipendio;+2500,00\n\
                   17/01/2024;17/01/2024;Farmacia;-12,30";

        let rows = parse_csv(csv, ';', DateFormat::Dmy, DecimalSep::Comma, None).unwrap();
        assert_eq!(rows.len(), 3);

        assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(rows[0].cents, -4550);
        assert_eq!(rows[0].description.as_deref(), Some("Supermercato SPA"));

        assert_eq!(rows[1].date, NaiveDate::from_ymd_opt(2024, 1, 16).unwrap());
        assert_eq!(rows[1].cents, 250000);
        assert_eq!(rows[1].description.as_deref(), Some("Stipendio"));

        assert_eq!(rows[2].date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
        assert_eq!(rows[2].cents, -1230);
        assert_eq!(rows[2].description.as_deref(), Some("Farmacia"));
    }

    #[test]
    fn parse_csv_ymd_format() {
        let csv = "Date,Description,Amount\n\
                   2024-01-15,Grocery Store,-12.34\n\
                   2024-01-16,Salary,5000.00";

        let rows = parse_csv(csv, ',', DateFormat::Ymd, DecimalSep::Dot, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cents, -1234);
        assert_eq!(rows[1].cents, 500000);
    }

    #[test]
    fn parse_csv_mdy_format() {
        let csv = "Date,Description,Amount\n\
                   01/15/2024,Coffee Shop,4.50\n\
                   01/16/2024,Restaurant,32.75";

        let rows = parse_csv(csv, ',', DateFormat::Mdy, DecimalSep::Dot, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(rows[0].cents, 450);
        assert_eq!(rows[1].cents, 3275);
    }

    #[test]
    fn parse_csv_explicit_col_map() {
        // Even with auto-detection that might fail, an explicit col_map works.
        let csv = "Ref;Date;Memo;Value\n\
                   ABC123;15/01/2024;Payment;100,00\n\
                   DEF456;16/01/2024;Refund;-50,25";

        let col_map = vec![
            ColumnRole::Other,
            ColumnRole::Date,
            ColumnRole::Description,
            ColumnRole::Amount,
        ];
        let rows = parse_csv(csv, ';', DateFormat::Dmy, DecimalSep::Comma, Some(&col_map)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cents, 10000);
        assert_eq!(rows[1].cents, -5025);
    }

    #[test]
    fn parse_csv_too_few_lines() {
        assert!(parse_csv("header only", ';', DateFormat::Dmy, DecimalSep::Comma, None).is_err());
    }

    #[test]
    fn parse_csv_empty() {
        assert!(parse_csv("", ';', DateFormat::Dmy, DecimalSep::Comma, None).is_err());
    }

    // ── DedupKey ──

    #[test]
    fn dedup_key_from_row() {
        let row = ImportRow {
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            cents: -4550,
            description: Some("Test".into()),
        };
        let key = DedupKey::from_row(&row);
        assert_eq!(key.date, row.date);
        assert_eq!(key.cents, -4550);
        assert_eq!(key.description.as_deref(), Some("Test"));
    }

    // ── Enum try_from / as_str round-trip ──

    #[test]
    fn date_format_roundtrip() {
        for fmt in [DateFormat::Dmy, DateFormat::Ymd, DateFormat::Mdy] {
            assert_eq!(DateFormat::try_from(fmt.as_str()).unwrap(), fmt);
        }
    }

    #[test]
    fn decimal_sep_roundtrip() {
        assert_eq!(DecimalSep::try_from(",").unwrap(), DecimalSep::Comma);
        assert_eq!(DecimalSep::try_from(".").unwrap(), DecimalSep::Dot);
    }

    #[test]
    fn column_role_roundtrip() {
        for role in [
            ColumnRole::Date,
            ColumnRole::Amount,
            ColumnRole::Description,
            ColumnRole::Other,
        ] {
            assert_eq!(ColumnRole::try_from(role.as_str()).unwrap(), role);
        }
    }
}
