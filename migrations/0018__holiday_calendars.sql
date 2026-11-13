-- 0018__holiday_calendars.sql
-- Holiday calendars as data, editable by an administrator. A calendar is an
-- ordered list of rules; the application evaluates them (see pfm-domain
-- calendar.rs). Rule order matters only for "next_free" observance.
--   kind        fixed | easter | nth_weekday | date
--   weekday     1 = Monday .. 7 = Sunday
--   nth         1..5, or -1 for the last one in the month
--   observance  none | nearest | next_free (fixed rules)
-- The seeded calendars below are national holidays only; edit or extend them
-- from the admin settings.

CREATE TABLE IF NOT EXISTS holiday_calendars (
    code       VARCHAR(16)  NOT NULL PRIMARY KEY,
    name       VARCHAR(100) NOT NULL,
    created_at VARCHAR(64)  NOT NULL
);

CREATE TABLE IF NOT EXISTS holiday_rules (
    id            VARCHAR(64)  NOT NULL PRIMARY KEY,
    calendar_code VARCHAR(16)  NOT NULL,
    sort_order    INTEGER      NOT NULL,
    label         VARCHAR(100) NOT NULL,
    kind          VARCHAR(16)  NOT NULL,
    month         INTEGER      NULL,
    day           INTEGER      NULL,
    offset_days   INTEGER      NULL,
    weekday       INTEGER      NULL,
    nth           INTEGER      NULL,
    observance    VARCHAR(16)  NULL,
    on_date       VARCHAR(10)  NULL,
    FOREIGN KEY (calendar_code) REFERENCES holiday_calendars (code) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_holiday_rules_calendar ON holiday_rules (calendar_code, sort_order);

INSERT INTO holiday_calendars (code, name, created_at) VALUES ('none', 'Nessuno (solo sabato e domenica)', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('it', 'Italia', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('de', 'Germania', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('fr', 'Francia', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('es', 'Spagna', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('gb', 'Regno Unito (Inghilterra e Galles)', '2026-01-01T00:00:00Z');
INSERT INTO holiday_calendars (code, name, created_at) VALUES ('us', 'Stati Uniti (federali)', '2026-01-01T00:00:00Z');

INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-01', 'it', 1, 'Capodanno', 'fixed', 1, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-02', 'it', 2, 'Epifania', 'fixed', 1, 6, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-it-03', 'it', 3, 'Lunedì dell''Angelo', 'easter', 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-04', 'it', 4, 'Festa della Liberazione', 'fixed', 4, 25, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-05', 'it', 5, 'Festa del Lavoro', 'fixed', 5, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-06', 'it', 6, 'Festa della Repubblica', 'fixed', 6, 2, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-07', 'it', 7, 'Ferragosto', 'fixed', 8, 15, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-08', 'it', 8, 'Ognissanti', 'fixed', 11, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-09', 'it', 9, 'Immacolata Concezione', 'fixed', 12, 8, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-10', 'it', 10, 'Natale', 'fixed', 12, 25, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-it-11', 'it', 11, 'Santo Stefano', 'fixed', 12, 26, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-de-01', 'de', 1, 'Neujahr', 'fixed', 1, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-de-02', 'de', 2, 'Karfreitag', 'easter', -2);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-de-03', 'de', 3, 'Ostermontag', 'easter', 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-de-04', 'de', 4, 'Tag der Arbeit', 'fixed', 5, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-de-05', 'de', 5, 'Christi Himmelfahrt', 'easter', 39);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-de-06', 'de', 6, 'Pfingstmontag', 'easter', 50);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-de-07', 'de', 7, 'Tag der Deutschen Einheit', 'fixed', 10, 3, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-de-08', 'de', 8, 'Erster Weihnachtstag', 'fixed', 12, 25, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-de-09', 'de', 9, 'Zweiter Weihnachtstag', 'fixed', 12, 26, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-01', 'fr', 1, 'Jour de l''An', 'fixed', 1, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-fr-02', 'fr', 2, 'Lundi de Pâques', 'easter', 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-03', 'fr', 3, 'Fête du Travail', 'fixed', 5, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-04', 'fr', 4, 'Victoire 1945', 'fixed', 5, 8, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-fr-05', 'fr', 5, 'Ascension', 'easter', 39);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-fr-06', 'fr', 6, 'Lundi de Pentecôte', 'easter', 50);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-07', 'fr', 7, 'Fête nationale', 'fixed', 7, 14, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-08', 'fr', 8, 'Assomption', 'fixed', 8, 15, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-09', 'fr', 9, 'Toussaint', 'fixed', 11, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-10', 'fr', 10, 'Armistice 1918', 'fixed', 11, 11, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-fr-11', 'fr', 11, 'Noël', 'fixed', 12, 25, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-01', 'es', 1, 'Año Nuevo', 'fixed', 1, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-02', 'es', 2, 'Epifanía del Señor', 'fixed', 1, 6, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-es-03', 'es', 3, 'Viernes Santo', 'easter', -2);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-04', 'es', 4, 'Fiesta del Trabajo', 'fixed', 5, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-05', 'es', 5, 'Asunción de la Virgen', 'fixed', 8, 15, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-06', 'es', 6, 'Fiesta Nacional de España', 'fixed', 10, 12, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-07', 'es', 7, 'Todos los Santos', 'fixed', 11, 1, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-08', 'es', 8, 'Día de la Constitución', 'fixed', 12, 6, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-09', 'es', 9, 'Inmaculada Concepción', 'fixed', 12, 8, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-es-10', 'es', 10, 'Navidad', 'fixed', 12, 25, 'none');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-gb-01', 'gb', 1, 'New Year''s Day', 'fixed', 1, 1, 'next_free');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-gb-02', 'gb', 2, 'Good Friday', 'easter', -2);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, offset_days) VALUES ('seed-gb-03', 'gb', 3, 'Easter Monday', 'easter', 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-gb-04', 'gb', 4, 'Early May bank holiday', 'nth_weekday', 5, 1, 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-gb-05', 'gb', 5, 'Spring bank holiday', 'nth_weekday', 5, 1, -1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-gb-06', 'gb', 6, 'Summer bank holiday', 'nth_weekday', 8, 1, -1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-gb-07', 'gb', 7, 'Christmas Day', 'fixed', 12, 25, 'next_free');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-gb-08', 'gb', 8, 'Boxing Day', 'fixed', 12, 26, 'next_free');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-us-01', 'us', 1, 'New Year''s Day', 'fixed', 1, 1, 'nearest');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-02', 'us', 2, 'Martin Luther King Jr. Day', 'nth_weekday', 1, 1, 3);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-03', 'us', 3, 'Presidents'' Day', 'nth_weekday', 2, 1, 3);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-04', 'us', 4, 'Memorial Day', 'nth_weekday', 5, 1, -1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-us-05', 'us', 5, 'Juneteenth', 'fixed', 6, 19, 'nearest');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-us-06', 'us', 6, 'Independence Day', 'fixed', 7, 4, 'nearest');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-07', 'us', 7, 'Labor Day', 'nth_weekday', 9, 1, 1);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-08', 'us', 8, 'Columbus Day', 'nth_weekday', 10, 1, 2);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-us-09', 'us', 9, 'Veterans Day', 'fixed', 11, 11, 'nearest');
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, weekday, nth) VALUES ('seed-us-10', 'us', 10, 'Thanksgiving', 'nth_weekday', 11, 4, 4);
INSERT INTO holiday_rules (id, calendar_code, sort_order, label, kind, month, day, observance) VALUES ('seed-us-11', 'us', 11, 'Christmas Day', 'fixed', 12, 25, 'nearest');
