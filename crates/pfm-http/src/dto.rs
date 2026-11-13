//! Request / response DTOs for the auth endpoints.
//!
//! These mirror the domain types but add `Serialize` / `Deserialize` derives
//! (which the domain crate deliberately avoids).

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// `POST /auth/login` request body.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /auth/login` response body.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user: UserDto,
    pub csrf_token: String,
}

/// Lightweight user representation sent to the client.
#[derive(Debug, Serialize, Deserialize)]
pub struct UserDto {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub locale: String,
    pub default_currency: String,
}

impl From<&pfm_domain::AuthUser> for UserDto {
    fn from(u: &pfm_domain::AuthUser) -> Self {
        Self {
            id: u.id.clone(),
            username: u.username.clone(),
            email: u.email.clone(),
            role: u.role.as_str().to_string(),
            locale: u.locale.clone(),
            default_currency: u.default_currency.clone(),
        }
    }
}

impl From<&pfm_domain::User> for UserDto {
    fn from(u: &pfm_domain::User) -> Self {
        let auth = u.auth();
        Self::from(&auth)
    }
}

/// `PUT /auth/password` request body.
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Body of `PUT /auth/locale`.
#[derive(Debug, Deserialize)]
pub struct SetLocaleRequest {
    pub locale: String,
}

/// User preferences DTO (bidirectional).
#[derive(Debug, Serialize, Deserialize)]
pub struct PrefsDto {
    pub theme: String,
    pub show_sign: bool,
    pub privacy: bool,
    pub show_m_saldo: bool,
    pub empty_days: i32,
    pub show_future: bool,
    pub rows_above: i32,
    pub rows_below: i32,
    pub rows_step: i32,
    /// Holiday calendar code; absent (older clients) keeps the default.
    #[serde(default = "default_holiday_calendar")]
    pub holiday_calendar: String,
    /// Colour of positive amounts, `#rrggbb`; empty/absent = default (green).
    #[serde(default)]
    pub color_pos: String,
    /// Colour of negative amounts, `#rrggbb`; empty/absent = default (red).
    #[serde(default)]
    pub color_neg: String,
}

fn default_holiday_calendar() -> String {
    "it".to_string()
}

impl From<&pfm_domain::UserPrefs> for PrefsDto {
    fn from(p: &pfm_domain::UserPrefs) -> Self {
        Self {
            theme: p.theme.clone(),
            show_sign: p.show_sign,
            privacy: p.privacy,
            show_m_saldo: p.show_m_saldo,
            empty_days: p.empty_days,
            show_future: p.show_future,
            rows_above: p.rows_above,
            rows_below: p.rows_below,
            rows_step: p.rows_step,
            holiday_calendar: p.holiday_calendar.clone(),
            color_pos: p.color_pos.clone(),
            color_neg: p.color_neg.clone(),
        }
    }
}

impl From<PrefsDto> for pfm_domain::UserPrefs {
    fn from(d: PrefsDto) -> Self {
        Self {
            theme: d.theme,
            show_sign: d.show_sign,
            privacy: d.privacy,
            show_m_saldo: d.show_m_saldo,
            empty_days: d.empty_days,
            show_future: d.show_future,
            rows_above: d.rows_above,
            rows_below: d.rows_below,
            rows_step: d.rows_step,
            holiday_calendar: d.holiday_calendar,
            color_pos: d.color_pos,
            color_neg: d.color_neg,
        }
    }
}

/// `GET /auth/me` response.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user: UserDto,
}

// ─── Account DTOs ───────────────────────────────────────────────────────────────

/// Account representation sent to the client.
#[derive(Debug, Serialize)]
pub struct AccountDto {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub kind: String,
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iban: Option<String>,
    pub currency: String,
    pub start_date: String,
    pub start_balance_cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_cents: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_day: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closing_day: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_pct_bps: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_cents: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_balance_cents: Option<i64>,
    pub active: bool,
    pub created_at: String,
    pub sort_order: i64,
}

impl From<&pfm_domain::Account> for AccountDto {
    fn from(a: &pfm_domain::Account) -> Self {
        Self {
            id: a.id.clone(),
            owner_id: a.owner_id.clone(),
            name: a.name.clone(),
            kind: a.kind.as_str().to_string(),
            icon: a.icon.clone(),
            note: a.note.clone(),
            iban: a.iban.clone(),
            currency: a.currency.clone(),
            start_date: a.start_date.format("%Y-%m-%d").to_string(),
            start_balance_cents: a.start_balance_cents,
            limit_cents: a.limit_cents,
            charge_day: a.charge_day,
            closing_day: a.closing_day,
            primary_account_id: a.primary_account_id.clone(),
            rate_pct_bps: a.rate_pct_bps,
            goal_cents: a.goal_cents,
            min_balance_cents: a.min_balance_cents,
            active: a.active,
            created_at: a.created_at.clone(),
            sort_order: a.sort_order,
        }
    }
}

/// `POST /accounts/reorder` request body.
#[derive(Debug, Deserialize)]
pub struct AccountReorderRequest {
    /// Account ids in the desired display order (front to back).
    pub ids: Vec<String>,
}

/// `POST /accounts` request body.
#[derive(Debug, Deserialize)]
pub struct AccountCreateRequest {
    pub name: String,
    pub kind: String,
    pub icon: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub iban: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    pub start_date: String,
    #[serde(default)]
    pub start_balance_cents: i64,
    #[serde(default)]
    pub limit_cents: Option<i64>,
    #[serde(default)]
    pub charge_day: Option<i32>,
    #[serde(default)]
    pub closing_day: Option<i32>,
    #[serde(default)]
    pub primary_account_id: Option<String>,
    #[serde(default)]
    pub rate_pct_bps: Option<i64>,
    #[serde(default)]
    pub goal_cents: Option<i64>,
    #[serde(default)]
    pub min_balance_cents: Option<i64>,
}

/// `PUT /accounts/{id}` request body. All fields optional.
#[derive(Debug, Default, Deserialize)]
pub struct AccountUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub iban: Option<String>,
    #[serde(default)]
    pub limit_cents: Option<i64>,
    #[serde(default)]
    pub charge_day: Option<i32>,
    #[serde(default)]
    pub closing_day: Option<i32>,
    #[serde(default)]
    pub primary_account_id: Option<String>,
    #[serde(default)]
    pub rate_pct_bps: Option<i64>,
    #[serde(default)]
    pub goal_cents: Option<i64>,
    #[serde(default)]
    pub min_balance_cents: Option<i64>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// Query parameters for `GET /accounts` (filter by account kind).
#[derive(Debug, Default, Deserialize)]
pub struct AccountListQueryParams {
    #[serde(default)]
    pub kind: Option<String>,
}

/// A selection of piggy-bank ids parsed from a query parameter, given as a
/// comma-separated list (`?vacct_included=pig1,pig2`). A single id and the
/// empty string are both accepted.
#[derive(Debug, Clone, Default)]
pub struct VacctSelection(pub Vec<String>);

impl<'de> Deserialize<'de> for VacctSelection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct VacctVisitor;

        impl<'de> Visitor<'de> for VacctVisitor {
            type Value = VacctSelection;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a comma-separated list of piggy-bank ids or repeated query parameters",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(VacctSelection(split_vacct_ids(value)))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut out = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    out.extend(split_vacct_ids(&value));
                }
                Ok(VacctSelection(out))
            }
        }

        deserializer.deserialize_any(VacctVisitor)
    }
}

/// Split a raw piggy-bank ids value on commas, trimming and dropping empties.
fn split_vacct_ids(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Query parameters for `GET /accounts/{id}/balance`.
#[derive(Debug, Default, Deserialize)]
pub struct BalanceQueryParams {
    /// Piggy-bank ids to fold back into the "available" balance. Absent or
    /// empty yields `allocated_cents == real_cents`.
    #[serde(default)]
    pub vacct_included: VacctSelection,
}

/// `GET /accounts/{id}/balance` response body: the account's two balances.
#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub account_id: String,
    pub real_cents: i64,
    pub allocated_cents: i64,
    /// Remaining limit of a credit card, excluding the computed closing
    /// movement. Absent for other accounts or cards without a limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_available_cents: Option<i64>,
    /// Amount spent on a credit card and not yet charged (positive number).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_outstanding_cents: Option<i64>,
}

impl From<&pfm_service::VirtualBalanceView> for BalanceResponse {
    fn from(v: &pfm_service::VirtualBalanceView) -> Self {
        Self {
            account_id: v.account_id.clone(),
            real_cents: v.real_cents,
            allocated_cents: v.allocated_cents,
            card_available_cents: None,
            card_outstanding_cents: None,
        }
    }
}

// ─── Category DTOs ──────────────────────────────────────────────────────────────

/// Category representation sent to the client.
#[derive(Debug, Serialize)]
pub struct CategoryDto {
    pub id: String,
    pub key: String,
    pub name: String,
    pub emoji: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_cents: Option<i64>,
    pub is_fallback: bool,
    pub created_at: String,
    /// Creator of the category; absent for shared ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Whether the requesting user may rename / delete it.
    pub editable: bool,
}

impl From<&pfm_service::CategoryItem> for CategoryDto {
    fn from(item: &pfm_service::CategoryItem) -> Self {
        let c = &item.category;
        Self {
            id: c.id.clone(),
            key: c.key.clone(),
            name: c.name.clone(),
            emoji: c.emoji.clone(),
            kind: c.kind.as_str().to_string(),
            budget_cents: c.budget_cents,
            is_fallback: c.is_fallback,
            created_at: c.created_at.clone(),
            owner_id: c.owner_id.clone(),
            editable: item.editable,
        }
    }
}

/// `POST /categories` request body.
#[derive(Debug, Deserialize)]
pub struct CategoryCreateRequest {
    pub name: String,
    pub emoji: String,
    pub kind: String,
    #[serde(default)]
    pub budget_cents: Option<i64>,
}

/// `PUT /categories/{id}` request body. All fields optional.
#[derive(Debug, Default, Deserialize)]
pub struct CategoryUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub budget_cents: Option<i64>,
}

// ─── Share DTOs ──────────────────────────────────────────────────────────────

/// `POST /accounts/{id}/shares` request body.
#[derive(Debug, Deserialize)]
pub struct ShareCreateRequest {
    pub email: String,
    #[serde(default)]
    pub level: Option<String>,
}

/// A share entry returned to the client.
#[derive(Debug, Serialize)]
pub struct ShareDto {
    pub id: String,
    pub account_id: String,
    pub user_id: String,
    pub level: String,
    pub created_at: String,
}

impl From<&pfm_domain::ShareEntry> for ShareDto {
    fn from(e: &pfm_domain::ShareEntry) -> Self {
        Self {
            id: e.id.clone(),
            account_id: e.account_id.clone(),
            user_id: e.user_id.clone(),
            level: e.level.as_str().to_string(),
            created_at: e.created_at.clone(),
        }
    }
}

/// `GET /accounts/{id}/shares/search` response body.
#[derive(Debug, Serialize)]
pub struct ShareSearchResponse {
    pub shared: bool,
}

// ─── Transaction DTOs ──────────────────────────────────────────────────────────

/// Transaction representation sent to the client.
#[derive(Debug, Serialize)]
pub struct TransactionDto {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub account_id: String,
    pub date: String,
    pub cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&pfm_domain::Transaction> for TransactionDto {
    fn from(t: &pfm_domain::Transaction) -> Self {
        Self {
            id: t.id.clone(),
            client_id: Some(t.client_id.clone()),
            account_id: t.account_id.clone(),
            date: t.date.format("%Y-%m-%d").to_string(),
            cents: t.cents,
            description: t.description.clone(),
            category_id: t.category_id.clone(),
            note: t.note.clone(),
            created_by: t.created_by.clone(),
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
        }
    }
}

/// `POST /transactions` request body.
#[derive(Debug, Deserialize)]
pub struct TransactionCreateRequest {
    pub account_id: String,
    pub date: String,
    pub cents: i64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Client-generated UUID for idempotent offline sync. When the same
    /// `client_id` is sent twice the server returns the existing row
    /// instead of creating a duplicate.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// `PUT /transactions/{id}` request body. All fields optional.
#[derive(Debug, Default, Deserialize)]
pub struct TransactionUpdateRequest {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub cents: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Move the movement to this account. Rejected (422) for transfer legs,
    /// recurring, rule-generated, virtual-account and imported movements.
    #[serde(default)]
    pub account_id: Option<String>,
}

/// A row in the register: a transaction with its running balance.
///
/// `account_id`, `pair_id` and `rec_id` are present because the register view
/// needs them and cannot derive them: the account column shown when several
/// accounts are listed together, the link to the other leg of a transfer, and
/// the link to the rule that generated the row. They are read from the same
/// transaction the rest of the row comes from, so they add no query.
#[derive(Debug, Serialize)]
pub struct TransactionRowDto {
    pub id: String,
    pub account_id: String,
    pub date: String,
    pub cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The shared identifier of a transfer pair, when this row is one leg.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<String>,
    /// The recurrence rule that produced this row, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rec_id: Option<String>,
    /// True when the row's account cannot be changed (transfer legs,
    /// recurring, rule-generated, virtual-account and imported rows). The
    /// client uses it to disable the account picker when editing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub account_locked: bool,
    pub running_balance_cents: i64,
}

impl From<&pfm_domain::TransactionRow> for TransactionRowDto {
    fn from(r: &pfm_domain::TransactionRow) -> Self {
        Self {
            id: r.tx.id.clone(),
            account_id: r.tx.account_id.clone(),
            date: r.tx.date.format("%Y-%m-%d").to_string(),
            cents: r.tx.cents,
            description: r.tx.description.clone(),
            category_id: r.tx.category_id.clone(),
            note: r.tx.note.clone(),
            pair_id: r.tx.pair_id.clone(),
            rec_id: r.tx.rec_id.clone(),
            account_locked: !r.tx.can_change_account(),
            running_balance_cents: r.running_balance_cents,
        }
    }
}

/// A day in the register response.
#[derive(Debug, Serialize)]
pub struct RegisterDayDto {
    pub date: String,
    pub is_empty: bool,
    pub is_today: bool,
    pub is_future: bool,
    pub closing_balance_cents: i64,
    pub rows: Vec<TransactionRowDto>,
}

impl From<&pfm_domain::RegisterDay> for RegisterDayDto {
    fn from(d: &pfm_domain::RegisterDay) -> Self {
        Self {
            date: d.date.format("%Y-%m-%d").to_string(),
            is_empty: d.is_empty,
            is_today: d.is_today,
            is_future: d.is_future,
            closing_balance_cents: d.closing_balance_cents,
            rows: d.rows.iter().map(TransactionRowDto::from).collect(),
        }
    }
}

/// Summary block in the register response.
#[derive(Debug, Serialize)]
pub struct RegisterSummaryDto {
    pub income_cents: i64,
    pub expense_cents: i64,
    pub net_cents: i64,
    pub opening_cents: i64,
    pub closing_cents: i64,
}

impl From<&pfm_domain::RegisterSummary> for RegisterSummaryDto {
    fn from(s: &pfm_domain::RegisterSummary) -> Self {
        Self {
            income_cents: s.income_cents,
            expense_cents: s.expense_cents,
            net_cents: s.net_cents,
            opening_cents: s.opening_cents,
            closing_cents: s.closing_cents,
        }
    }
}

/// `GET /transactions` response body (the register).
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub days: Vec<RegisterDayDto>,
    pub summary: RegisterSummaryDto,
}

impl From<&pfm_domain::RegisterResult> for RegisterResponse {
    fn from(r: &pfm_domain::RegisterResult) -> Self {
        Self {
            days: r.days.iter().map(RegisterDayDto::from).collect(),
            summary: RegisterSummaryDto::from(&r.summary),
        }
    }
}

/// Query parameters for the register endpoint.
#[derive(Debug, Deserialize)]
pub struct RegisterQueryParams {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub cat: Option<String>,
    #[serde(default)]
    pub tipo: Option<String>,
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    #[serde(default)]
    pub q: Option<String>,
    /// Piggy-bank ids to fold into the register/balance of the account. When
    /// present (even empty) the register shows only non-allocated movements and
    /// the allocation legs of the listed piggy banks.
    #[serde(default)]
    pub vacct_included: VacctSelection,
}

// ─── Transfer DTOs ─────────────────────────────────────────────────────────────

/// Transfer representation sent to the client.
#[derive(Debug, Serialize)]
pub struct TransferDto {
    pub pair_id: String,
    pub from_account_id: String,
    pub to_account_id: String,
    pub date: String,
    pub cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&pfm_domain::Transfer> for TransferDto {
    fn from(t: &pfm_domain::Transfer) -> Self {
        Self {
            pair_id: t.pair_id.clone(),
            from_account_id: t.from_account_id.clone(),
            to_account_id: t.to_account_id.clone(),
            date: t.date.format("%Y-%m-%d").to_string(),
            cents: t.cents,
            description: t.description.clone(),
            note: t.note.clone(),
            created_by: t.created_by.clone(),
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
        }
    }
}

/// `POST /transfers` request body.
#[derive(Debug, Deserialize)]
pub struct TransferCreateRequest {
    pub from_account_id: String,
    pub to_account_id: String,
    pub date: String,
    pub cents: i64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `PUT /transfers/{id}` request body. All fields optional.
#[derive(Debug, Default, Deserialize)]
pub struct TransferUpdateRequest {
    #[serde(default)]
    pub from_account_id: Option<String>,
    #[serde(default)]
    pub to_account_id: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub cents: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Query parameters for `GET /transfers`.
#[derive(Debug, Deserialize)]
pub struct TransferListQueryParams {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub from_account: Option<String>,
    #[serde(default)]
    pub to_account: Option<String>,
}

// ─── Bulk DTOs ─────────────────────────────────────────────────────────────────

/// `POST /transactions/bulk` request body.
#[derive(Debug, Deserialize)]
pub struct BulkRequest {
    pub ids: Vec<String>,
    pub action: String,
    #[serde(default)]
    pub category_id: Option<String>,
}

/// `POST /transactions/bulk` response body.
#[derive(Debug, Serialize)]
pub struct BulkResponse {
    pub affected: u64,
}

// ─── Recurrence DTOs ───────────────────────────────────────────────────────────

/// Recurrence rule representation sent to the client.
#[derive(Debug, Serialize)]
pub struct RecurrenceDto {
    pub id: String,
    pub owner_id: String,
    pub description: String,
    pub category_id: String,
    pub kind: String,
    pub amount_cents: i64,
    pub account_id: String,
    pub pattern: String,
    pub weekend: String,
    pub first_date: String,
    pub end_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_count: Option<i64>,
    pub mode: String,
    pub avg_n: i64,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_category_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&pfm_domain::Recurrence> for RecurrenceDto {
    fn from(r: &pfm_domain::Recurrence) -> Self {
        Self {
            id: r.id.clone(),
            owner_id: r.owner_id.clone(),
            description: r.description.clone(),
            category_id: r.category_id.clone(),
            kind: r.kind.as_str().to_string(),
            amount_cents: r.amount_cents,
            account_id: r.account_id.clone(),
            pattern: r.pattern.clone(),
            weekend: r.weekend.as_str().to_string(),
            first_date: r.first_date.format("%Y-%m-%d").to_string(),
            end_type: r.end_type.as_str().to_string(),
            end_date: r.end_date.map(|d| d.format("%Y-%m-%d").to_string()),
            end_count: r.end_count,
            mode: r.mode.as_str().to_string(),
            avg_n: r.avg_n,
            active: r.active,
            note: if r.note.is_empty() {
                None
            } else {
                Some(r.note.clone())
            },
            linked_account_id: r.linked_account_id.clone(),
            linked_category_id: r.linked_category_id.clone(),
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
        }
    }
}

/// `POST /recurrences` request body.
#[derive(Debug, Deserialize)]
pub struct RecurrenceCreateRequest {
    pub description: String,
    pub category_id: String,
    pub kind: String,
    pub amount_cents: i64,
    pub account_id: String,
    pub pattern: String,
    #[serde(default)]
    pub weekend: Option<String>,
    pub first_date: String,
    #[serde(default)]
    pub end_type: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub end_count: Option<i64>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub avg_n: Option<i64>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub linked_account_id: Option<String>,
    #[serde(default)]
    pub linked_category_id: Option<String>,
}

/// `PUT /recurrences/{id}` request body. All fields optional; `None` means keep
/// the current value. The nullable end fields use a double `Option`: `None` =
/// unchanged, `Some(None)` = clear, `Some(Some(..))` = set.
#[derive(Debug, Default, Deserialize)]
pub struct RecurrenceUpdateRequest {
    /// Also rewrite the amount of the already-registered occurrences (fixed
    /// rules only).
    #[serde(default)]
    pub apply_to_existing: bool,
    /// Only rewrite occurrences dated on or after this day (`YYYY-MM-DD`);
    /// absent = all of them.
    #[serde(default)]
    pub apply_from: Option<String>,
    /// Also rewrite the occurrences modified by hand.
    #[serde(default)]
    pub overwrite_manual: bool,
    /// After a start-date/pattern change: register the missing past occurrences.
    #[serde(default)]
    pub create_missing: bool,
    /// After a start-date/pattern change, for rows the schedule no longer
    /// generates: `keep` (default), `delete_auto` or `delete_all`.
    #[serde(default)]
    pub stale_action: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub amount_cents: Option<i64>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub weekend: Option<String>,
    #[serde(default)]
    pub first_date: Option<String>,
    #[serde(default)]
    pub end_type: Option<String>,
    #[serde(default)]
    pub end_date: Option<Option<String>>,
    #[serde(default)]
    pub end_count: Option<Option<i64>>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub avg_n: Option<i64>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    /// `Some(None)` clears the linked account (turns off transfer mode).
    #[serde(default)]
    pub linked_account_id: Option<Option<String>>,
    /// `Some(None)` clears the linked category.
    #[serde(default)]
    pub linked_category_id: Option<Option<String>>,
}

/// `PUT /recurrences/{id}` response: the updated rule plus the outcome of the
/// optional re-alignment of already-registered occurrences.
#[derive(Debug, Serialize)]
pub struct RecurrenceUpdateResponse {
    #[serde(flatten)]
    pub recurrence: RecurrenceDto,
    /// Transactions whose amount was rewritten.
    pub aligned: u64,
    /// Hand-modified transactions left untouched (no `overwrite_manual`).
    pub skipped_manual: u64,
    /// Rows deleted because the new schedule no longer generates them.
    pub removed: u64,
    /// Past occurrences registered after a schedule change.
    pub created: u64,
}

/// Effect a schedule change would have on the registered rows.
#[derive(Debug, Serialize)]
pub struct DateImpactDto {
    pub to_create: u64,
    pub stale_auto: u64,
    pub stale_manual: u64,
}

/// A generated (or confirmed) occurrence of a recurrence rule.
#[derive(Debug, Serialize)]
pub struct RecurrenceOccurrenceDto {
    pub date: String,
    pub amount_cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub overridden: bool,
}

impl From<&pfm_domain::RecurrenceOccurrence> for RecurrenceOccurrenceDto {
    fn from(o: &pfm_domain::RecurrenceOccurrence) -> Self {
        Self {
            date: o.date.format("%Y-%m-%d").to_string(),
            amount_cents: o.amount_cents,
            description: o.description.clone(),
            overridden: o.overridden,
        }
    }
}

/// Query parameters for `GET /recurrences/{id}/occurrences`.
#[derive(Debug, Deserialize)]
pub struct OccurrencesQueryParams {
    pub from: String,
    pub to: String,
}

/// A persisted per-date override of a recurrence rule.
#[derive(Debug, Serialize)]
pub struct OverrideDto {
    pub id: String,
    pub rec_id: String,
    pub date: String,
    pub amount_cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
}

impl From<&pfm_domain::RecurrenceOverride> for OverrideDto {
    fn from(o: &pfm_domain::RecurrenceOverride) -> Self {
        Self {
            id: o.id.clone(),
            rec_id: o.rec_id.clone(),
            date: o.date.format("%Y-%m-%d").to_string(),
            amount_cents: o.amount_cents,
            description: o.description.clone(),
            created_at: o.created_at.clone(),
        }
    }
}

/// `PUT /recurrences/{id}/overrides/{date}` request body.
#[derive(Debug, Deserialize)]
pub struct OverrideRequest {
    pub amount_cents: i64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub update_rule: bool,
}

// ─── Spending-rule DTOs ────────────────────────────────────────────────────────

/// A spending rule sent to the client.
#[derive(Debug, Serialize)]
pub struct RuleDto {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub category_id: String,
    pub calc_type: String,
    pub value: i64,
    pub is_annual: bool,
    pub prorata_days: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annual_cap_cents: Option<i64>,
    pub cadence: String,
    pub charge_day: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_month: Option<u32>,
    pub cond_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cond_op: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cond_threshold: Option<i64>,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&pfm_domain::AccountRule> for RuleDto {
    fn from(r: &pfm_domain::AccountRule) -> Self {
        Self {
            id: r.id.clone(),
            account_id: r.account_id.clone(),
            name: r.name.clone(),
            category_id: r.category_id.clone(),
            calc_type: r.calc_type.as_str().to_string(),
            value: r.value,
            is_annual: r.is_annual,
            prorata_days: r.prorata_days,
            annual_cap_cents: r.annual_cap_cents,
            cadence: r.cadence.as_str().to_string(),
            charge_day: r.charge_day,
            charge_month: r.charge_month,
            cond_base: r.cond_base.as_str().to_string(),
            cond_op: r.cond_op.map(|o| o.as_str().to_string()),
            cond_threshold: r.cond_threshold,
            active: r.active,
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
        }
    }
}

/// `POST /accounts/{id}/rules` request body. The account is the path parameter;
/// the body carries the rule parameters.
#[derive(Debug, Deserialize)]
pub struct RuleCreateRequest {
    pub name: String,
    pub category_id: String,
    pub calc_type: String,
    pub value: i64,
    #[serde(default)]
    pub is_annual: bool,
    #[serde(default)]
    pub prorata_days: bool,
    #[serde(default)]
    pub annual_cap_cents: Option<i64>,
    pub cadence: String,
    #[serde(default)]
    pub charge_day: Option<u32>,
    #[serde(default)]
    pub charge_month: Option<u32>,
    #[serde(default)]
    pub cond_base: Option<String>,
    #[serde(default)]
    pub cond_op: Option<String>,
    #[serde(default)]
    pub cond_threshold: Option<i64>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// `PUT /accounts/{id}/rules/{rid}` request body. All fields optional; `None`
/// = keep the current value. Nullable fields use a double `Option`: `None` =
/// unchanged, `Some(None)` = clear, `Some(Some(..))` = set.
#[derive(Debug, Default, Deserialize)]
pub struct RuleUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub calc_type: Option<String>,
    #[serde(default)]
    pub value: Option<i64>,
    #[serde(default)]
    pub is_annual: Option<bool>,
    #[serde(default)]
    pub prorata_days: Option<bool>,
    #[serde(default)]
    pub annual_cap_cents: Option<Option<i64>>,
    #[serde(default)]
    pub cadence: Option<String>,
    #[serde(default)]
    pub charge_day: Option<u32>,
    #[serde(default)]
    pub charge_month: Option<Option<u32>>,
    #[serde(default)]
    pub cond_base: Option<String>,
    #[serde(default)]
    pub cond_op: Option<Option<String>>,
    #[serde(default)]
    pub cond_threshold: Option<Option<i64>>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// `POST /accounts/{id}/rules/{rid}/generate` request body. Missing bounds fall
/// back to the account's full history.
#[derive(Debug, Default, Deserialize)]
pub struct RuleGenerateRequest {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// `POST /accounts/{id}/rules/preview` request body. When `rule_id` is absent
/// the account's active rules are previewed.
#[derive(Debug, Default, Deserialize)]
pub struct RulePreviewRequest {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub rule_id: Option<String>,
}

/// A single fee row in a generate/preview response.
#[derive(Debug, Serialize)]
pub struct FeeRowDto {
    pub date: String,
    pub cents: i64,
}

impl From<&pfm_domain::FeeRow> for FeeRowDto {
    fn from(r: &pfm_domain::FeeRow) -> Self {
        Self {
            date: r.date.format("%Y-%m-%d").to_string(),
            cents: r.amount_cents,
        }
    }
}

/// The fee rows a single rule would produce over a previewed horizon.
#[derive(Debug, Serialize)]
pub struct RulePreviewDto {
    pub rule_id: String,
    pub name: String,
    pub rows: Vec<FeeRowDto>,
}

// ─── Card DTOs ─────────────────────────────────────────────────────────────────

/// `POST /accounts/{id}/card/catch-up` request body.
#[derive(Debug, Default, Deserialize)]
pub struct CardChargeRequest {
    /// When true, only preview the charges and remaining limit without writing
    /// the settlement pairs.
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /accounts/{id}/card/catch-up` response body.
#[derive(Debug, Serialize)]
pub struct CardChargeDto {
    pub latest_charge_date: String,
    pub cycles_settled: usize,
    pub total_charge_cents: i64,
    /// The card's remaining limit in cents, or absent when the card has no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_cents: Option<i64>,
    /// Settlement pair ids that were written (empty when dry_run or no charges).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pair_ids: Vec<String>,
}

impl From<&pfm_service::CardCatchUpOutcome> for CardChargeDto {
    fn from(o: &pfm_service::CardCatchUpOutcome) -> Self {
        Self {
            latest_charge_date: o.latest_charge_date.format("%Y-%m-%d").to_string(),
            cycles_settled: o.cycles_settled,
            total_charge_cents: o.total_charge_cents,
            available_cents: o.available_cents,
            pair_ids: o.pair_ids.clone(),
        }
    }
}

/// `GET /cards/forecast` query.
#[derive(Debug, Deserialize)]
pub struct CardForecastQuery {
    pub from: String,
    pub to: String,
}

/// One expected card charge (a cycle not yet settled).
#[derive(Debug, Serialize)]
pub struct CardForecastDto {
    pub card_id: String,
    pub backing_id: String,
    pub closing_date: String,
    /// When the card's reset credit will post.
    pub card_date: String,
    /// When the backing account will be debited.
    pub backing_date: String,
    pub spend_cents: i64,
    pub fee_cents: i64,
}

impl From<&pfm_service::CardChargeForecast> for CardForecastDto {
    fn from(f: &pfm_service::CardChargeForecast) -> Self {
        let d = |x: chrono::NaiveDate| x.format("%Y-%m-%d").to_string();
        Self {
            card_id: f.card_id.clone(),
            backing_id: f.backing_id.clone(),
            closing_date: d(f.closing_date),
            card_date: d(f.card_date),
            backing_date: d(f.backing_date),
            spend_cents: f.spend_cents,
            fee_cents: f.fee_cents,
        }
    }
}

// ─── Deposit DTOs ──────────────────────────────────────────────────────────────

/// `GET /accounts/{id}/deposit/interest` query parameters.
#[derive(Debug, Deserialize)]
pub struct InterestQueryParams {
    pub from: String,
    pub to: String,
}

/// `GET /accounts/{id}/deposit/interest` response body.
#[derive(Debug, Serialize)]
pub struct InterestDto {
    pub gross_cents: i64,
    pub withholding_cents: i64,
    pub net_cents: i64,
}

impl From<&pfm_domain::InterestResult> for InterestDto {
    fn from(r: &pfm_domain::InterestResult) -> Self {
        Self {
            gross_cents: r.gross_cents,
            withholding_cents: r.withholding_cents,
            net_cents: r.net_cents,
        }
    }
}

/// `POST /accounts/{id}/deposit/consolidate` request body.
#[derive(Debug, Deserialize)]
pub struct ConsolidateRequest {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /accounts/{id}/deposit/consolidate` response body.
#[derive(Debug, Serialize)]
pub struct ConsolidateResponse {
    pub gross_cents: i64,
    pub withholding_cents: i64,
    pub net_cents: i64,
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<String>,
}

impl From<&pfm_service::DepositInterestOutcome> for ConsolidateResponse {
    fn from(o: &pfm_service::DepositInterestOutcome) -> Self {
        Self {
            gross_cents: o.gross_cents,
            withholding_cents: o.withholding_cents,
            net_cents: o.net_cents,
            from: o.from.format("%Y-%m-%d").to_string(),
            to: o.to.format("%Y-%m-%d").to_string(),
            pair_id: o.pair_id.clone(),
        }
    }
}

// ─── Stats DTOs ────────────────────────────────────────────────────────────────

/// Query parameters for `GET /stats`.
#[derive(Debug, Deserialize)]
pub struct StatsQueryParams {
    /// The account to compute statistics for.
    pub account: String,
    /// Explicit start (`YYYY-MM-DD`). Takes precedence over `period`; must be
    /// given together with `to`.
    #[serde(default)]
    pub from: Option<String>,
    /// Explicit end (`YYYY-MM-DD`). Takes precedence over `period`; must be
    /// given together with `from`.
    #[serde(default)]
    pub to: Option<String>,
    /// `month` (default) or `year`. Ignored when both `from` and `to` are given.
    #[serde(default)]
    pub period: Option<String>,
    /// Narrow the expenses-by-category breakdown to this category id.
    #[serde(default)]
    pub cat: Option<String>,
    /// Piggy-bank ids to fold into the account ledger (same semantics as the
    /// register). Applies to the expenses-by-category breakdown.
    #[serde(default)]
    pub vacct_included: VacctSelection,
}

/// KPI block in the stats response.
#[derive(Debug, Serialize)]
pub struct KpiDto {
    pub income_cents: i64,
    pub expense_cents: i64,
    pub net_cents: i64,
    pub opening_cents: i64,
    pub closing_cents: i64,
}

impl From<&pfm_service::KpiSummary> for KpiDto {
    fn from(k: &pfm_service::KpiSummary) -> Self {
        Self {
            income_cents: k.income_cents,
            expense_cents: k.expense_cents,
            net_cents: k.net_cents,
            opening_cents: k.opening_cents,
            closing_cents: k.closing_cents,
        }
    }
}

/// Total outflow in one category over the period.
#[derive(Debug, Serialize)]
pub struct CategorySpendDto {
    pub category_id: String,
    pub category_name: String,
    pub cents: i64,
}

impl From<&pfm_service::CategorySpend> for CategorySpendDto {
    fn from(c: &pfm_service::CategorySpend) -> Self {
        Self {
            category_id: c.category_id.clone(),
            category_name: c.category_name.clone(),
            cents: c.cents,
        }
    }
}

/// A single point on the balance trend.
#[derive(Debug, Serialize)]
pub struct BalancePointDto {
    pub date: String,
    pub balance_cents: i64,
    /// `true` when the point is a runtime projection (future date).
    pub is_forecast: bool,
}

impl From<&pfm_service::BalancePoint> for BalancePointDto {
    fn from(p: &pfm_service::BalancePoint) -> Self {
        Self {
            date: p.date.format("%Y-%m-%d").to_string(),
            balance_cents: p.balance_cents,
            is_forecast: p.is_forecast,
        }
    }
}

/// Per-category budget progress in the stats response.
#[derive(Debug, Serialize)]
pub struct BudgetDto {
    pub category_id: String,
    pub category_name: String,
    pub budget_cents: i64,
    pub spent_cents: i64,
    pub prev_period_cents: i64,
    pub pct: f64,
}

impl From<&pfm_domain::BudgetProgress> for BudgetDto {
    fn from(b: &pfm_domain::BudgetProgress) -> Self {
        Self {
            category_id: b.category_id.clone(),
            category_name: b.category_name.clone(),
            budget_cents: b.budget_cents,
            spent_cents: b.spent_cents,
            prev_period_cents: b.prev_period_cents,
            pct: b.pct,
        }
    }
}

/// `GET /stats` response body.
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub account_id: String,
    pub from: String,
    pub to: String,
    pub kpi: KpiDto,
    pub expenses_by_category: Vec<CategorySpendDto>,
    pub balance_trend: Vec<BalancePointDto>,
    pub budget: Vec<BudgetDto>,
}

// ─── Import DTOs ──────────────────────────────────────────────────────────────

/// `POST /import/preview` and `POST /import/commit` request body.
#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub account_id: String,
    pub bank_name: String,
    pub csv_content: String,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub date_format: Option<String>,
    #[serde(default)]
    pub decimal_sep: Option<String>,
    #[serde(default)]
    pub col_map: Option<std::collections::HashMap<String, String>>,
}

/// A single row in the import preview/commit response.
#[derive(Debug, Serialize)]
pub struct ImportedRowDto {
    pub date: String,
    pub cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub is_duplicate: bool,
    pub already_imported: bool,
    pub will_import: bool,
}

/// `POST /import/preview` response body.
#[derive(Debug, Serialize)]
pub struct ImportPreviewResponse {
    pub bank_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub total_rows: usize,
    pub duplicates: Vec<ImportedRowDto>,
    pub already_imported: Vec<ImportedRowDto>,
    pub to_import: Vec<ImportedRowDto>,
    pub skipped: usize,
}

impl From<&pfm_service::PreviewResult> for ImportPreviewResponse {
    fn from(r: &pfm_service::PreviewResult) -> Self {
        let to_dto = |rows: &[pfm_service::ImportedRow]| {
            rows.iter()
                .map(|row| ImportedRowDto {
                    date: row.date.clone(),
                    cents: row.cents,
                    description: row.description.clone(),
                    is_duplicate: row.is_duplicate,
                    already_imported: row.already_imported,
                    will_import: row.will_import,
                })
                .collect()
        };
        Self {
            bank_name: r.bank_name.clone(),
            profile_id: r.profile_id.clone(),
            total_rows: r.total_rows,
            duplicates: to_dto(&r.duplicates),
            already_imported: to_dto(&r.already_imported),
            to_import: to_dto(&r.to_import),
            skipped: r.skipped,
        }
    }
}

/// `POST /import/commit` response body.
#[derive(Debug, Serialize)]
pub struct ImportCommitResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub inserted: u64,
    pub skipped: u64,
    pub total_rows: usize,
}

impl From<&pfm_service::CommitResult> for ImportCommitResponse {
    fn from(r: &pfm_service::CommitResult) -> Self {
        Self {
            profile_id: r.profile_id.clone(),
            inserted: r.inserted,
            skipped: r.skipped,
            total_rows: r.total_rows,
        }
    }
}

/// An import profile sent to the client.
#[derive(Debug, Serialize)]
pub struct ImportProfileDto {
    pub id: String,
    pub account_id: String,
    pub bank_name: String,
    pub delimiter: String,
    pub date_format: String,
    pub decimal_sep: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col_map: Option<std::collections::HashMap<String, String>>,
    pub auto_cat: bool,
    pub skip_dup: bool,
    pub created_at: String,
}

impl From<&pfm_domain::ImportProfile> for ImportProfileDto {
    fn from(p: &pfm_domain::ImportProfile) -> Self {
        let col_map = p.col_map.as_ref().map(|map| {
            map.iter()
                .enumerate()
                .filter(|(_, role)| **role != pfm_domain::ColumnRole::Other)
                .map(|(i, role)| (i.to_string(), role.as_str().to_string()))
                .collect()
        });
        Self {
            id: p.id.clone(),
            account_id: p.account_id.clone(),
            bank_name: p.bank_name.clone(),
            delimiter: p.delimiter.to_string(),
            date_format: p.date_format.as_str().to_string(),
            decimal_sep: p.decimal_sep.as_str().to_string(),
            col_map,
            auto_cat: p.auto_cat,
            skip_dup: p.skip_dup,
            created_at: p.created_at.clone(),
        }
    }
}

/// `GET /import/profiles` response body.
#[derive(Debug, Serialize)]
pub struct ImportProfilesResponse {
    pub profiles: Vec<ImportProfileDto>,
}

// ─── Admin DTOs ────────────────────────────────────────────────────────────────

/// A user in the admin view (includes role and active status).
#[derive(Debug, Serialize)]
pub struct AdminUserDto {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub active: bool,
    pub locale: String,
    pub default_currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    pub created_at: String,
}

impl From<&pfm_domain::User> for AdminUserDto {
    fn from(u: &pfm_domain::User) -> Self {
        Self {
            id: u.id.clone(),
            username: u.username.clone(),
            email: u.email.clone(),
            role: u.role.as_str().to_string(),
            active: u.active,
            locale: u.locale.clone(),
            default_currency: u.default_currency.clone(),
            last_seen_at: u.last_seen_at.clone(),
            created_at: u.created_at.clone(),
        }
    }
}

/// `POST /users` request body.
#[derive(Debug, Deserialize)]
pub struct UserCreateRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub default_currency: Option<String>,
}

/// `PUT /users/{id}` request body. All fields optional.
#[derive(Debug, Default, Deserialize)]
pub struct UserUpdateRequest {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub default_currency: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// `DELETE /users/{id}` request body.
#[derive(Debug, Deserialize)]
pub struct DeleteUserRequest {
    /// If present, the user's accounts are reassigned to this target user id.
    /// If absent, deletion is blocked when the user has accounts.
    #[serde(default)]
    pub reassign_to: Option<String>,
}

/// `DELETE /admin/accounts/{id}` request body.
#[derive(Debug, Deserialize)]
pub struct DeleteAccountRequest {
    /// If present, the account's transactions are reassigned to this target
    /// account id. If absent, deletion is blocked when the account has
    /// transactions.
    #[serde(default)]
    pub reassign_tx_to: Option<String>,
}

/// An account in the admin view (includes owner info).
#[derive(Debug, Serialize)]
pub struct AdminAccountDto {
    pub id: String,
    pub owner_id: String,
    pub owner_username: String,
    pub name: String,
    pub kind: String,
    pub currency: String,
    pub active: bool,
}

impl From<&pfm_service::AdminAccountInfo> for AdminAccountDto {
    fn from(a: &pfm_service::AdminAccountInfo) -> Self {
        Self {
            id: a.id.clone(),
            owner_id: a.owner_id.clone(),
            owner_username: a.owner_username.clone(),
            name: a.name.clone(),
            kind: a.kind.clone(),
            currency: a.currency.clone(),
            active: a.active,
        }
    }
}

/// `POST /admin/impersonate/{uid}` response body.
#[derive(Debug, Serialize)]
pub struct ImpersonationResponse {
    pub impersonating: Option<String>,
}
