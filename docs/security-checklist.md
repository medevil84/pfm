# Step 18 — Security Review Checklist

Date: 2026-09-07 (revised 2026-10-08 after the security audit — see section 11)
Status: ✅ Complete

---

## 1. HTTP Security Headers

| Header | Configured | Notes |
|--------|-----------|-------|
| `X-Content-Type-Options: nosniff` | ✅ | Injected on every response by `security_headers` middleware |
| `Referrer-Policy: strict-origin-when-cross-origin` | ✅ | Injected on every response |
| `Permissions-Policy: camera=(), microphone=(), geolocation=()` | ✅ | Injected on every response |
| `Content-Security-Policy` | ✅ | From `[security].csp`, plus `frame-ancestors 'none'; form-action 'self'; base-uri 'self'; object-src 'none'` appended by the middleware whenever the configured value lacks them (so an old `config.toml` is covered too) |
| `X-Frame-Options: DENY` | ✅ | Legacy twin of `frame-ancestors` |
| `Cache-Control` (API) | ✅ | `no-cache, no-store, must-revalidate` on **every** `/api/*` response (path-prefix based: a route added later is covered automatically) |
| `Cache-Control` (static) | ✅ | `public, max-age=86400` on static assets (Step 19) |
| `Strict-Transport-Security` | ✅ conditional | `max-age=31536000` only (no `includeSubDomains` / `preload`: the app may live under a path of a larger site). Present when cookies are `Secure` (`secure_cookies`, which defaults to `trust_proxy`); omitted in plain-HTTP mode |

**Implementation**: `crates/pfm-http/src/security_headers.rs` — tower middleware applied to the entire router.

**Tests**: `tests/security.rs` → `security_headers_present_on_api_response`, `hsts_present_when_trust_proxy`, `hsts_absent_when_not_trust_proxy`

---

## 2. X-Forwarded-* Handling (Trusted Proxy)

| Aspect | Status | Notes |
|--------|--------|-------|
| Client address is the peer **IP**, never `ip:port` | ✅ | The limiter key used to include the ephemeral source port, so every new connection got a fresh budget (fixed) |
| `X-Forwarded-For` ignored unless `trust_proxy = true` | ✅ | |
| `[security].trusted_proxies` | ✅ | Real IP / CIDR matching (IPv4 and IPv6, IPv4-mapped addresses canonicalised); invalid entries are logged and ignored |
| Header read **right to left** | ✅ | The rightmost entry that is not a trusted proxy wins; the leftmost one is client-controlled |
| Empty `trusted_proxies` + `trust_proxy = true` | ⚠ | Still accepted for development, but logs a warning at startup: any peer may then set its own address. Always list the proxy |
| Consistent across endpoints | ✅ | `login`, `logout`, `change_password` all use `resolve_client_ip` |

**Implementation**: `crates/pfm-config/src/lib.rs` → `resolve_client_ip(peer_ip, xff, &security)`

**Tests**: `pfm-config` unit tests (`ip_tests`), `tests/security.rs`, `tests/hardening.rs` → `login_throttle_does_not_depend_on_the_source_port`

---

## 3. CSRF Protection

| Aspect | Status | Notes |
|--------|--------|-------|
| Double-submit cookie pattern | ✅ | Token stored in session + sent as `X-CSRF-Token` header |
| Every method except `GET`/`HEAD`/`OPTIONS` requires the token | ✅ | An allow-list of *safe* verbs, so PATCH or a verb added later cannot skip the check |
| Token compared in constant time | ✅ | |
| Exempt paths | ✅ | `/auth/login` (no session yet; it additionally requires `Content-Type: application/json`, which a cross-site form cannot send) and `/api/setup/*` (guarded by the setup token instead) |
| Missing token → 400 | ✅ | `csrf_missing_header_rejected` test |
| Mismatched token → 403 | ✅ | `csrf_mismatch_rejected` test |
| Token regenerated on login | ✅ | Prevents token fixation |
| Session cookie is `HttpOnly`, `SameSite=Lax` | ✅ | `build_session_layer`. The **CSRF cookie is not HttpOnly, by design**: the double-submit pattern needs the SPA to read it and echo it in `X-CSRF-Token` |

**Implementation**: `crates/pfm-http/src/csrf.rs`

**Tests**: `tests/auth.rs` → `csrf_correct_token_accepted`, `csrf_missing_header_rejected`, `csrf_mismatch_rejected`, `csrf_mismatch_rejected`, `login_is_csrf_exempt`

---

## 4. Permissions (Always in Query/Service Layer)

| Aspect | Status | Notes |
|--------|--------|-------|
| Account ownership/share check in SQL | ✅ | `WHERE account_id IN (SELECT ...)` subquery pattern |
| Write permission check in service | ✅ | Explicit `can_write` check before mutation |
| Admin can access all accounts | ✅ | Role-based bypass in service layer |
| Unshared user cannot read others' data | ✅ | Tested: `unshared_user_cannot_read_others_account` |
| Unshared user cannot write others' data | ✅ | Tested: `unshared_user_cannot_write_others_account` |
| Transaction/transfer permission scoped | ✅ | Tested: `unauthorized_user_cannot_read_register`, `unauthorized_user_cannot_create_transaction` |
| Bulk operations check all IDs | ✅ | Tested: `bulk_delete_mixed_permissions_rejected` |
| Export respects share permissions | ✅ | Tested: `export_forbidden_for_unshared_user` |
| Recurrences respect share permissions | ✅ | Tested: `unshared_user_cannot_read_rule`, `unshared_user_cannot_override_rule` |
| Rules respect share permissions | ✅ | Tested: `unshared_user_cannot_read_rules`, `unshared_user_cannot_create_rule` |
| Admin impersonation writes correct actor | ✅ | Tested: `impersonation_writes_audit_with_correct_actor_and_impersonated` |
| Ids supplied in a request body are authorised too | ✅ | `primary_account_id` (card / deposit), `linked_account_id` and the *current* account of a recurrence, both ends of a transfer being moved — each needs write access (`tests/hardening.rs`) |
| `category_id` must exist and be usable by the caller | ✅ | Transactions, bulk recategorise, recurrences, fee rules: 422 instead of a foreign-key 500 or a cross-user reference |
| Categories and tags are per-user | ✅ | Owned by the creator; ownerless (seeded / pre-migration) rows are shared and admin-managed; collaborators (users who share an account) see each other's. Deleting a category used by a recurrence or rule is refused (the database would cascade) |
| `client_id` idempotency does not cross accounts | ✅ | A reused id on another account is a 422, not the other account's row |
| Import profile list limited to visible accounts | ✅ | |

**Tests**: Across `tests/accounts.rs`, `tests/transactions.rs`, `tests/transfers.rs`, `tests/recurrences.rs`, `tests/rules.rs`, `tests/export.rs`, `tests/admin.rs`

---

## 5. Audit Log

| Event | AuditAction | Written | Test |
|-------|-------------|---------|------|
| Login success | `Login` | ✅ | `audit_log_written_on_login` |
| Login failure | `LoginFailed` | ✅ | `audit_log_written_on_failed_login` |
| Logout | `Logout` | ✅ | (implicit in logout flow) |
| Password change | `PasswordChanged` | ✅ | (implicit in change_password flow) |
| Account created | `AccountCreated` | ✅ | (service layer) |
| Account deleted (self) | `AccountSelfDeleted` | ✅ | (service layer, Step 18 addition) |
| Account shared | `AccountShared` | ✅ | (service layer, Step 18 addition) |
| Account unshared | `AccountUnshared` | ✅ | (service layer, Step 18 addition) |
| Import committed | `ImportCommitted` | ✅ | (service layer, Step 18 addition) |
| Admin impersonation | `Impersonation` | ✅ | (service layer) |
| User created/updated/deleted | `UserCreated` / `UserUpdated` / `UserDeleted` | ✅ | (service layer) |
| Admin account delete (with reassignment) | `AccountDeleted` | ✅ | (service layer) |

**Fields captured**: `actor_id`, `impersonated_id` (optional), `entity_type`, `entity_id`, `ip_address`, `user_agent`, `timestamp`

**Implementation**: `crates/pfm-domain/src/audit.rs` (builder pattern), written via `AuditRepo` trait in services.

---

## 6. Input Validation

| Field | Validation | Where |
|-------|-----------|-------|
| `username` | Length 1–64, non-empty | `validate::check_len` |
| `password` (login) | Max length 128 | `validate::check_password` (login: max only) |
| `password` (new) | Length 8–128 | `validate::check_password` |
| `email` | RFC 5322 simplified, max 254 chars | `validate::check_email` |
| `name` (account/category) | Length 1–128 | `validate::check_len` |
| `note` | Length ≤ 1024 | `validate::check_len_opt` |
| `description` (transaction) | Length ≤ 512 | `validate::check_len_opt` |
| `iban` | 15–34 chars, country code valid, alphanumeric | `validate::check_iban` |
| `cents` | Absolute value ≤ 100,000,000,000 (±1B EUR) | `validate::check_cents` |
| `date` | Valid `YYYY-MM-DD` via `NaiveDate::parse` | In each handler |
| `currency` | 3-char ASCII uppercase | In `to_create_cmd` |
| `kind` (enum) | Must parse to valid variant | In each handler |
| JSON body | Max 1 MiB, must parse | `axum::body::to_bytes` + `serde_json` |

**Implementation**: `crates/pfm-http/src/validate.rs` (helpers), applied in route handlers before service calls.

---

## 7. Rate Limiting and Password Hashing

| Aspect | Status | Notes |
|--------|--------|-------|
| Login rate limited | ✅ | Sliding window, 5 attempts per 60s per (IP, username) **and** 6× that per IP across all usernames (password spraying) |
| Key is the resolved client IP (no port) | ✅ | See section 2 |
| Limiter memory is bounded | ✅ | Pruned past 1024 keys, hard cap of 50,000 keys (fails closed for new keys) |
| Returns 429 | ✅ | `ApiError::too_many_requests()` |
| Argon2 runs off the async workers | ✅ | `spawn_blocking`, at most 2 hashes in parallel, then 429 after a 10 s queue timeout — 64 MiB per hash would otherwise let parallel logins exhaust memory |
| Unknown user costs the same as a wrong password | ✅ | A dummy hash with the configured parameters is verified |
| "Account disabled" only after the password verified | ✅ | Otherwise it confirmed that the account exists |
| In-memory (single instance) | ✅ | Adequate for single-process deployment |

**Tests**: `tests/auth.rs` → `login_rate_limited_after_threshold`; `tests/hardening.rs` → `login_throttle_*`, `disabled_account_is_indistinguishable_without_the_password`

---

## 8. Dependency Audit (cargo audit)

**Command**: `cargo audit`
**Date**: 2026-09-07
**Result**: ✅ **0 vulnerabilities** across 313 crate dependencies
**Advisory DB**: RustSec (https://github.com/RustSec/advisory-db.git), 1240 advisories loaded
**Cargo.lock**: Pinned and committed

---

## 9. Session Security

| Aspect | Status | Notes |
|--------|--------|-------|
| Session cookie `HttpOnly` | ✅ | Not accessible to JavaScript |
| Cookie `SameSite=Lax` | ✅ | Prevents cross-site form submission |
| Cookie `Secure` | ✅ when `secure_cookies` (default: follows `trust_proxy`) | Set `secure_cookies = true` behind an HTTPS proxy |
| Session store: in-memory | ✅ | The cookie carries only a signed session id. A leaked `session_secret` alone does not forge a login |
| CSRF token separate from session ID | ✅ | 64-char hex random |
| Session re-validated on every `/api` request | ✅ | `session_guard`: a disabled or deleted user, or one whose password hash changed, is logged out (401). A password change keeps only the session that made it |
| Session id cycled at login, flushed at logout | ✅ | |

---

## 10. Additional Hardening

| Item | Status | Notes |
|------|--------|-------|
| Argon2id for password hashing | ✅ | OWASP-recommended parameters (m=64MB, t=3, p=4) |
| No internal detail in error bodies | ✅ | Internal errors are logged server-side and answered with the stable `errors.internal` code only; setup endpoints do the same |
| No user data in URLs | ✅ | IDs in path params are internal UUIDs |
| JSON errors (not HTML) for API | ✅ | All `/api/*` errors are `application/json` |
| User data in `innerHTML` is escaped | ✅ | The SPA builds markup with template strings: every user-controlled value goes through `esc()` (now also escaping `'`); `t()` does **not** escape, so parameters must be wrapped. Icons / emoji are validated server-side (`[A-Za-z0-9_-]` / printable, no markup characters) and sanitised again in the client |
| CSV export defuses formulas | ✅ | Text columns starting with `= + - @ TAB CR` get a leading `'` |
| Static files cannot escape the web root | ✅ | `DiskFallback` rejects `..`, absolute and drive-prefixed components and canonicalises under the root (`safe_join`, unit-tested incl. symlinks) |
| Setup wizard is token-gated | ✅ | `/api/setup/*` needs the one-time `X-Setup-Token` printed on the console; the regular API is not mounted in setup mode; `complete` refuses an already-configured database |
| Offline queue is per user | ✅ | Items are stamped with the user id and only replayed for that user; nothing is replayed while logged out |

---

## 11. Security audit of 2026-10 — findings and fixes

Each item below has a regression test in `crates/pfm-http/tests/hardening.rs`
(unless noted) that fails on the pre-audit code.

| # | Finding | Severity | Fix |
|---|---------|----------|-----|
| 1 | A card / deposit could be backed by *any* account id; settlements then posted onto the victim's account | High | `primary_account_id` needs write access (create and update) |
| 2 | `PUT /transfers/{pair}` checked only the destination accounts: a read-only share could move / destroy a transfer | High | Write access also required on the current accounts |
| 3 | `PUT /recurrences/{id}` checked only the new account: anyone knowing the id could take the rule; `linked_account_id` unchecked | High | Write required on current, new and linked accounts |
| 4 | `--setup` mode: unauthenticated reset / complete, regular API mounted, bound to all interfaces by `pfm-setup` | High (while the wizard runs) | One-time console token, API unmounted in setup mode, `complete` refuses a configured DB, no error text leaked |
| 5 | Login throttle keyed on `ip:port` (new connection = new budget), trusted-proxy matching compared strings, leftmost `X-Forwarded-For` trusted | High | Peer IP only, real CIDR matching, rightmost untrusted XFF, per-IP spray cap |
| 6 | Static file server path traversal (`source = "disk"`) | High if disk mode is reachable | `safe_join` (unit-tested), also covers Windows prefixes and symlinks (`pfm-app` unit tests) |
| 7 | Disabled / deleted user or reset password kept a working session | Medium | `session_guard` revalidates every `/api` request; password hash fingerprint |
| 8 | Any user could rename / delete shared categories and tags (delete cascades to recurrences and rules) | Medium | Per-user ownership (migration `0020`); ownerless rows are admin-managed; delete refused while in use |
| 9 | Username enumeration (timing; "disabled" answered before the password) | Medium | Dummy hash verified for unknown users; state reported only after the password |
| 10 | Argon2 on async workers with no concurrency cap (memory exhaustion) | Medium (inferred) | `spawn_blocking` + 2-slot semaphore, 429 on queue timeout |
| 11 | Internal errors (SQL text) returned to the client; `category_id` unvalidated (500) | Low | Logged server-side only; ids validated (422) |
| 12 | `/api/tags`, `/cards`, `/holiday-calendars` answered `Cache-Control: public, max-age=86400` | Low–Medium | All `/api` responses are `no-store` |
| 13 | HTML injection: `t()` parameters, category emoji, account icon rendered into markup unescaped; CSP lacked `frame-ancestors` / `form-action` / `base-uri`; no `X-Frame-Options` | Medium | Escaping, server-side validation, CSP directives enforced by middleware, `X-Frame-Options: DENY` (verified by code and unit/integration tests, not in a browser) |
| 14 | CSV export formula injection | Medium | Leading `'` on text cells |
| 15 | Offline queue survived logout and replayed under the next user | Low | Per-user stamp, no replay while logged out (no automated test: frontend only) |
| 16 | Login CSRF via `text/plain` form; CSRF check skipped non-POST/PUT/DELETE verbs; non-constant-time compare | Low | JSON content type required; every non-read verb checked; constant-time compare |
| 17 | `client_id` idempotency returned another account's row; `by-pair` and import-profile listing exposed foreign data | Low | Conflict instead of the row; all legs readable; visible accounts only |
| 18 | `pfm-backup` ran `gzip` / `chmod` / `sqlite3` as root inside a directory writable by the app user | Hardening | Those steps run as `pfm`; upload refuses non-regular files (shell syntax-checked only, not run) |
| 19 | nginx sample forwarded a client-extendable `X-Forwarded-For` | Hardening | `$remote_addr` (sample config, not exercised against a real nginx) |

### Residual risk / operator actions

* **Redeploy.** The binary in `target/release/` and `deploy/alpine/dist/*.tar.gz` pre-date these fixes and must be rebuilt. Migration `0020` runs on the next start; it is one-way (tag tables are rebuilt), so back up the database first.
* **Edit `/etc/pfm/config.toml` by hand** (the installer never overwrites it): set `trusted_proxies` to the proxy address, `trust_proxy = true` and `secure_cookies = true` when behind HTTPS, and firewall port 9999 to the proxy. The startup log warns about an empty `trusted_proxies`.
* Existing categories and tags become *shared*: only administrators can change them. Users create their own from now on.
* The login limiter caps its memory at 50,000 keys and then refuses *new* keys until entries expire; IPv6 clients are keyed by /64 so an attacker needs 50,000 distinct subnets to trigger it.
* Administrators browse the same category / tag lists as everyone (own, shared, collaborators'); they can still open, edit or delete any item by id.
* Looking up a share target by e-mail (`POST /accounts/{id}/shares`) still tells an account owner whether an address is registered. This is inherent to sharing by e-mail and was left as is.
* `style-src 'unsafe-inline'` remains in the CSP (the SPA uses inline styles).
* The MariaDB path of migration `0020` could not be executed here (SQLite only); it uses only `ADD COLUMN`, `CREATE TABLE`, `INSERT … SELECT`, `DROP TABLE` and `RENAME`.

---

## Gate Summary

| Gate | Result |
|------|--------|
| `cargo check` | ✅ |
| `cargo test --workspace` (510 tests, 24 of them the new regression suite) | ✅ |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ |
| `cargo fmt --check` | ✅ |
| `node scripts/frontend_test.js` | ✅ |
| `cargo audit` | not re-run in the 2026-10 audit (dependencies unchanged except `tokio` now also used by `pfm-service`) |
| Security checklist | ✅ (this document) |
