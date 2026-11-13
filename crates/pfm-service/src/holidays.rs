//! Holiday calendars as data: administrator CRUD on calendars and their rules,
//! plus resolving the calendar a given user selected.

use chrono::NaiveDate;
use pfm_domain::{HolidayCalendar, HolidayRule, RuleParts};
use pfm_repo::{CalendarInfo, HolidaysRepo, PrefsRepo, SqlxAuthRepo, SqlxHolidaysRepo, Storage};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HolidayError {
    #[error("calendar not found")]
    NotFound,
    #[error("a calendar with this code already exists")]
    Duplicate,
    #[error("calendar is selected by {0} user(s)")]
    InUse(i64),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

fn internal(e: impl std::fmt::Display) -> HolidayError {
    HolidayError::Internal(e.to_string())
}

pub struct HolidayService {
    repo: SqlxHolidaysRepo,
    auth: SqlxAuthRepo,
}

impl HolidayService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            repo: SqlxHolidaysRepo::new(storage),
            auth: SqlxAuthRepo::new(storage),
        }
    }

    fn parse_rules(parts: &[RuleParts]) -> Result<Vec<HolidayRule>, HolidayError> {
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                HolidayRule::from_parts(p)
                    .map_err(|e| HolidayError::Validation(format!("rule {}: {e}", i + 1)))
            })
            .collect()
    }

    pub async fn list(&self) -> Result<Vec<CalendarInfo>, HolidayError> {
        self.repo.list_calendars().await.map_err(internal)
    }

    pub async fn rules(&self, code: &str) -> Result<Vec<RuleParts>, HolidayError> {
        self.repo
            .get_calendar(code)
            .await
            .map_err(internal)?
            .ok_or(HolidayError::NotFound)?;
        self.repo.rules(code).await.map_err(internal)
    }

    /// The calendar for `code`; an unknown code behaves as weekends only.
    pub async fn load(&self, code: &str) -> HolidayCalendar {
        let Ok(parts) = self.repo.rules(code).await else {
            return HolidayCalendar::weekends_only();
        };
        // Rows that no longer validate are ignored rather than breaking every
        // recurrence of every user.
        HolidayCalendar::new(
            parts
                .iter()
                .filter_map(|p| HolidayRule::from_parts(p).ok())
                .collect(),
        )
    }

    /// The calendar the user selected in their preferences.
    pub async fn for_user(&self, user_id: &str) -> HolidayCalendar {
        match self.auth.get(user_id).await {
            Ok(p) => self.load(&p.holiday_calendar).await,
            Err(_) => HolidayCalendar::weekends_only(),
        }
    }

    /// Whether `code` names an existing calendar.
    pub async fn exists(&self, code: &str) -> Result<bool, HolidayError> {
        Ok(self
            .repo
            .get_calendar(code)
            .await
            .map_err(internal)?
            .is_some())
    }

    pub async fn create(&self, code: &str, name: &str) -> Result<(), HolidayError> {
        let code = code.trim();
        let name = name.trim();
        let valid_code = !code.is_empty()
            && code.len() <= 16
            && code
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !valid_code {
            return Err(HolidayError::Validation(
                "code must be 1-16 characters: lowercase letters, digits, '-' or '_'".into(),
            ));
        }
        if name.is_empty() || name.chars().count() > 100 {
            return Err(HolidayError::Validation(
                "name must be 1-100 characters".into(),
            ));
        }
        if self.exists(code).await? {
            return Err(HolidayError::Duplicate);
        }
        self.repo
            .create_calendar(code, name)
            .await
            .map_err(internal)
    }

    pub async fn rename(&self, code: &str, name: &str) -> Result<(), HolidayError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(HolidayError::Validation(
                "name must be 1-100 characters".into(),
            ));
        }
        if !self.exists(code).await? {
            return Err(HolidayError::NotFound);
        }
        self.repo
            .rename_calendar(code, name)
            .await
            .map_err(internal)
    }

    /// Delete a calendar; refused while any user has it selected.
    pub async fn delete(&self, code: &str) -> Result<(), HolidayError> {
        if !self.exists(code).await? {
            return Err(HolidayError::NotFound);
        }
        let n = self.repo.users_using(code).await.map_err(internal)?;
        if n > 0 {
            return Err(HolidayError::InUse(n));
        }
        self.repo.delete_calendar(code).await.map_err(internal)?;
        Ok(())
    }

    /// Replace the rule list of a calendar after validating every rule.
    pub async fn replace_rules(&self, code: &str, parts: &[RuleParts]) -> Result<(), HolidayError> {
        if !self.exists(code).await? {
            return Err(HolidayError::NotFound);
        }
        Self::parse_rules(parts)?;
        self.repo.replace_rules(code, parts).await.map_err(internal)
    }

    /// The holidays `parts` would produce in `year` (unsaved edits can be
    /// previewed), sorted by date.
    pub fn preview(
        parts: &[RuleParts],
        year: i32,
    ) -> Result<Vec<(NaiveDate, String)>, HolidayError> {
        Ok(HolidayCalendar::new(Self::parse_rules(parts)?).holidays_of_year(year))
    }
}
