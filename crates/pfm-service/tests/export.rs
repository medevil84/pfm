//! Service-level tests for the export feature:
//!
//! 1. **Filter coherence** – the rows produced by `ExportService::export`
//!    must match exactly the rows from `TransactionService::register` for the
//!    same account + period + filters (no row is lost or duplicated).
//!
//! 2. **Permissions** – a user without access (not owner, not admin, no share)
//!    gets `ExportError::Forbidden`; a shared user succeeds.
//!
//! 3. **Running balance** – the per-row running balance in the export matches
//!    the per-row running balance from the register (same opening, same order).

use chrono::NaiveDate;
use pfm_domain::{parse_amount_str, DecimalSep};
use pfm_repo::{DbSettings, Engine, RegisterFilters, Storage};
use pfm_service::{ExportService, TransactionService};

const OWNER: &str = "u1";
const VIEWER: &str = "u2";
const NOACCESS: &str = "u3";
const ACCOUNT: &str = "acc1";
const CAT_OUT: &str = "cat-out";
const CAT_IN: &str = "cat-in";
const CAT_FOOD: &str = "cat-food";

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Split a CSV line into fields, respecting double-quoted fields
/// (embedded `;` inside quotes is not a separator; `""` → `"`).
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next(); // skip second quote
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => {
                in_quotes = true;
            }
            ';' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            _ => {
                current.push(c);
            }
        }
    }
    fields.push(current);
    fields
}

async fn setup() -> (ExportService, TransactionService, Storage) {
    sqlx::any::install_default_drivers();

    let storage = Storage::connect(&DbSettings {
        engine: Engine::Sqlite,
        sqlite_path: Some(":memory:".into()),
        sqlite_apply_pragmas: false,
        sqlite_max_connections: 1,
        mariadb_url: None,
        mariadb_max_connections: 0,
        mariadb_min_connections: 0,
    })
    .await
    .unwrap();

    // Real schema, from the migrations the application actually runs — indexes
    // included. The hand-written DDL this replaces defined no indexes at all,
    // so constraint violations that break production went unnoticed here.
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();

    // users

    for (id, uname) in [("u1", "owner"), ("u2", "viewer"), ("u3", "noaccess")] {
        sqlx::query("INSERT INTO users (id, username, email, password_hash, role, created_at) VALUES (?, ?, ?, 'x', 'user', '2025-01-01T00:00:00Z')")
            .bind(id)
            .bind(uname)
            .bind(format!("{uname}@example.com"))
            .execute(pool)
            .await
            .unwrap();
    }

    // accounts

    // start with 10_000 (100.00) opening balance
    sqlx::query("INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, created_at) VALUES ('acc1', 'u1', 'My Account', 'cash', '2024-01-01', 10000, '2024-01-01T00:00:00Z')")
        .execute(pool)
        .await
        .unwrap();

    // categories

    sqlx::query("INSERT INTO categories (id, key, name, kind, created_at) VALUES ('cat-out', 'out', 'Spending', 'out', '2024-01-01')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO categories (id, key, name, kind, created_at) VALUES ('cat-in', 'in', 'Income', 'in', '2024-01-01')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO categories (id, key, name, kind, created_at) VALUES ('cat-food', 'food', 'Food', 'out', '2024-01-01')")
        .execute(pool)
        .await
        .unwrap();

    // account_shares: u2 can see acc1

    sqlx::query("INSERT INTO account_shares (id, account_id, user_id, level, created_at) VALUES ('sh1', 'acc1', 'u2', 'read', '2024-01-01')")
        .execute(pool)
        .await
        .unwrap();

    // transactions

    // Seed transactions over Jan-Feb 2024:
    // tx1: -500 (5.00) out, food
    // tx2: +2000 (20.00) in, salary
    // tx3: -1250 (12.50) out, spending (with semicolons in description for escaping test)
    // tx4: -100 (1.00) out, food
    // tx5: +5000 (50.00) in, salary
    let txs = [
        ("tx1", "2024-01-10", -500i64, "Lunch", CAT_FOOD),
        ("tx2", "2024-01-15", 2000, "Salary", CAT_IN),
        ("tx3", "2024-01-20", -1250, "Semi;colon \"test\"", CAT_OUT),
        ("tx4", "2024-02-05", -100, "Coffee", CAT_FOOD),
        ("tx5", "2024-02-10", 5000, "Bonus", CAT_IN),
    ];
    for (id, date, cents, desc, cat) in &txs {
        sqlx::query(
            "INSERT INTO transactions (id, client_id, account_id, date, cents, description, category_id, created_by, created_at, updated_at) VALUES (?, ?, 'acc1', ?, ?, ?, ?, 'u1', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(id)
        .bind(date)
        .bind(cents)
        .bind(desc)
        .bind(cat)
        .execute(pool)
        .await
        .unwrap();
    }

    // user_prefs (required by TransactionService)

    let export_svc = ExportService::new(&storage);
    let tx_svc = TransactionService::new(&storage);
    (export_svc, tx_svc, storage)
}

// ─── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn export_matches_register_rows() {
    let (export_svc, tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    // register result
    let reg = tx_svc
        .register(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    // collect all rows from register
    let reg_rows: Vec<_> = reg.days.iter().flat_map(|day| day.rows.iter()).collect();

    // export result
    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    // parse CSV rows (skip BOM + header)
    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let csv_rows = &lines[1..]; // skip header

    // row count must match
    assert_eq!(
        csv_rows.len(),
        reg_rows.len(),
        "export has {} rows but register has {}",
        csv_rows.len(),
        reg_rows.len()
    );

    // verify each row's date, cents, and running balance match
    for (i, (csv_line, reg_row)) in csv_rows.iter().zip(reg_rows.iter()).enumerate() {
        let fields: Vec<String> = split_csv_line(csv_line);
        assert_eq!(
            fields.len(),
            5,
            "row {} should have 5 fields: {}",
            i,
            csv_line
        );

        // date: dd/mm/aaaa
        let expected_date = reg_row.tx.date.format("%d/%m/%Y").to_string();
        assert_eq!(fields[0], expected_date, "row {} date", i);

        // amount: comma decimal
        let expected_amount = pfm_domain::cents_to_decimal(reg_row.tx.cents);
        assert_eq!(fields[3], expected_amount, "row {} amount", i);

        // running balance: comma decimal
        let expected_balance = pfm_domain::cents_to_decimal(reg_row.running_balance_cents);
        assert_eq!(fields[4], expected_balance, "row {} running balance", i);
    }
}

#[tokio::test]
async fn export_filter_by_category() {
    let (export_svc, tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: Some(CAT_FOOD.to_string()),
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    let reg = tx_svc
        .register(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();
    let reg_rows: Vec<_> = reg.days.iter().flat_map(|day| day.rows.iter()).collect();

    // only tx1 (Lunch, -500) and tx4 (Coffee, -100) have category cat-food
    assert_eq!(reg_rows.len(), 2);

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let csv_rows = &lines[1..];
    assert_eq!(csv_rows.len(), 2, "food-only export should have 2 rows");
}

#[tokio::test]
async fn export_filter_min_max() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    // min=100, max=2000 → ABS(cents) in [100, 2000]
    // tx1=500✓, tx2=2000✓, tx3=1250✓, tx4=100✓, tx5=5000✗
    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: Some(100),
        max_cents: Some(2000),
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let csv_rows = &lines[1..];

    assert_eq!(
        csv_rows.len(),
        4,
        "min/max filter (abs 100-2000) should keep 4 rows"
    );
}

#[tokio::test]
async fn export_forbidden_for_non_shared_user() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    // u3 has no share and is not the owner
    let result = export_svc
        .export(NOACCESS, ACCOUNT, from, to, &filters)
        .await;
    assert!(matches!(result, Err(pfm_service::ExportError::Forbidden)));
}

#[tokio::test]
async fn export_allowed_for_shared_user() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    // u2 has a read share
    let out = export_svc
        .export(VIEWER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert!(lines.len() > 1); // header + at least one data row
}

#[tokio::test]
async fn export_round_trip_precision() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let csv_rows = &lines[1..];

    for (i, line) in csv_rows.iter().enumerate() {
        let fields: Vec<String> = split_csv_line(line);

        // parse the amount back to cents and verify it round-trips
        let parsed = parse_amount_str(&fields[3], DecimalSep::Comma).unwrap();
        let reformatted = pfm_domain::cents_to_decimal(parsed);
        assert_eq!(
            fields[3], reformatted,
            "row {} amount did not round-trip: '{}' != '{}'",
            i, fields[3], reformatted
        );

        // parse running balance
        let bal = parse_amount_str(&fields[4], DecimalSep::Comma).unwrap();
        let bal_re = pfm_domain::cents_to_decimal(bal);
        assert_eq!(fields[4], bal_re, "row {} balance did not round-trip", i);
    }
}

#[tokio::test]
async fn export_escaping_special_chars() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: Some("Semi".into()),
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    // The description "Semi;colon \"test\"" contains both `;` and `"`.
    // It must be quoted and inner quotes doubled in the CSV output.
    assert!(
        out.csv.contains("\"Semi;colon \"\"test\"\"\""),
        "expected escaped field in CSV, got:\n{}",
        out.csv
    );
}

#[tokio::test]
async fn export_csv_format_verification() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    let from = d("2024-01-01");
    let to = d("2024-02-29");

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    // 1. BOM
    assert!(
        out.csv.starts_with('\u{feff}'),
        "CSV must start with UTF-8 BOM"
    );

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();

    // 2. CRLF line endings
    assert!(stripped.contains("\r\n"), "CSV must use CRLF line endings");

    // 3. Semicolon separator in header
    let first_line = stripped.split('\n').next().unwrap().trim_end_matches('\r');
    assert_eq!(
        first_line, "Data;Descrizione;Categoria;Importo;Saldo",
        "header must be ; -separated with Italian column names"
    );

    // 4. Filename ends with .csv
    assert!(
        out.filename.ends_with(".csv"),
        "filename must end with .csv: {}",
        out.filename
    );
}

#[tokio::test]
async fn export_running_balance_continuity() {
    let (export_svc, _tx_svc, _storage) = setup().await;

    let filters = RegisterFilters {
        category_id: None,
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(vec![]),
    };

    // narrow window: only tx4 and tx5 (Feb)
    let from = d("2024-02-01");
    let to = d("2024-02-29");

    let out = export_svc
        .export(OWNER, ACCOUNT, from, to, &filters)
        .await
        .unwrap();

    let stripped = out.csv.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let csv_rows = &lines[1..];

    assert_eq!(csv_rows.len(), 2);

    // Opening balance: start_balance (10000) + pre_sum for [tx1 + tx2 + tx3] = 10000 + (-500+2000-1250) = 10250
    // tx4: -100 → 10150
    // tx5: +5000 → 15150
    let line1 = csv_rows[0];
    let fields1: Vec<String> = split_csv_line(line1);
    assert_eq!(
        fields1[4],
        pfm_domain::cents_to_decimal(10150),
        "tx4 running balance"
    );

    let line2 = csv_rows[1];
    let fields2: Vec<String> = split_csv_line(line2);
    assert_eq!(
        fields2[4],
        pfm_domain::cents_to_decimal(15150),
        "tx5 running balance"
    );
}
