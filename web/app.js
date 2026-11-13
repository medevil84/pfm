/* ============================================================
   PFM — App (frontend collegato alle API REST)
   Nessun dato finto: ogni valore arriva dal backend.
   Il markup generato qui deve restare quello del mockup
   (mockup_gestionale_finanza_v11.html): il CSS è il suo.
   ============================================================ */

/* ---------- costanti e utilità ---------- */
const API = '/api';
/* Lingue disponibili nel selettore della pagina Profilo.
   Per aggiungerne una: creare web/i18n/<code>.json (stesse chiavi di it.json)
   e aggiungere qui una voce {code, label nativa, intl, date}. `intl` è il tag
   Intl per i numeri; `date` è l'ordine dei campi in fmtDate ('dmy' | 'mdy').
   Per en si usa en-US sia per i numeri (1,234.56) sia per le date (mm/dd/yyyy):
   un solo formato coerente per l'inglese, invece di mischiare en-GB e en-US. */
const LANGS = [
  {code: 'it', label: 'Italiano', intl: 'it-IT', date: 'dmy'},
  {code: 'en', label: 'English', intl: 'en-US', date: 'mdy'},
];
let LANG = LANGS[0];
let nf = new Intl.NumberFormat(LANG.intl, {minimumFractionDigits: 2, maximumFractionDigits: 2});
let nfInput = new Intl.NumberFormat(LANG.intl, {minimumFractionDigits: 2, maximumFractionDigits: 2, useGrouping: false});
/* Da chiamare a ogni cambio lingua: ricrea i formattatori numerici. */
function setFormatLocale(code) {
  LANG = LANGS.find(l => l.code === code) || LANGS[0];
  nf = new Intl.NumberFormat(LANG.intl, {minimumFractionDigits: 2, maximumFractionDigits: 2});
  nfInput = new Intl.NumberFormat(LANG.intl, {minimumFractionDigits: 2, maximumFractionDigits: 2, useGrouping: false});
  refreshDateFields();
}
/* Valore per un <input> importo (o percentuale: stessa scala, /100), nel formato attivo e senza separatori migliaia. */
const formatAmountInput = cents => nfInput.format((cents || 0) / 100);
const money = (c, sign = false) => S.privacy ? '€ •••••'
  : (sign && c > 0 ? '+' : c < 0 ? '−' : '') + '€ ' + nf.format(Math.abs(c) / 100);
/* Restituisce il valore assoluto in centesimi: il segno lo decide sempre
   il chiamante (selettore uscita/entrata). Vedi B1 dell'audit.
   Nessuna tolleranza: it '.' è sempre migliaia e ',' sempre decimale;
   en ',' è sempre migliaia e '.' sempre decimale. */
const parseAmount = s => {
  if (!s) return 0;
  let clean = String(s).replace(/[^\d,.\-]/g, '');
  clean = LANG.code === 'it'
    ? clean.replace(/\./g, '').replace(',', '.')
    : clean.replace(/,/g, '');
  const v = parseFloat(clean);
  return isNaN(v) ? 0 : Math.round(Math.abs(v) * 100);
};
/* Stesse regole di parseAmount (it: ',' decimale, '.' migliaia; en: '.' decimale, ',' migliaia). */
const parsePct = s => {
  let c = String(s ?? '').replace(/[^\d,.\-]/g, '');
  c = LANG.code === 'it' ? c.replace(/\./g, '').replace(',', '.') : c.replace(/,/g, '');
  const v = parseFloat(c);
  return isNaN(v) ? 0 : v;
};
const iso = d => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
const fromISO = s => { const [y, m, d] = s.split('-').map(Number); return new Date(y, m - 1, d); };
const addDays = (s, n) => { const d = fromISO(s); d.setDate(d.getDate() + n); return iso(d); };
const som = (y, m) => iso(new Date(y, m, 1));
const eom = (y, m) => iso(new Date(y, m + 1, 0));
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));
const TODAY = iso(new Date());
const isWeekend = s => [0, 6].includes(fromISO(s).getDay());
const isMobile = () => window.innerWidth <= 768;

/* ---------- i18n ---------- */
let I18N = {};
let I18N_LOCALE = 'it';

async function loadI18n(locale) {
  const wanted = (locale || 'it').slice(0, 2);
  try {
    const res = await fetch(`/i18n/${wanted}.json`, {cache: 'no-cache'});  // il server dà max-age=86400: senza revalidazione un catalogo vecchio resta in cache
    if (!res.ok) throw new Error('missing catalog');
    I18N = await res.json();
    I18N_LOCALE = wanted;
    setFormatLocale(wanted);
  } catch {
    if (wanted !== 'it') return loadI18n('it');
    I18N = {};
  }
}

function t(key, params = {}) {
  let s = Object.prototype.hasOwnProperty.call(I18N, key) ? I18N[key] : key;
  for (const [k, v] of Object.entries(params)) s = s.split(`{${k}}`).join(v);
  return s;
}

/** Translate known system-generated descriptions to the active locale. */
const txDesc = (desc) => {
  if (!desc) return '';
  if (desc === 'card settlement') return t('tx.card_settlement');
  return desc;
};

const DOW = () => [0, 1, 2, 3, 4, 5, 6].map(i => t(`dow.${i}`));
const MONTHS = () => [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map(i => t(`month.${i}`));
const dowOf = s => DOW()[fromISO(s).getDay()];
const fmtDate = s => {
  const d = fromISO(s);
  const dd = String(d.getDate()).padStart(2, '0'), mm = String(d.getMonth() + 1).padStart(2, '0');
  return LANG.date === 'mdy' ? `${mm}/${dd}/${d.getFullYear()}` : `${dd}/${mm}/${d.getFullYear()}`;
};
const fmtDayLong = s => { const d = fromISO(s); return `${dowOf(s)} ${d.getDate()} ${MONTHS()[d.getMonth()]}`; };

/* ---------- stato ---------- */
const S = {
  logged: false,
  page: 'registro',
  accountId: 'all',
  month: TODAY.slice(0, 7),
  anchor: TODAY,
  period: 'thisMonth',
  range: {k1: TODAY.slice(0, 7), k2: TODAY.slice(0, 7)},
  winRange: null,
  win: {from: 0, to: 0},
  extraAbove: 0, extraBelow: 0,
  rows: isMobile() ? {above: 7, below: 14, step: 10} : {above: 15, below: 30, step: 15},
  showEmpty: true,
  showFuture: true,
  showSign: true,
  showMSaldo: false,
  privacy: false,
  theme: 'auto',
  holidayCalendar: 'it',
  colorPos: '',   // '#rrggbb' oppure '' = predefinito (verde)
  colorNeg: '',   // '#rrggbb' oppure '' = predefinito (rosso)
  holCals: [],
  hol: null,
  search: '',
  filters: {cat: '', tipo: '', from: '', to: '', min: '', max: ''},
  selMode: false,
  selected: new Set(),
  tags: [],
  tagMap: {},
  quickRow: false,
  quickFocus: false,
  virtualOn: new Set(),
  hlTx: null, hlRec: null,
  trEditing: null,
  trPeriod: 'around',
  setTab: 'vis',
  user: null,
  realUser: null,
  impersonating: null,
  csrfToken: '',
  accounts: [],
  categories: [],
  recurrences: [],
  users: [],
  adminAccounts: [],
  transfers: [],
  bal: {},              // accountId -> {real, allocated}
  fees: {},             // accountId -> [{nome, date, cents, dovuto}, ...]
  stats: {
    acct: 'all', k1: TODAY.slice(0, 7), k2: TODAY.slice(0, 7), from: '', to: '',
    search: '', cats: new Set(), tags: new Set(), tipo: '', min: '', max: '',
    forecast: true, transfers: false, gran: 'auto',
    balSplit: '0', pieBy: 'cat', pieKind: 'out', trendBy: 'cat', trendKind: 'out', trendMode: 'line', topKind: 'out',
  },
  interest: {},         // accountId -> InterestDto, KPI dei depositi
  entry: {tipo: 'uscita', cat: null, date: TODAY, acct: null, freq: 'monthly', tags: new Set()},
  loading: 0,
};

/* Registro normalizzato: array di giorni
   {key, empty, isToday, isFuture, saldoFine, entries:[…]} */
let LED = [];
let LED_OPENING = 0;

/* ---------- livello API ---------- */
class ApiError extends Error {
  constructor(code, detail, status) { super(detail || code); this.code = code; this.detail = detail; this.status = status; }
}

function getCsrfCookie() {
  const m = document.cookie.match(/(?:^|;\s*)pfm_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]) : null;
}

/* Indicatore di caricamento globale: conta le richieste in volo così che
   richieste sovrapposte non spengano la barra troppo presto. */
function setBusy(on) {
  S.loading = Math.max(0, S.loading + (on ? 1 : -1));
  document.body.classList.toggle('is-loading', S.loading > 0);
}

async function api(path, options = {}) {
  const method = options.method || 'GET';
  const headers = {};
  if (options.body !== undefined) headers['Content-Type'] = 'application/json';
  if (!['GET', 'HEAD', 'OPTIONS'].includes(method)) {
    const token = getCsrfCookie() || S.csrfToken;
    if (token) headers['X-CSRF-Token'] = token;
  }
  let res;
  setBusy(true);
  try {
    res = await fetch(API + path, {
      ...options,
      credentials: 'same-origin',
      headers: {...headers, ...(options.headers || {})},
    });
  } catch {
    throw new ApiError('errors.network', t('errors.network'), 0);
  } finally {
    setBusy(false);
  }
  if (res.status === 401) {
    let body = {};
    try { body = await res.json(); } catch { /* corpo non JSON */ }
    /* Password errata al login: non è una sessione scaduta, mostra il motivo vero. */
    if (body.code === 'errors.invalid_credentials') {
      throw new ApiError(body.code, body.detail || t(body.code), 401, true);
    }
    handleUnauthorized();
    throw new ApiError('errors.not_authenticated', t('errors.not_authenticated'), 401);
  }
  if (!res.ok) {
    let err = {};
    try { err = await res.json(); } catch { /* corpo non JSON */ }
    throw new ApiError(err.code || 'errors.internal', err.detail || res.statusText, res.status);
  }
  if (res.status === 204) return null;
  const ct = res.headers.get('content-type') || '';
  if (!ct.includes('json')) return null;
  return res.json();
}

function apiErrorToMessage(err) {
  if (err instanceof ApiError) {
    if (Object.prototype.hasOwnProperty.call(I18N, err.code)) return t(err.code);
    return err.detail || err.code;
  }
  return err?.message || t('errors.internal');
}

/* Un errore non deve mai restare silenzioso: toast all'utente e traccia in console. */
function fail(err, context) {
  if (err instanceof ApiError && err.status === 401) return;
  console.error(context || 'errore', err);
  toast(apiErrorToMessage(err));
}

function handleUnauthorized() {
  S.logged = false;
  if (window.pfmOfflineQueue) window.pfmOfflineQueue.userId = null;
  S.user = null;
  S.realUser = null;
  S.impersonating = null;
  S.accounts = []; S.categories = []; S.recurrences = [];
  S.users = []; S.adminAccounts = []; S.transfers = [];
  S.bal = {}; LED = [];
  document.getElementById('app').classList.remove('is-on');
  document.getElementById('loginScreen').style.display = 'flex';
}

function newClientId() {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID();
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, c => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/* ---------- helper di dominio ---------- */
const ICONS = {current: 'bi-bank', deposit: 'bi-piggy-bank', card: 'bi-credit-card', virtual: 'bi-bookmark-star'};
const KIND_LABEL = {current: 'corrente', deposit: 'deposito', card: 'carta', virtual: 'virtuale'};
const KIND_TO_TIPO = {current: 'corrente', deposit: 'deposito', card: 'carta', virtual: 'virtuale'};
const TIPO_TO_KIND = {corrente: 'current', deposito: 'deposit', carta: 'card', virtuale: 'virtual'};

const acct = id => S.accounts.find(a => a.id === id);
const catById = id => S.categories.find(c => c.id === id) || {id: id || '', name: t('common.category'), emoji: '✳️', kind: 'out'};
const catEmoji = id => catById(id).emoji || '✳️';
const catName = id => catById(id).name || '—';
/* Le righe di sistema della carta (chiusura calcolata, addebito previsto) non hanno categoria:
   senza questo avrebbero il segnaposto generico "Categoria". */
const isCardSys = e => !!(e && (e.settle || e.cardForecast));
const entryCatEmoji = e => esc(isCardSys(e) ? '💳' : catEmoji(e.cat));
const entryCatName = e => (isCardSys(e) ? t('reg.cat_card') : catName(e.cat));
/* L'icona finisce in un attributo class="…": accetta solo un nome di classe valido. */
const iconOf = a => {
  const v = (a && (a.icon || ICONS[a.kind])) || 'bi-bank';
  return /^[A-Za-z0-9_-]{1,40}$/.test(v) ? v : 'bi-bank';
};
/* Equivalente emoji delle icone bootstrap-icons dei conti (un <option> nativo
   non può contenere <i>). Unico punto di resa per tutti i selettori di conto. */
const ICON_EMOJI = {'bi-bank': '🏦', 'bi-piggy-bank': '🐷', 'bi-credit-card': '💳', 'bi-credit-card-2-front': '💳',
  'bi-cash-coin': '💰', 'bi-wallet2': '👛', 'bi-people': '👥', 'bi-airplane': '✈️', 'bi-house-door': '🏠',
  'bi-receipt': '🧾', 'bi-gift': '🎁', 'bi-mortarboard': '🎓', 'bi-heart-pulse': '🩺', 'bi-car-front': '🚗',
  'bi-bookmark-star': '🔖', 'bi-cloud': '☁️'};
const emojiOf = a => ICON_EMOJI[iconOf(a)] || '🏦';
/* Testo di un <option> conto: "🏦 Nome — nota" (nota troncata a 40 car.). Sempre
   con esc() all'inserimento: restituisce testo semplice, da passare a esc(). */
function accountOptionLabel(a) {
  const note = (a.note || '').replace(/\s+/g, ' ').trim();
  const n = note.length > 40 ? note.slice(0, 39) + '…' : note;
  return `${emojiOf(a)} ${a.name}${n ? ' — ' + n : ''}`;
}
const accountOption = (a, extra = '') => `<option value="${esc(a.id)}">${esc(accountOptionLabel(a) + extra)}</option>`;
const tipoOf = a => KIND_TO_TIPO[a?.kind] || 'corrente';
const kindLabel = a => t(`acct_type.${a?.kind}`, {}) || KIND_LABEL[a?.kind] || '';

const isAdmin = () => S.user?.role === 'admin';
const shareLevel = a => (a?.shares || []).find(x => x.user_id === S.user?.id)?.level || null;
/* `GET /accounts` applica già `list_visible`: quanto torna è, per definizione,
   ciò che l'utente può vedere. Nessun secondo filtro lato client, che sarebbe
   solo un'occasione per divergere dal server. */
const canSee = a => !!a;
/* `GET /accounts` restituisce già solo i conti visibili all'utente, quindi la
   visibilità è decisa dal server. La scrittura invece non è ricavabile dalla
   risposta (AccountDto non porta il livello di condivisione): per i conti non
   propri si presume scrivibile e l'autorizzazione resta quella del server,
   che risponde 403. Vedi "problemi residui". */
function canWrite(a) {
  if (!a) return false;
  if (a.active === false) return false;
  if (isAdmin() || a.owner_id === S.user?.id) return true;
  const lvl = shareLevel(a);
  return lvl === null ? true : lvl === 'write';
}
const visibleAccounts = () => S.accounts.filter(canSee);
const writableAccounts = () => S.accounts.filter(canWrite);
const canWriteCurrent = () => S.accountId === 'all'
  ? writableAccounts().length > 0
  : canWrite(acct(S.accountId));
const userName = id => S.users.find(u => u.id === id)?.username || (id === S.user?.id ? S.user.username : '—');

const balReal = id => {
  const b = S.bal[id];
  /* Carta: il saldo è il non ancora addebitato, non il saldo azzerato dalla chiusura calcolata. */
  if (b?.cardOut != null) return -b.cardOut;
  return b?.real ?? (acct(id)?.start_balance_cents || 0);
};
/* Esposizione della carta: il saldo "reale" include il movimento di chiusura calcolato (che azzera la carta),
   quindi si usa il plafond residuo calcolato dal server sulle spese non ancora addebitate. */
const cardUsed = (a, fallback) => {
  const out = S.bal[a.id]?.cardOut;
  return out != null ? out : Math.abs(Math.min(0, fallback));
};
const balShown = id => (S.bal[id]?.cardOut != null ? balReal(id) : (S.bal[id]?.allocated ?? balReal(id)));
const allocatedOn = id => S.accounts
  .filter(a => a.kind === 'virtual' && a.primary_account_id === id)
  .reduce((s, a) => s + balShown(a.id), 0);
/* Cifra mostrata in sidebar: le carte di credito come prima; per conti correnti, depositi e salvadanai
   il saldo a oggi, senza movimenti con data futura. */
const sideBal = a => (a.kind !== 'card' && S.bal[a.id]?.today != null ? S.bal[a.id].today : balShown(a.id));
/* Totale di sidebar: coerente con le cifre dei conti reali elencati (i salvadanai ne sono una quota). */
const sideTotal = () => visibleAccounts()
  .filter(a => a.kind !== 'virtual')
  .reduce((s, a) => s + (a.kind !== 'card' && S.bal[a.id]?.today != null ? S.bal[a.id].today : balReal(a.id)), 0);
const netWorth = () => visibleAccounts()
  .filter(a => a.kind !== 'virtual')
  .reduce((s, a) => s + balReal(a.id), 0);

function cssVar(n) { return getComputedStyle(document.body).getPropertyValue(n).trim(); }

/* ---------- toast ---------- */
function toast(msg, actionLabel, action, ms = 6000) {
  const wrap = document.getElementById('toasts');
  if (!wrap) return;
  const el = document.createElement('div');
  el.className = 'toast-item';
  el.innerHTML = `<span class="t-msg">${esc(msg)}</span>`;
  if (actionLabel) {
    const b = document.createElement('button');
    b.className = 't-act';
    b.textContent = actionLabel;
    b.onclick = () => { el.remove(); action?.(); };
    el.appendChild(b);
  }
  const x = document.createElement('button');
  x.className = 'icon-btn';
  x.style.color = 'inherit';
  x.innerHTML = '<i class="bi bi-x"></i>';
  x.setAttribute('aria-label', t('common.cancel'));
  x.onclick = () => el.remove();
  el.appendChild(x);
  wrap.appendChild(el);
  setTimeout(() => el.remove(), ms);
}

/* ============================================================
   CARICAMENTO DATI
   ============================================================ */
async function loadAccounts() { S.accounts = await api('/accounts'); }
async function loadCategories() { S.categories = await api('/categories'); }

/* Tag: elenco globale + mappa movimento → id dei tag (solo movimenti visibili). */
async function loadTags() {
  S.tags = await api('/tags');
  await loadTagAssignments();
}
async function loadTagAssignments() { S.tagMap = await api('/tags/assignments'); }
const tagOf = id => S.tags.find(x => x.id === id);
const tagsOfTx = id => (S.tagMap[id] || []).map(tagOf).filter(Boolean);
const tagColor = tg => tg.color || 'var(--accent)';
const tagPill = tg => `<span class="tag-pill" style="background:${esc(tagColor(tg))}" title="${esc(tg.description || tg.label)}">${esc(tg.label)}</span>`;
async function loadRecurrences() { S.recurrences = await api('/recurrences'); }
async function loadUsers() { S.users = await api('/users'); }
async function loadAdminAccounts() { S.adminAccounts = await api('/admin/accounts'); }

/* Le condivisioni non sono nel payload dei conti e `GET /accounts/{id}/shares`
   è riservato a proprietario e amministratore: le carico solo dove è lecito. */
async function loadShares() {
  const owned = S.accounts.filter(a => isAdmin() || a.owner_id === S.user?.id);
  await Promise.all(owned.map(async a => {
    try { a.shares = await api(`/accounts/${encodeURIComponent(a.id)}/shares`); }
    catch { a.shares = a.shares || []; }
  }));
}

/* Saldi: due valori per conto, quello della banca e quello mostrato nel
   registro (che ripiega dentro i salvadanai selezionati). */
async function loadBalances() {
  const vacct = [...S.virtualOn].join(',');
  const qs = vacct ? `?vacct_included=${encodeURIComponent(vacct)}` : '';
  const next = {};
  const domani = addDays(TODAY, 1);
  await Promise.all(visibleAccounts().map(async a => {
    try {
      const b = await api(`/accounts/${encodeURIComponent(a.id)}/balance${qs}`);
      next[a.id] = {real: b.real_cents, allocated: b.allocated_cents, cardAvail: b.card_available_cents ?? null, cardOut: b.card_outstanding_cents ?? null};
    } catch {
      next[a.id] = {real: a.start_balance_cents || 0, allocated: a.start_balance_cents || 0};
    }
    /* Saldo a oggi dei conti non carta (sidebar): l'apertura di domani è la somma dei soli movimenti
       fino a oggi, senza addebiti/accrediti con data futura né previsioni. */
    if (a.kind !== 'card') {
      try {
        const p = new URLSearchParams({from: domani, to: domani, account: a.id});
        if (vacct) p.set('vacct_included', vacct);
        next[a.id].today = (await api(`/transactions?${p}`)).summary.opening_cents;
      } catch { /* la sidebar ripiega sul saldo complessivo */ }
    }
  }));
  S.bal = next;
}

/* Spese e imposte del conto ("Spese" nella card e nelle statistiche): stessa
   chiamata usata dal modale regole (`fillRuleNext`) per la prossima scadenza,
   ma senza `rule_id` così arrivano in una volta sola tutte le regole attive
   del conto. Un conto senza regole (o non leggibile) resta semplicemente
   senza voci: non è un errore da segnalare. */
async function loadFeeSummaries() {
  const targets = visibleAccounts().filter(a => a.kind !== 'virtual');
  const from = TODAY;
  const to = addDays(TODAY, 400);
  const next = {};
  await Promise.all(targets.map(async a => {
    try {
      const previews = await api(`/accounts/${encodeURIComponent(a.id)}/rules/preview`, {
        method: 'POST', body: JSON.stringify({from, to}),
      });
      next[a.id] = (Array.isArray(previews) ? previews : []).map(p => ({
        nome: p.name,
        dovuto: p.rows.length > 0,
        cents: p.rows[0]?.cents ?? 0,
        date: p.rows[0]?.date ?? null,
      }));
    } catch {
      next[a.id] = [];
    }
  }));
  S.fees = next;
}

function feeSummary(accountId) {
  return S.fees[accountId] || [];
}

async function loadAll() {
  await Promise.all([loadAccounts(), loadCategories(), loadRecurrences(), loadHolidayCalendars(), loadTags().catch(err => console.warn('tag non disponibili', err))]);
  if (isAdmin()) {
    await Promise.all([loadUsers(), loadAdminAccounts()]);
  } else {
    S.users = S.user ? [{id: S.user.id, username: S.user.username, email: S.user.email, role: S.user.role, active: true}] : [];
    S.adminAccounts = [];
  }
  await loadShares();
  const vis = visibleAccounts();
  if (S.accountId !== 'all' && !vis.some(a => a.id === S.accountId)) {
    S.accountId = vis.length ? 'all' : 'all';
  }
  // I salvadanai partono inclusi: è il saldo che l'utente si aspetta.
  vis.filter(a => a.kind === 'virtual').forEach(a => S.virtualOn.add(a.id));
  await loadBalances();
  await loadFeeSummaries();
}

/* ---------- intervalli di date ---------- */
function registerRange() {
  let base;
  if (S.winRange) base = S.winRange;
  else {
    const center = S.anchor || TODAY;
    base = {from: addDays(center, -(S.rows.above + S.extraAbove)), to: addDays(center, S.rows.below + S.extraBelow)};
  }
  /* I filtri "Dal"/"Al" del registro non possono limitarsi al lato client:
     se cadono fuori dalla finestra caricata non ci sarebbe nulla da filtrare.
     Diventano quindi l'intervallo interrogato. */
  const from = S.filters.from || base.from;
  const to = S.filters.to || base.to;
  return from <= to ? {from, to} : {from: to, to: from};
}

/* Chiavi mese (YYYY-MM) di inizio e fine di un periodo del registro. */
function periodKeys(p) {
  const d = fromISO(TODAY);
  const y = d.getFullYear(), m = d.getMonth();
  const key = (yy, mm) => som(yy, mm).slice(0, 7);
  if (p === 'thisMonth') return [key(y, m), key(y, m)];
  if (p === 'lastMonth') return [key(y, m - 1), key(y, m - 1)];
  if (p === 'last3') return [key(y, m - 2), key(y, m)];
  if (p === 'last12') return [key(y, m - 11), key(y, m)];
  if (p === 'year') return [key(y, 0), key(y, 11)];
  if (p === 'custom') return [S.range.k1, S.range.k2];
  return [S.month, S.month];
}

const monthsBetween = (k1, k2) => (Number(k2.slice(0, 4)) - Number(k1.slice(0, 4))) * 12 + Number(k2.slice(5, 7)) - Number(k1.slice(5, 7)) + 1;

/* Intervallo di date dalla chiave mese iniziale a quella finale: mese solare,
   oppure ciclo di chiusura quando è selezionata una carta. */
function boundsForKeys(k1, k2, card) {
  if (card) return {first: cardMonthBounds(k1, card).first, last: cardMonthBounds(k2, card).last};
  return {first: `${k1}-01`, last: eom(Number(k2.slice(0, 4)), Number(k2.slice(5, 7)) - 1)};
}

function periodRange(p) {
  const [k1, k2] = periodKeys(p);
  const {first, last} = boundsForKeys(k1, k2, cardOf(S.accountId));
  return {from: first, to: last};
}

/* ---------- registro ---------- */
function registerQuery(accountId, from, to) {
  const p = new URLSearchParams();
  p.set('from', from);
  p.set('to', to);
  p.set('account', accountId);
  if (S.filters.cat) p.set('cat', S.filters.cat);
  if (S.filters.tipo) p.set('tipo', S.filters.tipo === 'entrata' ? 'in' : 'out');
  if (S.filters.min !== '') p.set('min', String(parseAmount(S.filters.min)));
  if (S.filters.max !== '') p.set('max', String(parseAmount(S.filters.max)));
  if (S.search.trim()) p.set('q', S.search.trim());
  const vacct = [...S.virtualOn].join(',');
  if (vacct) p.set('vacct_included', vacct);
  return p.toString();
}

/* Righe previste dalle ricorrenze.
   `GET /transactions` restituisce solo movimenti persistiti: le occorrenze
   future esistono unicamente come calcolo, esposto da
   `GET /recurrences/{id}/occurrences`. Le sovrappongo qui, e solo per le date
   successive a oggi, così non possono duplicare righe reali già presenti. */
async function loadForecast(accountIds, from, to, force = false) {
  if ((!force && !S.showFuture) || to <= TODAY) return [];
  const fFrom = from > TODAY ? from : addDays(TODAY, 1);
  /* Una ricorrenza con conto collegato è un trasferimento: ogni occorrenza ha due gambe,
     l'addebito sul conto di origine e l'accredito (segno opposto) su quello collegato.
     La ricorrenza serve se almeno uno dei due conti è nell'ambito richiesto. */
  const inScope = S.recurrences.filter(r => r.active
    && (accountIds.includes(r.account_id) || (r.linked_account_id && accountIds.includes(r.linked_account_id))));
  const out = [];
  await Promise.all(inScope.map(async r => {
    try {
      const occ = await api(`/recurrences/${encodeURIComponent(r.id)}/occurrences?from=${fFrom}&to=${to}`);
      for (const o of occ) {
        if (o.date <= TODAY) continue;
        const leg = (acctId, cents, cat) => ({
          id: null,
          recId: r.id,
          date: o.date,
          cents,
          desc: o.description || r.description || '',
          cat,
          note: null,
          acct: acctId,
          pair: r.linked_account_id ? `rec:${r.id}:${o.date}` : null,
          previsto: true,
          stimato: r.mode === 'avg' && !o.overridden,
          override: o.overridden,
        });
        if (accountIds.includes(r.account_id)) out.push(leg(r.account_id, o.amount_cents, r.category_id));
        if (r.linked_account_id && accountIds.includes(r.linked_account_id)) {
          out.push(leg(r.linked_account_id, -o.amount_cents, r.linked_category_id || r.category_id));
        }
      }
    } catch (err) {
      // Una ricorrenza non leggibile non deve svuotare il registro.
      console.warn('occorrenze non disponibili per', r.id, err);
    }
  }));
  out.push(...await loadCardForecast(accountIds, from, to));
  return out;
}

/* Addebiti previsti delle carte: i cicli non ancora liquidati, con la spesa già registrata più
   le ricorrenze in arrivo. Sul conto collegato compare l'addebito alla data lavorativa prevista;
   sulla carta compare la chiusura (che azzera il saldo, quindi fuori dal progressivo). */
async function loadCardForecast(accountIds, from, to) {
  if (!S.accounts.some(a => a.kind === 'card')) return [];
  try {
    const list = await api(`/cards/forecast?from=${from}&to=${to}`);
    const out = [];
    for (const f of list) {
      const name = acct(f.card_id)?.name || '';
      const base = {id: null, recId: null, cardForecast: true, cat: '', note: null, pair: null,
        previsto: true, stimato: false, override: false};
      if (accountIds.includes(f.backing_id) && f.backing_date > TODAY) {
        out.push({...base, date: f.backing_date, cents: -(f.spend_cents + f.fee_cents),
          desc: t('reg.card_forecast', {card: name}), acct: f.backing_id});
      }
      if (accountIds.includes(f.card_id) && f.card_date > TODAY) {
        out.push({...base, date: f.card_date, cents: f.spend_cents, settle: true,
          desc: t('reg.card_close_forecast', {card: name}), acct: f.card_id});
      }
    }
    return out;
  } catch (err) {
    console.warn('addebiti carta previsti non disponibili', err);
    return [];
  }
}

function rowToEntry(row, accountId) {
  return {
    id: row.id,
    recId: row.rec_id || null,
    date: row.date,
    cents: row.cents,
    desc: txDesc(row.description) || catName(row.category_id),
    cat: row.category_id || '',
    note: row.note || null,
    acct: row.account_id || accountId,
    pair: row.pair_id || null,
    /* Il server segna le righe il cui conto non è modificabile (giroconti,
       ricorrenze, regole, salvadanai, importati): vedi Transaction::can_change_account. */
    locked: !!row.account_locked,
    settle: row.description === 'card settlement' && row.cents > 0,
    previsto: row.date > TODAY,
    stimato: false,
    override: false,
  };
}

/* Costruisce LED a partire da una o più risposte del registro.
   Il saldo progressivo viene ricalcolato qui perché le righe previste si
   inseriscono in mezzo a quelle reali e perché su "tutti i conti"
   l'apertura è la somma delle aperture dei singoli conti. */
function buildLed(from, to, opening, entries) {
  const byDay = new Map();
  for (const e of entries) {
    if (e.date < from || e.date > to) continue;
    if (!byDay.has(e.date)) byDay.set(e.date, []);
    byDay.get(e.date).push(e);
  }
  const days = [];
  let bal = opening;
  let cur = from;
  let guard = 0;
  while (cur <= to && guard++ < 4000) {
    const list = (byDay.get(cur) || []).sort((a, b) => (a.cents === b.cents ? 0 : a.cents < b.cents ? -1 : 1));
    /* Il movimento di chiusura della carta (calcolato) azzererebbe il saldo: non entra nel progressivo. */
    for (const e of list) { if (!e.settle) bal += e.cents; e.saldo = bal; }
    days.push({
      key: cur,
      empty: list.length === 0,
      isToday: cur === TODAY,
      isFuture: cur > TODAY,
      saldoFine: bal,
      entries: list,
    });
    cur = addDays(cur, 1);
  }
  LED_OPENING = opening;
  return days;
}

async function loadRegister() {
  const vis = visibleAccounts();
  if (!vis.length) { LED = []; return; }
  const {from, to} = registerRange();
  const targets = S.accountId === 'all'
    ? vis.filter(a => a.kind !== 'virtual').map(a => a.id)
    : [S.accountId];

  /* Su "tutti i conti" il backend non aggrega: `GET /transactions` accetta un
     conto solo. Interrogo i conti visibili in parallelo e unisco qui, come fa
     il mockup con `ledger('all')`. Vedi "problemi residui". */
  const parts = await Promise.all(targets.map(async id => {
    const data = await api(`/transactions?${registerQuery(id, from, to)}`);
    return {id, data};
  }));

  let opening = 0;
  const entries = [];
  for (const {id, data} of parts) {
    opening += data.summary.opening_cents;
    for (const day of data.days) {
      for (const row of day.rows) entries.push(rowToEntry(row, id));
    }
  }
  /* Le occorrenze previste vanno proiettate dall'oggi fino alla fine del
     periodo richiesto, non solo dentro la finestra selezionata: se si passa
     direttamente a un mese futuro non contiguo (es. novembre saltando
     ottobre), l'apertura di quel mese deve comunque riflettere le occorrenze
     previste nei mesi intermedi anche se non vengono mostrate. La parte del
     forecast antecedente a `from` confluisce quindi nell'apertura invece che
     tra le righe visualizzate, che restano limitate al periodo richiesto. */
  const forecastFrom = addDays(TODAY, 1);
  const forecast = forecastFrom <= to ? await loadForecast(targets, forecastFrom, to) : [];
  const seen = new Set(entries.filter(e => e.recId).map(e => `${e.recId}|${e.date}|${e.acct}`));
  for (const f of forecast) {
    if (seen.has(`${f.recId}|${f.date}|${f.acct}`)) continue;
    if (f.date < from) {
      /* Un'occorrenza già confermata da una transazione reale è dedotta
         dall'apertura solo se cade prima di `from`: `opening_cents` del
         backend somma già tutte le transazioni reali con date < from,
         quindi qui sommerei l'importo una seconda volta. Solo le
         occorrenze ancora stimate mancano da quel calcolo. */
      if (!f.override && !f.settle) opening += f.cents;
      continue;
    }
    entries.push(f);
  }
  LED = buildLed(from, to, opening, entries);
}

/* ---------- finestra dei giorni mostrati ---------- */
function visibleDays() {
  let days = LED;
  if (!S.showFuture) days = days.filter(d => !d.isFuture);
  if (!S.showEmpty) days = days.filter(d => !d.empty || d.isToday);
  return days;
}

function computeWindow() {
  const days = visibleDays();
  if (!days.length) { S.win = {from: 0, to: 0}; return; }
  if (S.winRange) {
    let a = days.findIndex(d => d.key >= S.winRange.from);
    if (a < 0) a = days.length - 1;
    const b = days.map(d => d.key).reduce((acc, k, i) => (k <= S.winRange.to ? i : acc), a);
    S.win = {from: Math.max(0, a - S.extraAbove), to: Math.min(days.length - 1, b + S.extraBelow)};
    return;
  }
  let c = days.findIndex(d => d.key >= (S.anchor || TODAY));
  if (c < 0) c = days.length - 1;
  S.win = {
    from: Math.max(0, c - (S.rows.above + S.extraAbove)),
    to: Math.min(days.length - 1, c + (S.rows.below + S.extraBelow)),
  };
}

function rangeEmpty() {
  if (!S.winRange) return LED.length > 0 && visibleDays().length === 0;
  return !visibleDays().some(d => d.key >= S.winRange.from && d.key <= S.winRange.to);
}

function monthLabelText() {
  const nm = k => { const m = MONTHS()[Number(k.slice(5, 7)) - 1] || ''; return isMobile() ? m.slice(0, 3) : m; };
  const [k1, k2] = S.period === 'around' ? [S.month, S.month] : periodKeys(S.period);
  if (k1 === k2) return `${nm(k1)} ${k1.slice(0, 4)}`;
  return k1.slice(0, 4) === k2.slice(0, 4)
    ? `${nm(k1)} – ${nm(k2)} ${k2.slice(0, 4)}`
    : `${nm(k1)} ${k1.slice(2, 4)} – ${nm(k2)} ${k2.slice(2, 4)}`;
}

/* ---------- ricerca e filtri (lato server, riflessi in UI) ---------- */
const filtersActive = () => S.search.trim() !== '' || Object.values(S.filters).some(v => v !== '');

function localMatch(e) {
  const f = S.filters;
  if (f.from && e.date < f.from) return false;
  if (f.to && e.date > f.to) return false;
  if (f.tag && !(S.tagMap[e.id] || []).includes(f.tag)) return false;
  return true;
}

function searchResults() {
  const out = [];
  LED.forEach(d => d.entries.forEach(e => { if (localMatch(e)) out.push(e); }));
  return out.sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : 0));
}

/* ============================================================
   KPI DEL REGISTRO
   Le classi (k-l / k-v / k-s / k-two / k-meter) sono quelle del
   mockup: cambiarle svuota il CSS già presente in index.html.
   ============================================================ */
function balanceAt(dayKey) {
  let last = LED_OPENING;
  for (const d of LED) { if (d.key <= dayKey) last = d.saldoFine; else break; }
  return last;
}

/* La carta selezionata (con giorno di chiusura) i cui movimenti vanno raggruppati
   per ciclo di chiusura anziché per mese solare. `null` altrimenti. */
function cardOf(id) {
  const a = id && id !== 'all' ? acct(id) : null;
  return a && a.kind === 'card' && a.closing_day ? a : null;
}

/* Ultimo giorno di competenza del ciclo che chiude in `closeDay`: se la chiusura
   cade di sabato o domenica, tutto il weekend passa al ciclo successivo e il
   ciclo si ferma al venerdì precedente. */
const effectiveClose = closeDay => {
  const wd = fromISO(closeDay).getDay();
  return wd === 6 ? addDays(closeDay, -1) : wd === 0 ? addDays(closeDay, -2) : closeDay;
};

/* Finestra del "mese" per una carta: il ciclo tra due giorni di chiusura
   contabile. Selezionando dicembre (2025-12) con chiusura al 4 si vedono i
   movimenti dal 5 dicembre al 4 gennaio: il giorno di chiusura appartiene al
   ciclo che chiude (estremo finale incluso), non a quello che apre, tranne
   quando cade nel weekend (vedi effectiveClose). */
function cardMonthBounds(mk, a) {
  const [y, m] = mk.split('-').map(Number);
  const cd = String(a.closing_day).padStart(2, '0');
  const ny = m === 12 ? y + 1 : y;
  const nm = m === 12 ? 1 : m + 1;
  return {first: addDays(effectiveClose(`${mk}-${cd}`), 1), last: effectiveClose(`${ny}-${String(nm).padStart(2, '0')}-${cd}`)};
}

/* Data di addebito prevista del ciclo etichettato `mk`: giorno di addebito dopo
   la chiusura, spostato al primo giorno lavorativo (come fa il server). */
function cardChargeDate(mk, a) {
  const [y, m] = mk.split('-').map(Number);
  const closeM = new Date(y, m, 1);               // mese della chiusura
  const chargeM = a.charge_day >= a.closing_day ? closeM : new Date(y, m + 1, 1);
  let d = iso(new Date(chargeM.getFullYear(), chargeM.getMonth(), a.charge_day));
  for (let i = 0; i < 10 && (isWeekend(d) || isItalianHoliday(d)); i++) d = addDays(d, 1);
  return d;
}

function isItalianHoliday(s) {
  const d = fromISO(s), md = (d.getMonth() + 1) * 100 + d.getDate();
  if ([101, 106, 425, 501, 602, 815, 1101, 1208, 1225, 1226].includes(md)) return true;
  const Y = d.getFullYear(), a = Y % 19, b = Math.floor(Y / 100), c = Y % 100;
  const dd = Math.floor(b / 4), e = b % 4, f = Math.floor((b + 8) / 25), g = Math.floor((b - f + 1) / 3);
  const h = (19 * a + b - dd - g + 15) % 30, i = Math.floor(c / 4), k = c % 4;
  const l = (32 + 2 * e + 2 * i - h - k) % 7, mm = Math.floor((a + 11 * h + 22 * l) / 451);
  const month = Math.floor((h + l - 7 * mm + 114) / 31), day = ((h + l - 7 * mm + 114) % 31) + 1;
  return s === addDays(iso(new Date(Y, month - 1, day)), 1);
}

function monthBounds(mk) {
  const a = cardOf(S.accountId);
  if (a) return cardMonthBounds(mk, a);
  const [y, m] = mk.split('-').map(Number);
  return {first: `${mk}-01`, last: eom(y, m - 1)};
}

function renderKPIs() {
  const box = document.getElementById('kpis');
  if (!box) return;
  if (!visibleAccounts().length) {
    box.innerHTML = `<div class="kpi k-wide"><div class="k-l">${t('kpi.saldo_today')}</div>
      <div class="k-v num">—</div><div class="k-s">${t('empty.no_accounts')}</div></div>`;
    return;
  }
  const a = S.accountId === 'all' ? null : acct(S.accountId);
  /* Per una carta i KPI seguono il periodo mostrato (più mesi inclusi); l'ultimo mese è il riferimento. */
  let [k1, k2] = a && a.kind === 'card' && S.period !== 'around' ? periodKeys(S.period) : [S.month, S.month];
  const nMesi = monthsBetween(k1, k2);
  const {first, last} = a && a.kind === 'card' && S.period !== 'around' ? boundsForKeys(k1, k2, cardOf(a.id)) : monthBounds(S.month);
  const mese = `${MONTHS()[Number(k2.slice(5, 7)) - 1]} ${k2.slice(0, 4)}`;
  const meseBreve = MONTHS()[Number(k2.slice(5, 7)) - 1];
  const futuro = last > TODAY;
  const saldoOggi = a && a.kind === 'card' && S.bal[a.id]?.cardOut != null ? -S.bal[a.id].cardOut : balanceAt(TODAY);
  const saldoFineMese = balanceAt(last);

  let html = `
    <div class="kpi k-wide">
      <div class="k-l">${t('kpi.saldo_today')}</div>
      <div class="k-v num ${saldoOggi < 0 ? 'neg' : ''}">${money(saldoOggi)}</div>
      <div class="k-s">${a ? esc(a.name) : t('kpi.all_accounts')} · ${fmtDate(TODAY)}</div>
    </div>
    <div class="kpi">
      <div class="k-l">${t('kpi.saldo_month_end', {month: meseBreve})}</div>
      <div class="k-v num ${saldoFineMese < 0 ? 'neg' : ''}">${money(saldoFineMese)}</div>
      <div class="k-s">${futuro ? t('kpi.after_forecast') : t('kpi.actual_value')}</div>
    </div>`;

  if (a && a.kind === 'deposit') {
    const it = S.interest?.[a.id] || null;
    html += `
      <div class="kpi">
        <div class="k-l">${t('kpi.interest', {month: meseBreve})}
          <i class="bi bi-info-circle hint" title="${esc(t('kpi.interest_hint'))}"></i></div>
        <div class="k-v num pos">${it ? money(it.net_cents) : '—'}</div>
        <div class="k-s">${it ? t('kpi.interest_sub', {gross: money(it.gross_cents)}) : t('kpi.no_rate')}</div>
      </div>
      <div class="kpi">
        <div class="k-l">${t('kpi.rate')}</div>
        <div class="k-v num">${a.rate_pct_bps ? nf.format(a.rate_pct_bps / 100) + '%' : '—'}</div>
        <div class="k-s">${a.rate_pct_bps ? t('kpi.rate_sub') : t('kpi.set_rate')}</div>
      </div>`;
  } else if (a && a.kind === 'card') {
    /* Saldo del ciclo (senza chiusura calcolata) e non addebitato del server: vale il più alto, così il massimale segue sempre le spese inserite. */
    const esposizione = Math.max(Math.abs(Math.min(0, saldoFineMese)), last >= TODAY ? cardUsed(a, 0) : 0);
    const mass = (a.limit_cents || 0) * nMesi;
    const pct = mass ? Math.round(esposizione / mass * 100) : 0;
    const cls = pct > 85 ? 'over' : pct > 60 ? 'warn' : 'good';
    html += `
      <div class="kpi">
        <div class="k-l">${t('kpi.card_charge_date')}</div>
        <div class="k-v num">${a.charge_day && a.closing_day ? fmtDate(cardChargeDate(k2, a)) : '—'}</div>
        <div class="k-s">${a.primary_account_id && acct(a.primary_account_id)
          ? t('kpi.card_charge_on', {account: esc(acct(a.primary_account_id).name)}) : ''}</div>
      </div>
      <div class="kpi k-wide ${pct > 85 ? 'k-warn' : ''}">
        <div class="k-l">${t('kpi.card_left', {month: meseBreve})}</div>
        <div class="k-v num">${money(mass - esposizione)}</div>
        <div class="k-meter"><i class="${cls}" style="width:${Math.min(100, pct)}%"></i></div>
        <div class="k-s">${t('kpi.card_used', {used: money(esposizione), limit: money(mass), pct})}</div>
      </div>`;
  } else if (a && a.kind === 'virtual') {
    const goal = a.goal_cents || 0;
    const manca = Math.max(0, goal - saldoFineMese);
    const pct = goal ? Math.min(100, Math.round(saldoFineMese / goal * 100)) : 0;
    let versato = 0;
    LED.forEach(d => { if (d.key >= first && d.key <= last) d.entries.forEach(e => { if (e.cents > 0) versato += e.cents; }); });
    html += `
      <div class="kpi">
        <div class="k-l">${goal ? t('kpi.goal_left', {month: meseBreve}) : t('kpi.set_aside')}</div>
        <div class="k-v num ${goal && !manca ? 'pos' : ''}">${goal ? (manca ? money(manca) : t('kpi.goal_reached')) : money(saldoFineMese)}</div>
        ${goal ? `<div class="k-meter"><i class="${pct >= 100 ? 'good' : ''}" style="width:${pct}%"></i></div>
          <div class="k-s">${money(saldoFineMese)} / ${money(goal)} · ${pct}%</div>`
      : `<div class="k-s">${t('kpi.no_goal')}</div>`}
      </div>
      <div class="kpi">
        <div class="k-l">${t('kpi.paid_in', {month: meseBreve})}</div>
        <div class="k-v num ${versato ? 'pos' : ''}">${money(versato)}</div>
        <div class="k-s">${a.primary_account_id && acct(a.primary_account_id)
          ? t('kpi.from_account', {account: esc(acct(a.primary_account_id).name)}) : ''}${futuro ? t('kpi.forecast') : ''}</div>
      </div>`;
  } else {
    /* Come nelle Statistiche: la chiusura calcolata della carta e l'addebito previsto non sono
       entrate/uscite; su "tutti i conti" i giroconti tra i propri conti si annullano e non contano.
       Su un singolo conto un giroconto resta un vero movimento di cassa. */
    const interno = e => isCardSys(e) || (!a && e.pair);
    let entrate = 0, uscite = 0;
    LED.forEach(d => {
      if (d.key >= first && d.key <= last) d.entries.forEach(e => { if (!interno(e)) (e.cents > 0 ? entrate += e.cents : uscite -= e.cents); });
    });
    let minVal = Infinity, minDay = null;
    const fromEff = last < TODAY ? first : TODAY;
    LED.forEach(d => { if (d.key >= fromEff && d.key <= last && d.saldoFine < minVal) { minVal = d.saldoFine; minDay = d.key; } });
    if (minVal === Infinity) { minVal = saldoFineMese; minDay = last; }
    const minAcct = a ? (a.min_balance_cents || 0) : 0;
    const sotto = minAcct > 0 && minVal < minAcct;
    html += `
      <div class="kpi ${sotto ? 'k-warn' : ''}">
        <div class="k-l">${t('kpi.min_balance', {status: futuro ? t('kpi.min_balance_forecast') : t('kpi.min_balance_actual')})}
          <i class="bi bi-info-circle hint" title="${esc(t('kpi.min_balance_hint'))}"></i></div>
        <div class="k-v num">${money(minVal)}</div>
        <div class="k-s">${minDay ? fmtDayLong(minDay) : ''}</div>
      </div>
      <div class="kpi k-wide-sm">
        <div class="k-l">${t('kpi.movements_of', {month: mese})}</div>
        <div class="k-two">
          <div>
            <div class="k-sl"><i class="bi bi-arrow-up-right" style="color:var(--pos)"></i> ${t('kpi.income')}</div>
            <div class="k-sv num pos">${money(entrate)}</div>
          </div>
          <div class="sep"></div>
          <div>
            <div class="k-sl"><i class="bi bi-arrow-down-left" style="color:var(--neg)"></i> ${t('kpi.expense')}</div>
            <div class="k-sv num neg">${money(uscite)}</div>
          </div>
        </div>
        <div class="k-s" style="margin-top:.35rem">${t('kpi.net')}
          <b class="${entrate - uscite < 0 ? 'neg' : 'pos'}">${money(entrate - uscite, true)}</b>${futuro ? t('kpi.forecast') : ''}</div>
        ${a ? '' : `<div class="k-s" style="font-size:.72rem">${t('kpi.excl_internal')}</div>`}
      </div>`;
  }
  box.innerHTML = html;
}

/* ============================================================
   REGISTRO — tabella desktop e lista mobile
   Colonne: [sel] DATA | SALDO | IMPORTO | DESCRIZIONE | [CONTO] | CATEGORIA | AZIONI
   ============================================================ */
function emptyRangeHTML() {
  const futuro = S.winRange && S.winRange.from > TODAY;
  return `<div class="empty">
    <i class="bi bi-eye-slash"></i>
    <h6>${futuro ? t('reg.empty.no_rows') : t('reg.empty.no_tx')}</h6>
    <p>${futuro ? t('reg.empty.future_hidden') : t('reg.empty.widen')}</p>
    ${futuro ? `<button class="btn-tool" id="showFutureBtn" style="margin-top:.5rem"><i class="bi bi-eye"></i> ${t('reg.empty.show_forecast')}</button>` : ''}
  </div>`;
}

function rowActions(e) {
  if (e.previsto && e.recId) {
    return `<button class="rowbtn" data-edit="${esc(e.id || '')}" data-rec-date="${e.date}" data-rec-id="${esc(e.recId)}"
              aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>`;
  }
  if (!e.id) return '';
  if (!canWrite(acct(e.acct))) {
    return `<span style="color:var(--text-muted)" title="${esc(t('reg.readonly'))}"><i class="bi bi-lock"></i></span>`;
  }
  return `<button class="rowbtn" data-edit="${esc(e.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>
          <button class="rowbtn del" data-del="${esc(e.id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i></button>`;
}

function descCell(e) {
  const link = e.recId
    ? `<a href="#" class="lnk" data-rec="${esc(e.recId)}" title="${esc(t('reg.link.rec_title'))}">${t('reg.link.rec')}</a>`
    : e.pair
      ? `<a href="#" class="lnk" data-pair="${esc(e.pair)}" data-from="${esc(e.id || '')}" title="${esc(t('reg.link.pair_title'))}">${t('reg.link.pair')}</a>`
      : '';
  const prev = e.previsto ? `<span class="tag-prev">${t('tag.forecast')}</span>` : '';
  const est = e.stimato && e.previsto ? `<span class="tag-est" title="${esc(t('tag.estimated_hint'))}">${t('tag.estimated')}</span>` : '';
  const real = e.recId && e.override && !e.previsto ? `<span class="tag-real" title="${esc(t('tag.actual_hint'))}">${t('tag.actual')}</span>` : '';
  const tg = e.id ? tagsOfTx(e.id).map(tagPill).join('') : '';
  return `${esc(e.desc)}${link}${prev}${est}${real}${tg}`;
}

function renderTable() {
  const body = document.getElementById('regBody');
  if (!body) return;
  const showSel = S.selMode;
  const showAcct = S.accountId === 'all';
  const head = document.getElementById('regHead');
  if (head) {
    head.querySelector('.c-sel').style.display = showSel ? '' : 'none';
    head.querySelector('.c-acct').style.display = showAcct ? '' : 'none';
  }
  const cols = 6 + (showSel ? 1 : 0) + (showAcct ? 1 : 0);
  const acctCell = e => showAcct
    ? `<td class="c-acct"><span class="chip-acct"><i class="bi ${iconOf(acct(e.acct))}"></i> ${esc(acct(e.acct)?.name || '—')}</span></td>`
    : '';
  const row = (e, dataCell, extraCls = '') => `
    <tr ${e.id ? `data-id="${esc(e.id)}"` : ''} class="${extraCls} ${e.previsto ? 'is-future' : ''} ${S.selected.has(e.id) ? 'is-sel' : ''} ${S.hlTx === e.id ? 'is-hl' : ''}">
      ${showSel ? `<td class="c-sel">${e.id ? `<input type="checkbox" data-sel="${esc(e.id)}" ${S.selected.has(e.id) ? 'checked' : ''} aria-label="${esc(e.desc)}">` : ''}</td>` : ''}
      ${dataCell}
      <td class="c-saldo num">${money(e.saldo)}</td>
      <td class="c-imp num ${e.cents > 0 ? 'pos' : 'neg'}">${money(e.cents, S.showSign)}</td>
      <td class="c-desc">${descCell(e)}</td>
      ${acctCell(e)}
      <td class="c-cat"><span class="chip-cat">${entryCatEmoji(e)} ${esc(entryCatName(e))}</span></td>
      <td class="c-az">${rowActions(e)}</td>
    </tr>`;

  if (!visibleAccounts().length) {
    body.innerHTML = `<tr><td colspan="${cols}"><div class="empty"><i class="bi bi-bank"></i>
      <h6>${t('empty.no_accounts')}</h6><p>${t('empty.no_accounts_hint')}</p></div></td></tr>`;
    return;
  }

  if (filtersActive()) {
    const res = searchResults();
    body.innerHTML = res.length
      ? res.map(e => row(e, `<td class="c-data" data-date="${e.date}">${fmtDate(e.date)}<span class="dow">${dowOf(e.date)}</span></td>`)).join('')
      : `<tr><td colspan="${cols}"><div class="empty"><i class="bi bi-search"></i>
          <h6>${t('reg.empty.not_found')}</h6><p>${t('reg.empty.try_filters')}</p></div></td></tr>`;
    if (S.quickRow) injectQuickRow();
    return;
  }

  if (rangeEmpty()) { body.innerHTML = `<tr><td colspan="${cols}">${emptyRangeHTML()}</td></tr>`; return; }

  const days = visibleDays().slice(S.win.from, S.win.to + 1);
  let html = '', prevFuture = false;
  for (const day of days) {
    if (day.isFuture && !prevFuture) {
      prevFuture = true;
      html += `<tr class="today-sep"><td colspan="${cols}"><div class="bar">${t('reg.forecast_sep')}</div></td></tr>`;
    }
    if (day.empty) {
      html += `<tr class="empty-day ${day.isToday ? 'today-row today-first today-last' : ''} ${day.isFuture ? 'is-future' : ''}">
        ${showSel ? '<td class="c-sel"></td>' : ''}
        <td class="c-data" data-date="${day.key}">${fmtDate(day.key)}<span class="dow">${dowOf(day.key)}</span></td>
        <td class="c-saldo num">${money(day.saldoFine)}</td>
        <td class="c-imp"></td>
        <td class="c-desc"><span class="no-mov">${t('reg.no_movement')}</span></td>
        ${showAcct ? '<td class="c-acct"></td>' : ''}
        <td class="c-cat"></td><td class="c-az"></td></tr>`;
      continue;
    }
    day.entries.forEach((e, i) => {
      const first = i === 0, last = i === day.entries.length - 1;
      const tCls = day.isToday ? `today-row ${first ? 'today-first' : ''} ${last ? 'today-last' : ''}` : '';
      const dataCell = `<td class="c-data ${first ? '' : 'repeat'}" data-date="${day.key}">${first
        ? `${fmtDate(day.key)}<span class="dow">${dowOf(day.key)}</span>` : '↳'}</td>`;
      html += row(e, dataCell, tCls);
    });
  }
  body.innerHTML = html || `<tr><td colspan="${cols}"><div class="empty"><i class="bi bi-calendar3"></i>
    <h6>${t('reg.empty.no_tx')}</h6><p>${t('reg.empty.add_first')}</p></div></td></tr>`;
  if (S.quickRow) injectQuickRow();
}

function mobileRow(e, withDate = false) {
  const rw = !!e.id && canWrite(acct(e.acct));
  return `<div class="m-row ${e.previsto ? 'is-future' : ''} ${S.hlTx === e.id ? 'is-hl' : ''} ${rw ? '' : 'ro'}" ${e.id ? `data-id="${esc(e.id)}"` : ''}>
    ${rw ? `<div class="m-actions">
      <button class="a-edit" data-edit="${esc(e.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i>${t('reg.action.edit')}</button>
      <button class="a-del" data-del="${esc(e.id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i>${t('reg.action.delete')}</button>
    </div>` : ''}
    <button class="m-front" ${e.id ? `data-open="${esc(e.id)}"` : `data-rec-open="${esc(e.recId || '')}" data-rec-date="${e.date}"`}>
      <span class="m-ico">${entryCatEmoji(e)}</span>
      <span class="m-mid">
        <span class="m-desc">${esc(e.desc)}${e.previsto ? `<span class="tag-prev">${t('tag.forecast')}</span>` : ''}${e.stimato && e.previsto ? `<span class="tag-est">${t('tag.estimated')}</span>` : ''}</span>
        <span class="m-meta">${withDate ? fmtDate(e.date) + ' · ' : ''}${S.accountId === 'all' ? esc(acct(e.acct)?.name || '—') + ' · ' : ''}${esc(entryCatName(e))}${e.recId ? ' · ' + t('reg.link.rec') : ''}</span>
      </span>
      <span class="m-right">
        <span class="m-imp ${e.cents > 0 ? 'pos' : 'neg'}">${money(e.cents, S.showSign)}</span>
        <span class="m-saldo">${t('common.balance').toLowerCase()} ${money(e.saldo)}</span>
      </span>
    </button>
  </div>`;
}

function renderMobile() {
  const root = document.getElementById('mobileList');
  if (!root) return;
  if (!visibleAccounts().length) {
    root.innerHTML = `<div class="empty"><i class="bi bi-bank"></i><h6>${t('empty.no_accounts')}</h6>
      <p>${t('empty.no_accounts_hint')}</p></div>`;
    return;
  }
  if (filtersActive()) {
    const res = searchResults();
    root.innerHTML = res.length
      ? res.map(e => mobileRow(e, true)).join('')
      : `<div class="empty"><i class="bi bi-search"></i><h6>${t('reg.empty.not_found')}</h6><p>${t('reg.empty.try_filters')}</p></div>`;
    return;
  }
  if (rangeEmpty()) { root.innerHTML = emptyRangeHTML(); return; }
  const days = visibleDays().slice(S.win.from, S.win.to + 1);
  let html = '', prevFuture = false;
  for (const day of days) {
    if (day.isFuture && !prevFuture) { prevFuture = true; html += `<div class="m-today-line">${t('reg.forecast_short')}</div>`; }
    html += `<div class="m-day ${day.isToday ? 'is-today' : ''} ${day.isFuture ? 'is-future' : ''}">
        <span class="d-l">${day.isToday ? t('reg.today') + ' · ' : ''}${fmtDayLong(day.key)}</span>
        <span class="d-r">${money(day.saldoFine)}</span></div>`;
    if (day.empty) {
      html += `<div class="m-row"><div class="m-front" style="color:var(--text-muted);font-size:.83rem;font-style:italic">${t('reg.no_movement')}</div></div>`;
      continue;
    }
    day.entries.forEach(e => { html += mobileRow(e); });
  }
  root.innerHTML = html || `<div class="empty"><i class="bi bi-calendar3"></i><h6>${t('reg.empty.no_tx')}</h6><p>${t('reg.empty.add_first')}</p></div>`;
}

function renderRowsBars() {
  const top = document.getElementById('rowsTop'), bot = document.getElementById('rowsBottom');
  if (!top || !bot) return;
  if (rangeEmpty() || filtersActive() || !visibleAccounts().length) {
    top.style.display = 'none'; bot.style.display = 'none';
    return;
  }
  top.style.display = ''; bot.style.display = '';
  const days = visibleDays();
  const step = S.rows.step;
  const topEdge = S.win.from <= 0, botEdge = S.win.to >= days.length - 1;
  const expanded = S.extraAbove > 0 || S.extraBelow > 0;
  top.innerHTML = (topEdge
    ? `<span class="edge">${t('reg.rows.start')}</span>`
    : `<button data-rows="up"><i class="bi bi-chevron-up"></i> ${t('reg.rows.before', {n: step})}</button>`)
    + (expanded ? `<button class="ghost" data-rows="reset"><i class="bi bi-arrow-counterclockwise"></i> ${t('reg.rows.reset')}</button>` : '');
  bot.innerHTML = botEdge
    ? `<span class="edge">${t('reg.rows.end')}</span>`
    : `<button data-rows="down"><i class="bi bi-chevron-down"></i> ${t('reg.rows.after', {n: step})}</button>`;
}

function renderFoot() {
  const el = document.getElementById('regInfo');
  const bar = document.getElementById('resultBar');
  if (!el || !bar) return;
  if (filtersActive()) {
    const res = searchResults();
    let ent = 0, usc = 0;
    res.forEach(e => (e.cents > 0 ? ent += e.cents : usc -= e.cents));
    el.textContent = t('reg.filtered', {count: res.length});
    bar.classList.add('is-on');
    bar.innerHTML = `<span><i class="bi bi-funnel-fill"></i> <b>${res.length}</b> ${t('reg.results')}</span>
      <span>${t('kpi.income')} <b class="pos">${money(ent)}</b></span>
      <span>${t('kpi.expense')} <b class="neg">${money(usc)}</b></span>
      <span>${t('kpi.net')} <b class="${ent - usc < 0 ? 'neg' : 'pos'}">${money(ent - usc, true)}</b></span>
      <button class="btn-tool" style="margin-left:auto;height:28px" id="clearAll"><i class="bi bi-x-lg"></i> ${t('reg.clear_filters')}</button>`;
    return;
  }
  bar.classList.remove('is-on');
  bar.innerHTML = '';
  const days = visibleDays();
  if (rangeEmpty() || !days.length) { el.textContent = t('reg.no_days'); return; }
  const shown = Math.min(S.win.to, days.length - 1) - S.win.from + 1;
  let mov = 0;
  days.slice(S.win.from, S.win.to + 1).forEach(d => { mov += d.entries.length; });
  el.textContent = t('reg.days_movements', {days: shown, count: mov});
}

/* Dimensiona lo scroller del registro in modo che la pagina intera stia nel viewport:
   così a scorrere resta solo la tabella (intestazione sticky) e non c'è una seconda barra
   sulla pagina. Lo spazio sotto la tabella (barra righe, piè di pagina, padding) è misurato. */
function fitRegisterScroll() {
  const sc = document.getElementById('regScroll');
  if (!sc || isMobile() || !sc.offsetParent) { if (sc) sc.style.maxHeight = ''; return; }
  const card = sc.closest('.pcard');
  const main = document.getElementById('main');
  const below = card.getBoundingClientRect().bottom - sc.getBoundingClientRect().bottom
    + (main ? parseFloat(getComputedStyle(main).paddingBottom) || 0 : 0);
  const top = sc.getBoundingClientRect().top + window.scrollY;
  sc.style.maxHeight = Math.max(0, window.innerHeight - top - below - 2) + 'px';
}
function initRegisterFit() {
  let raf = 0;
  const run = () => { cancelAnimationFrame(raf); raf = requestAnimationFrame(fitRegisterScroll); };
  window.addEventListener('resize', run);
  if (window.ResizeObserver) {
    const ro = new ResizeObserver(run);
    ['kpis', 'resultBar', 'bulkBar', 'rowsTop', 'rowsBottom', 'impBar'].forEach(id => {
      const el = document.getElementById(id); if (el) ro.observe(el);
    });
    document.querySelectorAll('#page-registro .toolbar, #page-registro .reg-foot, #page-registro .topbar')
      .forEach(el => ro.observe(el));
  }
  run();
}

/* ---------- selezione multipla ---------- */
/* Selezione con rettangolo (come sul desktop): trascinando sul registro in modalità selezione si
   evidenziano le righe toccate; Ctrl/Cmd aggiunge alla selezione esistente, Ctrl+clic inverte la
   singola riga, Maiusc+clic seleziona l'intervallo. Il clic semplice seleziona solo quella riga. */
function initRubberBand() {
  const sc = document.getElementById('regScroll');
  if (!sc) return;
  let drag = null, lastClicked = null;
  const rows = () => [...sc.querySelectorAll('#regBody tr[data-id]:not(.quick-row)')];
  const paintRow = tr => {
    const on = S.selected.has(tr.dataset.id);
    tr.classList.toggle('is-sel', on);
    const cb = tr.querySelector('[data-sel]');
    if (cb) cb.checked = on;
  };
  const paintAll = () => { rows().forEach(paintRow); renderBulkBar(); };

  sc.addEventListener('mousedown', e => {
    if (!S.selMode || e.button !== 0) return;
    if (e.target.closest('input, button, a, select, textarea, label, .quick-row')) return;
    e.preventDefault();
    const additive = e.ctrlKey || e.metaKey;
    drag = {
      x: e.clientX, y: e.clientY + sc.scrollTop, cx: e.clientX, cy: e.clientY,
      additive, base: new Set(additive ? S.selected : []), moved: false,
      startRow: e.target.closest('tr[data-id]'), shift: e.shiftKey,
      box: null, timer: null,
    };
  });

  const update = () => {
    if (!drag) return;
    const top = Math.min(drag.y - sc.scrollTop, drag.cy), bottom = Math.max(drag.y - sc.scrollTop, drag.cy);
    const left = Math.min(drag.x, drag.cx), right = Math.max(drag.x, drag.cx);
    if (!drag.box) {
      drag.box = document.createElement('div');
      drag.box.className = 'rubber';
      document.body.appendChild(drag.box);
    }
    Object.assign(drag.box.style, {left: left + 'px', top: top + 'px', width: right - left + 'px', height: bottom - top + 'px'});
    S.selected = new Set(drag.base);
    rows().forEach(tr => {
      const r = tr.getBoundingClientRect();
      if (r.bottom >= top && r.top <= bottom && r.right >= left && r.left <= right) S.selected.add(tr.dataset.id);
    });
    paintAll();
  };
  /* Scorrimento automatico ai bordi mentre si trascina. */
  const autoScroll = () => {
    if (!drag) return;
    const r = sc.getBoundingClientRect();
    const dy = drag.cy < r.top + 30 ? -18 : drag.cy > r.bottom - 30 ? 18 : 0;
    if (dy) { sc.scrollTop += dy; update(); }
    drag.timer = requestAnimationFrame(autoScroll);
  };

  document.addEventListener('mousemove', e => {
    if (!drag) return;
    drag.cx = e.clientX; drag.cy = e.clientY;
    if (!drag.moved) {
      if (Math.abs(drag.cx - drag.x) + Math.abs(drag.cy - (drag.y - sc.scrollTop)) < 5) return;
      drag.moved = true;
      drag.timer = requestAnimationFrame(autoScroll);
    }
    update();
  });
  document.addEventListener('mouseup', e => {
    if (!drag) return;
    const d = drag; drag = null;
    if (d.timer) cancelAnimationFrame(d.timer);
    d.box?.remove();
    if (d.moved) { lastClicked = null; return; }
    const tr = d.startRow;
    if (!tr) { if (!d.additive) { S.selected.clear(); paintAll(); } return; }
    const id = tr.dataset.id;
    if (d.shift && lastClicked) {
      const list = rows().map(r => r.dataset.id);
      const a = list.indexOf(lastClicked), b = list.indexOf(id);
      if (a >= 0 && b >= 0) {
        if (!d.additive) S.selected.clear();
        list.slice(Math.min(a, b), Math.max(a, b) + 1).forEach(x => S.selected.add(x));
      }
    } else if (d.additive) {
      if (S.selected.has(id)) S.selected.delete(id); else S.selected.add(id);
      lastClicked = id;
    } else {
      S.selected = new Set([id]);
      lastClicked = id;
    }
    paintAll();
  });
}

function renderBulkBar() {
  const bar = document.getElementById('bulkBar');
  if (!bar) return;
  if (!S.selMode || S.selected.size === 0) { bar.classList.remove('is-on'); bar.innerHTML = ''; return; }
  let tot = 0, inc = 0, out = 0;
  LED.forEach(d => d.entries.forEach(e => {
    if (!S.selected.has(e.id)) return;
    tot += e.cents;
    if (e.cents > 0) inc += e.cents; else out += e.cents;
  }));
  /* Solo le categorie del verso dei movimenti selezionati (qui sono tutti dello stesso segno). */
  const catKind = inc > 0 ? 'in' : out < 0 ? 'out' : null;
  bar.classList.add('is-on');
  bar.innerHTML = `<span><b>${S.selected.size}</b> ${t('bulk.selected')} · ${t('bulk.total')} <b>${money(tot, true)}</b>
    · ${t('bulk.in')} <b>${money(inc, true)}</b> · ${t('bulk.out')} <b>${money(out, true)}</b></span>
    <span style="margin-left:auto;display:flex;gap:.4rem;flex-wrap:wrap">
      ${inc > 0 && out < 0 ? '' : `<select id="bulkCat" aria-label="${esc(t('bulk.change_cat'))}"
        style="background:transparent;color:inherit;border:1px solid rgba(255,255,255,.3);border-radius:8px;padding:.25rem .5rem;font-size:.82rem">
        <option value="">${t('bulk.change_cat')}</option>
        ${S.categories.filter(c => !catKind || c.kind === catKind).map(c => `<option value="${esc(c.id)}" style="color:#111">${esc(c.emoji)} ${esc(c.name)}</option>`).join('')}
      </select>`}
      ${S.tags.length ? `<select id="bulkTagAdd" aria-label="${esc(t('bulk.add_tag'))}"
        style="background:transparent;color:inherit;border:1px solid rgba(255,255,255,.3);border-radius:8px;padding:.25rem .5rem;font-size:.82rem">
        <option value="">${t('bulk.add_tag')}</option>
        ${S.tags.map(tg => `<option value="${esc(tg.id)}" style="color:#111">${esc(tg.label)}</option>`).join('')}
      </select>
      <select id="bulkTagRemove" aria-label="${esc(t('bulk.remove_tag'))}"
        style="background:transparent;color:inherit;border:1px solid rgba(255,255,255,.3);border-radius:8px;padding:.25rem .5rem;font-size:.82rem">
        <option value="">${t('bulk.remove_tag')}</option>
        ${S.tags.map(tg => `<option value="${esc(tg.id)}" style="color:#111">${esc(tg.label)}</option>`).join('')}
      </select>` : ''}
      <button id="bulkDel"><i class="bi bi-trash3"></i> ${t('common.delete')}</button>
      <button id="bulkOff">${t('common.cancel')}</button></span>`;
}

/* ---------- pannello salvadanai ---------- */
function virtualChildren() {
  if (S.accountId === 'all') return visibleAccounts().filter(a => a.kind === 'virtual');
  return visibleAccounts().filter(a => a.kind === 'virtual' && a.primary_account_id === S.accountId);
}

function renderVirtualPanel() {
  const kids = virtualChildren();
  const btn = document.getElementById('virtualBtn');
  const panel = document.getElementById('vPanel');
  if (!btn || !panel) return;
  if (!kids.length) { btn.style.display = 'none'; panel.classList.remove('is-on'); panel.innerHTML = ''; return; }
  const on = kids.filter(k => S.virtualOn.has(k.id)).length;
  btn.style.display = '';
  btn.classList.toggle('is-on', on < kids.length);
  const lbl = document.getElementById('virtualBtnLabel');
  if (lbl) lbl.textContent = `${t('reg.piggies')} ${on}/${kids.length}`;
  if (!panel.classList.contains('is-on')) return;
  const tot = kids.filter(k => S.virtualOn.has(k.id)).reduce((x, k) => x + balShown(k.id), 0);
  panel.innerHTML = `<h6>${t('reg.piggies_on_account')}</h6>` + kids.map(k => {
    const active = S.virtualOn.has(k.id);
    return `<label class="vrow ${active ? '' : 'off'}">
      <input type="checkbox" data-virt="${esc(k.id)}" ${active ? 'checked' : ''}>
      <span class="vn"><i class="bi ${iconOf(k)}"></i> ${esc(k.name)}
        ${k.note ? `<span style="font-weight:400;color:var(--text-muted);font-size:.78rem">${esc(k.note)}</span>` : ''}</span>
      <span class="vm num">${money(balShown(k.id))}</span></label>`;
  }).join('')
    + `<div class="vtot"><span>${t('reg.total_shown')}</span><span class="num">${money(tot)}</span></div>
       <p class="vnote">${t('reg.piggies_note')}</p>`;
}

/* ---------- barra laterale ---------- */
function acctItemHTML(a, extraCls = '') {
  const b = sideBal(a);
  const ro = !canWrite(a);
  const altrui = a.owner_id !== S.user?.id;
  return `<button class="acct-item ${S.accountId === a.id ? 'is-on' : ''} ${extraCls}" data-acct="${esc(a.id)}" title="${esc(a.name)}">
    <span class="ai"><i class="bi ${iconOf(a)}"></i></span>
    <span class="amid"><span class="an">${esc(a.name)}${ro ? ` <i class="bi bi-eye" title="${esc(t('reg.readonly'))}"></i>` : ''}</span>
      ${a.note || altrui ? `<span class="ano">${altrui ? t('acct.owned_by', {name: esc(userName(a.owner_id))}) + (a.note ? ' · ' : '') : ''}${esc(a.note || '')}</span>` : ''}</span>
    <span class="av num ${b < 0 ? 'neg' : ''}">${money(b)}</span></button>`;
}

function renderSidebarAccounts() {
  const list = document.getElementById('acctList');
  if (!list) return;
  const vis = visibleAccounts();
  const reali = vis.filter(a => a.kind !== 'virtual');
  const virtuali = vis.filter(a => a.kind === 'virtual');
  list.innerHTML = `<button class="acct-item ${S.accountId === 'all' ? 'is-on' : ''}" data-acct="all">
      <span class="ai"><i class="bi bi-collection"></i></span>
      <span class="amid"><span class="an">${t('reg.all_accounts')}</span></span>
      <span class="av num">${money(sideTotal())}</span></button>`
    + reali.map(a => acctItemHTML(a)).join('')
    + (virtuali.length ? `<div class="side-sub">${t('reg.piggies')}</div>` + virtuali.map(a => acctItemHTML(a, 'is-virtual')).join('') : '');
}

function renderRegTitle() {
  const h = document.getElementById('regTitle');
  if (!h) return;
  if (S.accountId === 'all') { h.innerHTML = `<i class="bi bi-collection"></i> ${t('reg.all_accounts')}`; return; }
  const a = acct(S.accountId);
  if (!a) { h.innerHTML = '—'; return; }
  h.innerHTML = `<i class="bi ${iconOf(a)}"></i> ${esc(a.name)} <span class="badge-tipo t-${tipoOf(a)}">${esc(kindLabel(a))}</span>`
    + (a.owner_id !== S.user?.id ? ` <span class="badge-ro">${t('acct.owned_by', {name: esc(userName(a.owner_id))})}</span>` : '')
    + (!canWrite(a) ? ` <span class="badge-ro"><i class="bi bi-eye"></i> ${t('reg.readonly_short')}</span>` : '');
}

/* Nasconde i comandi di scrittura sui conti in sola lettura. */
function syncWriteUI() {
  const rw = canWriteCurrent();
  ['newEntryBtn', 'fab', 'quickRowBtn', 'regImportBtn'].forEach(id => {
    const el = document.getElementById(id);
    if (el) el.style.display = rw ? '' : 'none';
  });
  if (!rw) S.quickRow = false;
  const sel = document.getElementById('selModeBtn');
  if (sel) sel.disabled = !LED.some(d => d.entries.some(e => e.id));
}

/* ---------- orchestrazione ---------- */
function paintRegister(keepScroll = false) {
  const scroller = isMobile() ? document.scrollingElement : document.getElementById('regScroll');
  const before = scroller ? scroller.scrollHeight : 0;
  const top = scroller ? scroller.scrollTop : 0;
  computeWindow();
  syncWriteUI();
  requestAnimationFrame(layoutToolbar);
  renderKPIs();
  renderTable();
  renderMobile();
  renderFoot();
  renderBulkBar();
  renderRowsBars();
  renderVirtualPanel();
  if (keepScroll && scroller) scroller.scrollTop = top + (scroller.scrollHeight - before);
  attachSwipe();
}

/* Ricarica dal server e ridisegna. Ogni azione che modifica dati passa da qui:
   niente aggiornamenti ottimistici che possano divergere dal database. */
async function refreshRegister(keepScroll = false) {
  try {
    await loadRegister();
  } catch (err) {
    fail(err, 'registro');
    LED = [];
  }
  paintRegister(keepScroll);
}

async function refreshAll() {
  try { await loadBalances(); } catch (err) { fail(err, 'saldi'); }
  try { await loadFeeSummaries(); } catch (err) { fail(err, 'spese conto'); }
  try { await loadTags(); } catch (err) { console.warn('tag non disponibili', err); }
  renderTagFilter();
  renderSidebarAccounts();
  renderRegTitle();
  await refreshRegister();
  if (S.page === 'conti') renderAccountsPage();
  if (S.page === 'statistiche') { stInvalidate(); renderStats(); }
  if (S.page === 'ricorrenti') renderRecurrences();
  if (S.page === 'trasferimenti') renderTransfers();
}

async function applyPeriod(p, {silent = false} = {}) {
  S.period = p;
  S.extraAbove = 0; S.extraBelow = 0;
  if (p === 'around') {
    S.winRange = null;
    S.month = TODAY.slice(0, 7);
    S.anchor = TODAY;
  } else {
    const {from, to} = periodRange(p);
    S.winRange = {from, to};
    if (p === 'custom') {
      /* I KPI dei conti non-carta guardano un solo mese: quello corrente se
         rientra nell'intervallo, altrimenti l'ultimo scelto. */
      const cur = TODAY.slice(0, 7);
      S.month = cur >= S.range.k1 && cur <= S.range.k2 ? cur : S.range.k2;
    } else S.month = p === 'lastMonth' ? from.slice(0, 7) : (p === 'month' ? S.month : TODAY.slice(0, 7));
    S.anchor = (TODAY >= from && TODAY <= to) ? TODAY : from;
  }
  syncPeriodUI();
  await refreshRegister();
  if (!silent) requestAnimationFrame(() => scrollToAnchor());
}

function scrollToAnchor() {
  const key = S.anchor || TODAY;
  if (isMobile()) {
    const nodes = [...document.querySelectorAll('#mobileList .m-day')];
    const n = nodes.find(x => x.querySelector('.d-l')?.textContent.includes(fmtDayLong(key)));
    if (n) window.scrollTo({top: n.getBoundingClientRect().top + window.scrollY - 100, behavior: 'smooth'});
  } else {
    const sc = document.getElementById('regScroll');
    if (!sc) return;
    const rows = [...document.querySelectorAll('#regBody tr')];
    const n = rows.find(r => r.querySelector('.c-data')?.textContent.startsWith(fmtDate(key)));
    if (n) sc.scrollTop = Math.max(0, n.offsetTop - sc.clientHeight / 3);
  }
}

async function extendWindow(dir) {
  if (dir === 'reset') { S.extraAbove = 0; S.extraBelow = 0; }
  else if (dir < 0) S.extraAbove += S.rows.step;
  else S.extraBelow += S.rows.step;
  await refreshRegister(dir < 0);
}

async function setAccount(id) {
  const vis = visibleAccounts();
  if (id !== 'all' && !vis.some(a => a.id === id)) id = 'all';
  S.accountId = id;
  S.selected.clear();
  S.interest = S.interest || {};
  // Il "mese" di una carta copre il ciclo di chiusura, non il mese solare:
  // ricalcolo la finestra quando si passa da/verso una carta col mese attivo.
  if (S.period !== 'around') {
    const {from, to} = periodRange(S.period);
    S.winRange = {from, to};
    S.anchor = (TODAY >= from && TODAY <= to) ? TODAY : from;
  }
  renderSidebarAccounts();
  renderRegTitle();
  await refreshRegister();
  loadInterestIfDeposit();
}

/* Il KPI degli interessi vive su un endpoint separato: lo chiedo solo quando
   il conto selezionato è un deposito, e ridisegno i KPI al ritorno. */
function loadInterestIfDeposit() {
  const a = S.accountId === 'all' ? null : acct(S.accountId);
  if (!a || a.kind !== 'deposit') return;
  const {first, last} = monthBounds(S.month);
  api(`/accounts/${encodeURIComponent(a.id)}/deposit/interest?from=${a.start_date || first}&to=${last}`)
    .then(res => { S.interest = {...(S.interest || {}), [a.id]: res}; renderKPIs(); })
    .catch(err => console.warn('interessi non disponibili', err));
}

async function focusTx(id, preloaded) {
  let target = preloaded || null;
  if (!target) {
    LED.forEach(d => d.entries.forEach(e => { if (e.id === id) target = e; }));
  }
  if (!target) { toast(t('reg.tx_gone')); return; }
  const targetAcct = target.acct || target.account_id;
  clearAllFilters(true);
  S.hlTx = id;
  if (S.accountId !== 'all' && S.accountId !== targetAcct) { S.accountId = targetAcct; renderSidebarAccounts(); renderRegTitle(); }
  S.month = target.date.slice(0, 7);
  S.winRange = null; S.period = 'around'; S.anchor = target.date;
  syncPeriodUI();
  if (S.page !== 'registro') goPage('registro');
  await refreshRegister();
  requestAnimationFrame(() => {
    const el = document.querySelector(`#regBody tr[data-id="${id}"]`) || document.querySelector(`#mobileList .m-row[data-id="${id}"]`);
    if (!el) return;
    if (isMobile()) window.scrollTo({top: el.getBoundingClientRect().top + window.scrollY - 120, behavior: 'smooth'});
    else { const sc = document.getElementById('regScroll'); if (sc) sc.scrollTop = el.offsetTop - sc.clientHeight / 2.5; }
  });
  setTimeout(() => { if (S.hlTx === id) { S.hlTx = null; paintRegister(); } }, 4500);
}

function focusRecurrence(recId) {
  S.hlRec = recId;
  goPage('ricorrenti');
  requestAnimationFrame(() => {
    const el = document.querySelector(`#recBody tr[data-rec-row="${recId}"]`);
    if (el) el.scrollIntoView({block: 'center', behavior: 'smooth'});
  });
  setTimeout(() => { S.hlRec = null; if (S.page === 'ricorrenti') renderRecurrences(); }, 4500);
}

async function scrollToDate(key) {
  if (filtersActive()) clearAllFilters(true);
  S.winRange = null; S.period = 'around'; S.anchor = key;
  S.extraAbove = 0; S.extraBelow = 0;
  syncPeriodUI();
  await refreshRegister();
  requestAnimationFrame(() => scrollToAnchor());
}

/* ---------- swipe mobile ---------- */
function attachSwipe() {
  if (!isMobile()) return;
  document.querySelectorAll('#mobileList .m-row').forEach(row => {
    if (row.dataset.swipe || row.classList.contains('ro')) return;
    row.dataset.swipe = '1';
    const front = row.querySelector('.m-front');
    if (!front) return;
    let x0 = 0, y0 = 0, dx = 0, drag = false;
    row.addEventListener('touchstart', e => {
      x0 = e.touches[0].clientX; y0 = e.touches[0].clientY; dx = 0; drag = false;
      front.style.transition = 'none';
    }, {passive: true});
    row.addEventListener('touchmove', e => {
      const cx = e.touches[0].clientX - x0, cy = e.touches[0].clientY - y0;
      if (!drag && Math.abs(cx) > 10 && Math.abs(cx) > Math.abs(cy)) drag = true;
      if (drag) {
        dx = Math.max(-132, Math.min(0, row.classList.contains('swiped') ? cx - 132 : cx));
        front.style.transform = `translateX(${dx}px)`;
      }
    }, {passive: true});
    row.addEventListener('touchend', () => {
      front.style.transition = '';
      front.style.transform = '';
      if (drag) row.classList.toggle('swiped', dx < -60);
    });
  });
}

/* ============================================================
   RIGA RAPIDA (desktop, dentro la tabella)
   Ha data, selettore −/+, importo, descrizione con suggerimenti,
   conto quando si guardano tutti i conti, e categorie filtrate
   per tipo. Il segno è esplicito: è la correzione di B1.
   ============================================================ */
let qTipo = 'uscita';
/* true quando la categoria della riga rapida è stata scelta a mano: l'autocompletamento non la tocca. */
let qCatTouched = false;
/* Ultima data usata nella riga rapida: resta per gli inserimenti successivi. */
let qLastDate = null;
let qCatBuf = '', qCatBufAt = 0;

function catOptionsFor(tipo) {
  const kind = tipo === 'entrata' ? 'in' : 'out';
  return S.categories.filter(c => c.kind === kind)
    .map(c => `<option value="${esc(c.id)}">${esc(c.emoji)} ${esc(c.name)}</option>`).join('');
}

function injectQuickRow() {
  const body = document.getElementById('regBody');
  if (!body) return;
  if (body.querySelector('.quick-row')) return;
  qCatTouched = false;
  const tr = document.createElement('tr');
  tr.className = 'quick-row';
  tr.innerHTML = `${S.selMode ? '<td class="c-sel"></td>' : ''}
    <td class="c-data"><input type="date" id="qDate" autocomplete="off" value="${qLastDate || (S.anchor && S.anchor <= TODAY ? S.anchor : TODAY)}" aria-label="${esc(t('common.date'))}"></td>
    <td class="c-saldo" style="text-align:right;color:var(--text-muted);font-weight:500">—</td>
    <td class="c-imp"><div style="display:flex;gap:.3rem;align-items:center">
      <div class="seg" id="qTipo" role="group" aria-label="${esc(t('common.type'))}">
        <button type="button" data-tipo="uscita" class="${qTipo === 'uscita' ? 'is-on' : ''}">−</button>
        <button type="button" data-tipo="entrata" class="${qTipo === 'entrata' ? 'is-on' : ''}">+</button></div>
      <input type="text" inputmode="decimal" class="qi-imp" id="qAmount" autocomplete="off" placeholder="${formatAmountInput(0)}" aria-label="${esc(t('common.amount'))}">
    </div></td>
    <td class="c-desc"><input type="text" id="qDesc" autocomplete="off" placeholder="${esc(t('reg.quick.desc_placeholder'))}"></td>
    ${S.accountId === 'all' ? `<td class="c-acct"><select id="qAcct" aria-label="${esc(t('stat.col.conto'))}">${writableAccounts()
      .filter(a => a.kind !== 'virtual').map(a => accountOption(a)).join('')}</select></td>` : ''}
    <td class="c-cat"><select id="qCat" aria-label="${esc(t('common.category'))}">${catOptionsFor(qTipo)}</select></td>
    <td class="c-az"><button class="rowbtn" id="qSave" aria-label="${esc(t('reg.quick.save'))}"><i class="bi bi-check-lg"></i></button></td>`;
  body.prepend(tr);
  if (S.quickFocus) { tr.querySelector('#qAmount')?.focus(); S.quickFocus = false; }
}

async function saveQuickRow() {
  const amtEl = document.getElementById('qAmount');
  const cents = parseAmount(amtEl?.value);
  if (!cents) { toast(t('toast.amount_required')); amtEl?.focus(); return; }
  const catSel = document.getElementById('qCat');
  const cat = catSel?.value || null;
  const target = S.accountId === 'all'
    ? (document.getElementById('qAcct')?.value || writableAccounts()[0]?.id)
    : S.accountId;
  if (!target) { toast(t('toast.no_writable')); return; }
  const desc = (document.getElementById('qDesc')?.value || '').trim() || catName(cat);
  const date = document.getElementById('qDate')?.value || TODAY;
  const signed = qTipo === 'entrata' ? cents : -cents;
  const btn = document.getElementById('qSave');
  if (btn) btn.disabled = true;
  try {
    await createTransaction({account_id: target, date, cents: signed, description: desc, category_id: cat});
    qLastDate = date;
    S.quickFocus = true;
    await refreshAll();
    toast(t(signed > 0 ? 'toast.in_saved' : 'toast.out_saved', {amount: money(cents)}));
  } catch (err) {
    fail(err, 'riga rapida');
  } finally {
    const b = document.getElementById('qSave');
    if (b) b.disabled = false;
  }
}

/* La coda offline e la riga rapida spediscono lo stesso payload: un solo
   punto di creazione, così la correzione del segno vale per entrambe. */
async function createTransaction(fields) {
  const clientId = newClientId();
  const body = {...fields, client_id: clientId};
  if (!body.description) delete body.description;
  if (!S.offlineQueue) return api('/transactions', {method: 'POST', body: JSON.stringify(body)});
  /* Sempre attraverso la coda: così anche un errore di rete a browser
     "online" mette la riga in attesa invece di perderla. La coda applica il
     prefisso /api e considera un fallimento una risposta non JSON. */
  const res = await S.offlineQueue.post('/transactions', body, clientId);
  if (res.queued) toast(t('toast.queued_offline'));
  return res;
}

/* ============================================================
   SHEET NUOVA VOCE
   ============================================================ */
let editingId = null;
let editingRec = null;   // {recId, date} quando si modifica una riga prevista

function findEntry(id) {
  let found = null;
  LED.forEach(d => d.entries.forEach(e => { if (e.id === id) found = e; }));
  return found;
}

function findForecast(recId, date) {
  let found = null;
  LED.forEach(d => d.entries.forEach(e => { if (e.recId === recId && e.date === date && !e.id) found = e; }));
  return found;
}

function openEntry(txId = null, forecast = null) {
  if (forecast) {
    const e = findForecast(forecast.recId, forecast.date);
    if (!e) return;
    const rec = S.recurrences.find(r => r.id === forecast.recId);
    if (!canWrite(acct(e.acct))) { toast(t('toast.readonly')); return; }
    editingId = null;
    editingRec = {recId: forecast.recId, date: forecast.date};
    fillEntry(e, rec, true);
    return;
  }
  if (txId) {
    const e = findEntry(txId);
    if (!e) return;
    if (!canWrite(acct(e.acct))) { toast(t('toast.readonly')); return; }
    editingId = txId;
    editingRec = null;
    const rec = e.recId ? S.recurrences.find(r => r.id === e.recId) : null;
    fillEntry(e, rec, false);
    return;
  }
  if (!writableAccounts().length) { toast(t('toast.no_writable')); return; }
  editingId = null;
  editingRec = null;
  fillEntry(null, null, false);
}

function fillEntry(e, rec, isForecast) {
  S.entry.tipo = e ? (e.cents > 0 ? 'entrata' : 'uscita') : 'uscita';
  S.entry.date = e ? e.date : (S.anchor && S.anchor <= TODAY ? S.anchor : TODAY);
  S.entry.cat = e ? e.cat : null;
  S.entry.tags = new Set(e && e.id ? (S.tagMap[e.id] || []) : []);
  S.entry.acct = e ? e.acct : (S.accountId !== 'all' ? S.accountId : writableAccounts()[0]?.id || null);
  /* Conto non modificabile: righe previste (nessun movimento da spostare) e righe
     che il server dichiara bloccate. Una voce nuova (e == null) sceglie sempre. */
  S.entry.acctLocked = !!(e && (isForecast || e.locked));
  document.getElementById('entryTitle').textContent = e ? t('modal.entry.edit') : t('modal.entry.title');
  document.getElementById('eAmount').value = e ? formatAmountInput(Math.abs(e.cents)) : '';
  document.getElementById('eDesc').value = e ? e.desc : '';
  document.getElementById('eNote').value = e?.note || '';
  document.getElementById('eRecurring').checked = false;
  document.getElementById('eRecurring').closest('.form-check').style.display = e ? 'none' : '';
  document.getElementById('eRecBox').style.display = 'none';
  document.getElementById('moreBox').classList.remove('is-on');
  document.getElementById('eDate').value = S.entry.date;

  const notice = document.getElementById('recNotice');
  if (rec) {
    notice.style.display = '';
    document.getElementById('recNoticeTitle').textContent = t('modal.entry.from_rec', {name: rec.description});
    document.getElementById('recNoticeText').textContent = rec.mode === 'avg'
      ? t('modal.entry.rec_avg', {n: rec.avg_n || 6})
      : t('modal.entry.rec_fixed', {amount: money(rec.amount_cents)});
    document.getElementById('recApplyAll').checked = false;
  } else {
    notice.style.display = 'none';
  }
  document.getElementById('eDate').disabled = !!isForecast;

  syncTipo();
  renderCatGrid();
  renderTagChips();
  document.getElementById('tagBox').style.display = isForecast ? 'none' : '';
  renderAcctChips();
  syncDateChips();
  /* Su una riga prevista non esiste ancora un movimento: l'override della
     regola accetta solo importo e descrizione, quindi categoria, conto e data
     restano quelli della regola invece di dare l'illusione di essere
     modificabili. Uso l'attributo `disabled` dei pulsanti veri: nessuna classe
     inventata, nessun CSS aggiunto. */
  /* I pulsanti del conto li gestisce renderAcctChips (S.entry.acctLocked). */
  document.querySelectorAll('#catGrid button, #dateChips .chip[data-day]')
    .forEach(b => { b.disabled = !!isForecast; });
  document.getElementById('entrySheet').classList.add('is-on');
  document.body.style.overflow = 'hidden';
  setTimeout(() => document.getElementById('eAmount').focus(), 120);
}

function closeEntry() {
  document.getElementById('entrySheet')?.classList.remove('is-on');
  document.body.style.overflow = '';
  editingId = null;
  editingRec = null;
}

function syncTipo() {
  document.querySelectorAll('#tipoToggle button').forEach(b => b.classList.toggle('is-on', b.dataset.tipo === S.entry.tipo));
  document.getElementById('amountField')?.classList.toggle('is-entrata', S.entry.tipo === 'entrata');
  const hint = document.getElementById('amountHint');
  if (hint) hint.textContent = S.entry.tipo === 'uscita' ? t('modal.entry.hint_out') : t('modal.entry.hint_in');
}

function renderCatGrid() {
  const el = document.getElementById('catGrid');
  if (!el) return;
  const kind = S.entry.tipo === 'entrata' ? 'in' : 'out';
  const list = S.categories.filter(c => c.kind === kind);
  if (S.entry.cat && !list.some(c => c.id === S.entry.cat)) S.entry.cat = null;
  el.innerHTML = list.length
    ? list.map(c => `<button type="button" class="cat-cell ${S.entry.cat === c.id ? 'is-on' : ''}" data-cat="${esc(c.id)}">
        <span class="e">${esc(c.emoji)}</span><span class="n">${esc(c.name)}</span></button>`).join('')
    : `<p style="font-size:.82rem;color:var(--text-muted);margin:0">${t('modal.entry.no_cats')}</p>`;
  const hint = document.getElementById('catHint');
  if (hint) hint.textContent = S.entry.cat ? '' : t('modal.entry.pick_one');
}

function renderAcctChips() {
  const el = document.getElementById('acctChips');
  if (!el) return;
  const locked = !!S.entry.acctLocked;
  /* Spiega perché i pulsanti sono spenti, ma solo per le voci reali bloccate
     (sulle righe previste vale già il resto del modale). */
  el.title = locked && !editingRec ? t('modal.entry.acct_locked') : '';
  el.innerHTML = writableAccounts().map(a =>
    `<button type="button" class="chip ${S.entry.acct === a.id ? 'is-on' : ''}" data-eacct="${esc(a.id)}" ${locked ? 'disabled' : ''} title="${esc(accountOptionLabel(a))}"><i class="bi ${iconOf(a)}"></i> ${esc(a.name)}${a.note ? ` <span style="font-weight:400;opacity:.7;font-size:.78rem">${esc(a.note.length > 40 ? a.note.slice(0, 39) + '…' : a.note)}</span>` : ''}</button>`).join('');
}

function syncDateChips() {
  document.querySelectorAll('#dateChips .chip[data-day]').forEach(b =>
    b.classList.toggle('is-on', addDays(TODAY, Number(b.dataset.day)) === S.entry.date));
  const d = document.getElementById('eDate');
  if (d) d.value = S.entry.date;
  const echo = document.getElementById('dateEcho');
  if (echo) echo.textContent = fmtDayLong(S.entry.date);
  const d2 = addDays(TODAY, -2);
  const chip = document.getElementById('chipTwo');
  if (chip) chip.textContent = fromISO(d2).getDate() + ' ' + MONTHS()[fromISO(d2).getMonth()].slice(0, 3);
}

/* Autocompletamento della descrizione (riga rapida #qDesc e modale #eDesc).
   Le proposte vengono dalle righe già in memoria: nessuna richiesta aggiuntiva.
   Ricerca per sottostringa, le più frequenti per prime; frecce/Invio/Esc. */
const AC = {box: null, input: null, items: [], idx: -1, timer: null, seq: 0, remote: new Map()};
const AC_MIN_REMOTE = 3;

/* Proposte locali: una per descrizione (senza distinguere maiuscole), con
   importo e categoria dell'uso più recente. Trasferimenti e righe previste
   restano fuori. */
/* Segno atteso dal campo: uscita = -1, entrata = +1. Le proposte di segno opposto non hanno senso. */
function acSign(input) {
  const tipo = input?.id === 'qDesc' ? qTipo : S.entry?.tipo;
  return tipo === 'entrata' ? 1 : -1;
}
const acSameSign = (cents, sign) => !cents || (cents > 0 ? 1 : -1) === sign;

function acPool() {
  const seen = new Map();
  LED.forEach(d => d.entries.forEach(e => {
    if (!e.desc || e.pair || e.cardForecast || (e.recId && !e.id)) return;
    const d0 = e.desc.trim();
    if (!d0) return;
    const k = d0.toLowerCase() + (e.cents > 0 ? '+' : '-');
    const cur = seen.get(k);
    if (!cur) seen.set(k, {d: d0, n: 1, cents: e.cents, cat: e.cat || null, date: e.date});
    else {
      cur.n++;
      if (e.date > cur.date) Object.assign(cur, {d: d0, cents: e.cents, cat: e.cat || null, date: e.date});
    }
  }));
  return [...seen.values()];
}

function acLocal(q, sign) {
  return acPool()
    .filter(s => acSameSign(s.cents, sign))
    .filter(s => !q || (s.d.toLowerCase().includes(q) && s.d.toLowerCase() !== q))
    .sort((a, b) => {
      const sa = a.d.toLowerCase().startsWith(q) ? 0 : 1, sb = b.d.toLowerCase().startsWith(q) ? 0 : 1;
      return sa - sb || b.n - a.n || a.d.localeCompare(b.d);
    }).slice(0, 8);
}

/* Fallback sul server, solo senza corrispondenze locali e da 3 caratteri in su.
   `fetch` diretto (non `api`): niente indicatore di attività per un suggerimento,
   e un errore di rete o di sessione qui non deve disturbare chi sta scrivendo. */
function acRemote(input, q) {
  clearTimeout(AC.timer);
  if (q.length < AC_MIN_REMOTE) return;
  const sign = acSign(input);
  const show = list => { if (AC.input === input && input.value.trim().toLowerCase() === q) acRender(input, q, list.filter(x => acSameSign(x.cents, sign))); };
  const ck = `${sign}|${q}`;
  if (AC.remote.has(ck)) { show(AC.remote.get(ck)); return; }
  const seq = ++AC.seq;
  AC.timer = setTimeout(async () => {
    try {
      const res = await fetch(`${API}/transactions/suggest?q=${encodeURIComponent(q)}&sign=${sign}`, {credentials: 'same-origin'});
      if (!res.ok || seq !== AC.seq) return;
      const list = (await res.json()).map(s => ({d: s.description, n: s.count, cents: s.cents, cat: s.category_id || null}));
      AC.remote.set(ck, list);
      if (AC.remote.size > 50) AC.remote.delete(AC.remote.keys().next().value);
      if (seq === AC.seq) show(list);
    } catch { /* offline o sessione scaduta: nessun suggerimento */ }
  }, 250);
}

function acHide() {
  clearTimeout(AC.timer);
  AC.seq++;
  AC.box?.classList.remove('is-on');
  AC.items = []; AC.idx = -1; AC.input = null;
}

function acShow(input) {
  const q = input.value.trim().toLowerCase();
  AC.input = input;
  const hits = acLocal(q, acSign(input));
  if (hits.length) { AC.seq++; clearTimeout(AC.timer); acRender(input, q, hits); return; }
  AC.box?.classList.remove('is-on');
  AC.items = []; AC.idx = -1;
  acRemote(input, q);
}

function acRender(input, q, hits) {
  if (!hits.length) { AC.box?.classList.remove('is-on'); AC.items = []; AC.idx = -1; return; }
  if (!AC.box) {
    AC.box = document.createElement('div');
    AC.box.className = 'ac-box';
    AC.box.setAttribute('role', 'listbox');
    /* mousedown (non click) e preventDefault: l'input non deve perdere il focus. */
    AC.box.addEventListener('mousedown', e => {
      const it = e.target.closest('.ac-item');
      if (!it) return;
      e.preventDefault();
      acPick(AC.items[+it.dataset.i]);
    });
    document.body.appendChild(AC.box);
  }
  AC.input = input; AC.items = hits; AC.idx = -1;
  AC.box.innerHTML = hits.map((s, n) => {
    const i = q ? s.d.toLowerCase().indexOf(q) : -1;
    const label = i < 0 ? esc(s.d)
      : `${esc(s.d.slice(0, i))}<mark>${esc(s.d.slice(i, i + q.length))}</mark>${esc(s.d.slice(i + q.length))}`;
    return `<div class="ac-item" role="option" data-i="${n}">${label}</div>`;
  }).join('');
  const r = input.getBoundingClientRect();
  AC.box.style.left = r.left + 'px';
  AC.box.style.minWidth = r.width + 'px';
  const below = window.innerHeight - r.bottom;
  if (below < 200 && r.top > below) { AC.box.style.top = 'auto'; AC.box.style.bottom = (window.innerHeight - r.top + 2) + 'px'; }
  else { AC.box.style.bottom = 'auto'; AC.box.style.top = (r.bottom + 2) + 'px'; }
  AC.box.classList.add('is-on');
}

function acPick(s) {
  const inp = AC.input;
  if (!inp || !s) return;
  inp.value = s.d;
  acFill(inp, s);
  acHide();
  inp.focus();
}

/* Dopo la scelta di una descrizione: importo e categoria dell'ultimo uso, ma
   solo dove l'utente non ha già deciso. Il tipo (uscita/entrata) può seguire la
   proposta soltanto se non è stato impegnato né importo né categoria. */
function acFill(inp, s) {
  if (!s.cents) return;
  const tipoOf = c => c < 0 ? 'uscita' : 'entrata';
  const catOk = (tipo, id) => !!id && S.categories.some(c => c.id === id && c.kind === (tipo === 'entrata' ? 'in' : 'out'));
  if (inp.id === 'qDesc') {
    const amt = document.getElementById('qAmount'), sel = document.getElementById('qCat');
    const noAmount = !parseAmount(amt?.value);
    if (noAmount && !qCatTouched && tipoOf(s.cents) !== qTipo) {
      qTipo = tipoOf(s.cents);
      document.querySelectorAll('#qTipo button').forEach(x => x.classList.toggle('is-on', x.dataset.tipo === qTipo));
      if (sel) sel.innerHTML = catOptionsFor(qTipo);
    }
    if (noAmount && tipoOf(s.cents) === qTipo && amt) amt.value = formatAmountInput(Math.abs(s.cents));
    if (!qCatTouched && sel && catOk(qTipo, s.cat)) sel.value = s.cat;
  } else if (inp.id === 'eDesc') {
    const amt = document.getElementById('eAmount');
    const noAmount = !parseAmount(amt?.value);
    if (noAmount && !S.entry.cat && !editingRec && tipoOf(s.cents) !== S.entry.tipo) S.entry.tipo = tipoOf(s.cents);
    if (noAmount && tipoOf(s.cents) === S.entry.tipo) amt.value = formatAmountInput(Math.abs(s.cents));
    if (!S.entry.cat && !editingRec && catOk(S.entry.tipo, s.cat)) S.entry.cat = s.cat;
    syncTipo();
    renderCatGrid();
  }
}

function acMove(step) {
  const els = AC.box.querySelectorAll('.ac-item');
  AC.idx = (AC.idx + step + els.length) % els.length;
  els.forEach((el, i) => el.classList.toggle('is-act', i === AC.idx));
  els[AC.idx].scrollIntoView({block: 'nearest'});
}

function initDescAutocomplete() {
  const isDesc = el => el && el.matches?.('#qDesc, #eDesc');
  document.addEventListener('input', e => { if (isDesc(e.target)) acShow(e.target); });
  document.addEventListener('focusin', e => { if (isDesc(e.target) && e.target.value.trim() === '') acShow(e.target); });
  document.addEventListener('focusout', e => { if (isDesc(e.target)) acHide(); });
  /* L'utente ha scelto lui la categoria della riga rapida: non va sovrascritta. */
  document.addEventListener('change', e => { if (e.target.id === 'qCat') qCatTouched = true; });
  window.addEventListener('resize', acHide);
  document.addEventListener('scroll', acHide, true);
  /* Fase di cattura: prima dei gestori di Invio/Esc della riga rapida e della modale. */
  document.addEventListener('keydown', e => {
    if (!isDesc(e.target) || !AC.box?.classList.contains('is-on')) return;
    if (e.key === 'ArrowDown') { e.preventDefault(); acMove(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); acMove(-1); }
    else if (e.key === 'Enter' && AC.idx >= 0) { e.preventDefault(); e.stopPropagation(); acPick(AC.items[AC.idx]); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); acHide(); }
    else if (e.key === 'Tab' && AC.idx >= 0) { acPick(AC.items[AC.idx]); }
  }, true);
}

/* Ctrl+Z / Ctrl+Y: pile di annulla e ripeti, ogni voce è lo stato da ripristinare. */
const UNDO_STACK = [];
const REDO_STACK = [];
function snapshotEntry(e) {
  return {id: e.id, account_id: e.acct, date: e.date, cents: e.cents, description: e.desc, category_id: e.cat || null, note: e.note || ''};
}
function pushUndo(prev) { UNDO_STACK.push(prev); if (UNDO_STACK.length > 20) UNDO_STACK.shift(); REDO_STACK.length = 0; }
async function applyHistory(from, to, msg) {
  const target = from.pop();
  if (!target) return;
  const cur = findEntry(target.id);
  const {id, ...fields} = target;
  try {
    await api(`/transactions/${encodeURIComponent(id)}`, {method: 'PUT', body: JSON.stringify(fields)});
    if (cur) to.push(snapshotEntry(cur));
    await refreshAll();
    toast(msg);
  } catch (err) {
    from.push(target);
    fail(err, 'cronologia modifiche');
  }
}
const undoLastEdit = () => applyHistory(UNDO_STACK, REDO_STACK, 'Modifica annullata');
const redoLastEdit = () => applyHistory(REDO_STACK, UNDO_STACK, 'Modifica ripristinata');

async function saveEntry(keepOpen = false) {
  const cents = parseAmount(document.getElementById('eAmount').value);
  if (!cents) { toast(t('toast.amount_required')); document.getElementById('eAmount').focus(); return; }
  const signed = S.entry.tipo === 'entrata' ? cents : -cents;
  const desc = document.getElementById('eDesc').value.trim();
  const note = document.getElementById('eNote').value.trim();
  const btns = ['saveEntry', 'saveAndNew'].map(id => document.getElementById(id));
  btns.forEach(b => { if (b) b.disabled = true; });

  try {
    /* Riga prevista: non c'è un movimento da aggiornare, c'è un'occorrenza da
       fissare. `PUT /recurrences/{id}/overrides/{date}` fa esattamente questo,
       e `recApplyAll` ribasa anche la regola. */
    if (editingRec) {
      await api(`/recurrences/${encodeURIComponent(editingRec.recId)}/overrides/${editingRec.date}`, {
        method: 'PUT',
        body: JSON.stringify({
          amount_cents: signed,
          description: desc || null,
          update_rule: document.getElementById('recApplyAll').checked,
        }),
      });
      const rec = S.recurrences.find(r => r.id === editingRec.recId);
      closeEntry();
      await loadRecurrences();
      await refreshAll();
      toast(document.getElementById('recApplyAll')?.checked
        ? t('toast.rec_and_row', {name: rec?.description || ''})
        : t('toast.occurrence_set'));
      return;
    }

    if (!S.entry.cat) { toast(t('toast.cat_required')); return; }

    if (editingId) {
      const before = findEntry(editingId);
      const body = {date: S.entry.date, cents: signed, description: desc || null, category_id: S.entry.cat, note: note || null};
      /* Il conto si invia solo se è cambiato e la voce non è bloccata: il server rifiuta lo spostamento delle voci speciali. */
      if (before && !S.entry.acctLocked && S.entry.acct && S.entry.acct !== before.acct) body.account_id = S.entry.acct;
      await api(`/transactions/${encodeURIComponent(editingId)}`, {method: 'PUT', body: JSON.stringify(body)});
      if (before) pushUndo(snapshotEntry(before));
      await saveEntryTags(editingId, S.entry.tags);
      closeEntry();
      await refreshAll();
      toast(t('toast.entry_updated'));
      return;
    }

    if (!S.entry.acct) { toast(t('toast.no_writable')); return; }
    const created = await createTransaction({
      account_id: S.entry.acct,
      date: S.entry.date,
      cents: signed,
      description: desc || catName(S.entry.cat),
      category_id: S.entry.cat,
      note: note || null,
    });
    if (created?.id && S.entry.tags.size) await saveEntryTags(created.id, S.entry.tags);

    /* "Si ripete nel tempo": crea anche la regola, con lo stesso segno
       dell'importo, perché `amount_cents` della ricorrenza è firmato. */
    if (document.getElementById('eRecurring').checked) {
      const freq = document.querySelector('#recQuick .chip.is-on')?.dataset.freq || 'monthly';
      /* Il 31 non esiste in tutti i mesi: la ricorrenza rapida mensile usa l'ultimo giorno. */
      const lastDay = fromISO(S.entry.date).getDate() === 31;
      const pattern = {monthly: lastDay ? 'FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=-1' : 'FREQ=MONTHLY;INTERVAL=1', weekly: 'FREQ=WEEKLY;INTERVAL=1', yearly: 'FREQ=YEARLY;INTERVAL=1'}[freq];
      try {
        await api('/recurrences', {
          method: 'POST',
          body: JSON.stringify({
            description: desc || catName(S.entry.cat),
            category_id: S.entry.cat,
            kind: S.entry.tipo === 'entrata' ? 'in' : 'out',
            amount_cents: signed,
            account_id: S.entry.acct,
            pattern,
            weekend: 'create',
            first_date: S.entry.date,
            end_type: 'never',
            mode: 'fixed',
            avg_n: 6,
            active: true,
          }),
        });
        await loadRecurrences();
      } catch (err) {
        fail(err, 'ricorrenza rapida');
      }
    }

    if (keepOpen) {
      document.getElementById('eAmount').value = '';
      document.getElementById('eDesc').value = '';
      document.getElementById('eAmount').focus();
    } else {
      closeEntry();
    }
    await refreshAll();
    toast(t(signed > 0 ? 'toast.in_saved' : 'toast.out_saved', {amount: money(cents)}));
  } catch (err) {
    fail(err, 'salvataggio voce');
  } finally {
    btns.forEach(b => { if (b) b.disabled = false; });
  }
}

/* ---------- pattern di conferma per le eliminazioni (armed click) ---------- */
function armedClick(btn, toastKey, action, params = {}) {
  if (btn.dataset.armed !== '1') {
    // Primo click: armamento
    btn.dataset.armed = '1';
    btn.classList.add('is-armed');
    // Cambia solo la classe dell'<i>, non l'innerHTML, così resta valido il testo del bottone
    const icon = btn.querySelector('i');
    if (icon) {
      icon.classList.remove('bi-trash3');
      icon.classList.add('bi-exclamation-triangle');
    }
    btn.title = t(toastKey, params);
    if (btn.getAttribute('aria-label')) btn.setAttribute('aria-label', t(toastKey, params));
    toast(t(toastKey, params));
    // Ripristino dopo 5000ms
    setTimeout(() => {
      if (btn.isConnected) {
        btn.dataset.armed = '';
        btn.classList.remove('is-armed');
        const i = btn.querySelector('i');
        if (i) {
          i.classList.remove('bi-exclamation-triangle');
          i.classList.add('bi-trash3');
        }
        btn.title = '';
        if (btn.getAttribute('aria-label')) btn.setAttribute('aria-label', t('reg.action.delete'));
      }
    }, 5000);
    return;
  }
  // Secondo click: esecuzione
  btn.dataset.armed = '';
  btn.classList.remove('is-armed');
  const i = btn.querySelector('i');
  if (i) {
    i.classList.remove('bi-exclamation-triangle');
    i.classList.add('bi-trash3');
  }
  btn.title = '';
  action();
}

async function deleteTx(id) {
  const e = findEntry(id);
  try {
    await api(`/transactions/${encodeURIComponent(id)}`, {method: 'DELETE'});
    await refreshAll();
    toast(t('toast.tx_deleted', {desc: e?.desc || ''}));
  } catch (err) { fail(err, 'eliminazione movimento'); }
}

/* ---------- azioni di gruppo ---------- */
async function bulkDelete() {
  const ids = [...S.selected];
  if (!ids.length) return;
  try {
    const res = await api('/transactions/bulk', {method: 'POST', body: JSON.stringify({ids, action: 'delete'})});
    S.selected.clear();
    await refreshAll();
    toast(t('toast.deleted', {count: res?.affected ?? ids.length}));
  } catch (err) { fail(err, 'eliminazione multipla'); }
}

async function bulkCategory(catId) {
  const ids = [...S.selected];
  if (!ids.length || !catId) return;
  try {
    const res = await api('/transactions/bulk', {method: 'POST', body: JSON.stringify({ids, action: 'recategorize', category_id: catId})});
    S.selected.clear();
    await refreshAll();
    toast(t('toast.uncat', {count: res?.affected ?? ids.length}));
  } catch (err) { fail(err, 'ricategorizzazione'); }
}

/* ---------- calendari delle festività ---------- */
async function loadHolidayCalendars() {
  try { S.holCals = await api('/holiday-calendars'); } catch (err) { S.holCals = []; console.warn('calendari non disponibili', err); }
  const sel = document.getElementById('setHolidays');
  if (sel) {
    sel.innerHTML = S.holCals.map(c => `<option value="${esc(c.code)}">${esc(c.name)}</option>`).join('');
    sel.value = S.holCals.some(c => c.code === S.holidayCalendar) ? S.holidayCalendar : '';
  }
}

/* Editor amministrativo: S.hol = {code, rules[]} è la copia in modifica delle regole. */
const HOL_KINDS = ['fixed', 'easter', 'nth_weekday', 'date'];
const HOL_NEW = {
  fixed: {kind: 'fixed', month: 1, day: 1, observance: 'none'},
  easter: {kind: 'easter', offset_days: 1},
  nth_weekday: {kind: 'nth_weekday', month: 1, weekday: 1, nth: 1},
  date: {kind: 'date', date: TODAY},
};

async function openHolidayAdmin(code) {
  await loadHolidayCalendars();
  const sel = document.getElementById('holCal');
  const want = code || S.hol?.code || S.holCals[0]?.code;
  sel.innerHTML = S.holCals.map(c => `<option value="${esc(c.code)}">${esc(c.name)} (${esc(c.code)})</option>`).join('');
  if (!S.holCals.some(c => c.code === want)) { S.hol = null; renderHolidayRules(); return; }
  sel.value = want;
  document.getElementById('holName').value = S.holCals.find(c => c.code === want).name;
  document.getElementById('holYear').value = document.getElementById('holYear').value || new Date().getFullYear();
  try {
    const rules = await api(`/holiday-calendars/${encodeURIComponent(want)}/rules`);
    S.hol = {code: want, rules};
  } catch (err) { fail(err, 'regole calendario'); S.hol = {code: want, rules: []}; }
  document.getElementById('holPreview').innerHTML = '';
  renderHolidayRules();
}

function renderHolidayRules() {
  const box = document.getElementById('holRules');
  if (!box) return;
  if (!S.hol) { box.innerHTML = ''; return; }
  if (!S.hol.rules.length) { box.innerHTML = `<p style="color:var(--text-muted)">${t('hol.empty')}</p>`; return; }
  const months = MONTHS();
  const monthSel = (v, k) => `<select class="form-select form-select-sm" data-f="${k}" style="width:auto">${months.map((m, i) => `<option value="${i + 1}" ${v === i + 1 ? 'selected' : ''}>${m}</option>`).join('')}</select>`;
  const wdSel = v => `<select class="form-select form-select-sm" data-f="weekday" style="width:auto">${[1, 2, 3, 4, 5, 6, 7].map(i => `<option value="${i}" ${v === i ? 'selected' : ''}>${DOW()[i % 7]}</option>`).join('')}</select>`;
  const nthSel = v => `<select class="form-select form-select-sm" data-f="nth" style="width:auto">${[1, 2, 3, 4, 5, -1].map(n => `<option value="${n}" ${v === n ? 'selected' : ''}>${n === -1 ? t('hol.nth.last') : t('hol.nth_n', {n})}</option>`).join('')}</select>`;
  box.innerHTML = S.hol.rules.map((r, i) => {
    let params = '';
    if (r.kind === 'fixed') {
      params = `<input type="number" class="form-control form-control-sm" data-f="day" min="1" max="31" value="${r.day ?? 1}" style="width:70px">${monthSel(r.month, 'month')}
        <select class="form-select form-select-sm" data-f="observance" style="width:auto">${['none', 'nearest', 'next_free'].map(o => `<option value="${o}" ${(r.observance || 'none') === o ? 'selected' : ''}>${t(`hol.obs.${o}`)}</option>`).join('')}</select>`;
    } else if (r.kind === 'easter') {
      params = `<input type="number" class="form-control form-control-sm" data-f="offset_days" value="${r.offset_days ?? 0}" style="width:80px"><span style="font-size:.8rem;color:var(--text-muted)">${t('hol.easter_offset')} · ${t('hol.easter_hint')}</span>`;
    } else if (r.kind === 'nth_weekday') {
      params = `${nthSel(r.nth)}${wdSel(r.weekday)}${monthSel(r.month, 'month')}`;
    } else {
      params = `<input type="date" class="form-control form-control-sm" data-f="date" value="${esc(r.date || '')}" style="width:auto">`;
    }
    return `<div class="d-flex align-items-center gap-2 flex-wrap mb-2" data-hrule="${i}" style="border-bottom:1px solid var(--border);padding-bottom:.5rem">
      <input type="text" class="form-control form-control-sm" data-f="label" value="${esc(r.label)}" placeholder="${esc(t('hol.label_ph'))}" style="width:210px" maxlength="100">
      <select class="form-select form-select-sm" data-f="kind" style="width:auto">${HOL_KINDS.map(k => `<option value="${k}" ${r.kind === k ? 'selected' : ''}>${t(`hol.kind.${k}`)}</option>`).join('')}</select>
      ${params}
      <span class="ms-auto d-flex gap-1">
        <button class="rowbtn" data-hact="up" title="${esc(t('hol.move_up'))}" ${i === 0 ? 'disabled' : ''}><i class="bi bi-arrow-up"></i></button>
        <button class="rowbtn" data-hact="down" title="${esc(t('hol.move_down'))}" ${i === S.hol.rules.length - 1 ? 'disabled' : ''}><i class="bi bi-arrow-down"></i></button>
        <button class="rowbtn del" data-hact="del" aria-label="${esc(t('hol.delete'))}"><i class="bi bi-trash3"></i></button>
      </span></div>`;
  }).join('');
}

function holEditRule(i, field, raw) {
  const r = S.hol.rules[i];
  if (!r) return;
  if (field === 'kind') {
    S.hol.rules[i] = {label: r.label, ...HOL_NEW[raw]};
    renderHolidayRules();
    return;
  }
  const numeric = ['month', 'day', 'offset_days', 'weekday', 'nth'];
  r[field] = numeric.includes(field) ? Number(raw) : raw;
}

async function holSave() {
  if (!S.hol) return;
  try {
    await api(`/admin/holiday-calendars/${encodeURIComponent(S.hol.code)}/rules`, {method: 'PUT', body: JSON.stringify({rules: S.hol.rules})});
    toast(t('hol.saved', {name: S.holCals.find(c => c.code === S.hol.code)?.name || S.hol.code}));
    await refreshAll();
  } catch (err) { fail(err, 'salvataggio calendario'); }
}

async function holPreview() {
  if (!S.hol) return;
  const year = Number(document.getElementById('holYear').value) || new Date().getFullYear();
  try {
    const list = await api('/admin/holiday-calendars/preview', {method: 'POST', body: JSON.stringify({rules: S.hol.rules, year})});
    document.getElementById('holPreview').innerHTML = list.length
      ? list.map(h => `<div>${fmtDate(h.date)} <span style="color:var(--text-muted)">${dowOf(h.date)}</span> · ${esc(h.label)}</div>`).join('')
      : t('hol.no_preview');
  } catch (err) { fail(err, 'anteprima festività'); }
}

async function holCreate() {
  const code = (window.prompt(t('hol.new_prompt_code')) || '').trim();
  if (!code) return;
  const name = (window.prompt(t('hol.new_prompt_name')) || '').trim();
  if (!name) return;
  try {
    await api('/admin/holiday-calendars', {method: 'POST', body: JSON.stringify({code, name})});
    toast(t('hol.created', {name}));
    await openHolidayAdmin(code);
  } catch (err) { fail(err, 'creazione calendario'); }
}

async function holRename() {
  if (!S.hol) return;
  const name = document.getElementById('holName').value.trim();
  try {
    await api(`/admin/holiday-calendars/${encodeURIComponent(S.hol.code)}`, {method: 'PUT', body: JSON.stringify({name})});
    toast(t('hol.renamed'));
    await openHolidayAdmin(S.hol.code);
  } catch (err) { fail(err, 'rinomina calendario'); }
}

async function holDelete() {
  if (!S.hol) return;
  const btn = document.getElementById('holDelete');
  if (btn.dataset.armed !== '1') {
    btn.dataset.armed = '1';
    btn.innerHTML = `<i class="bi bi-exclamation-triangle"></i> ${t('hol.confirm_delete')}`;
    setTimeout(() => { btn.dataset.armed = ''; btn.innerHTML = `<i class="bi bi-trash3"></i> <span>${t('hol.delete')}</span>`; }, 5000);
    return;
  }
  btn.dataset.armed = '';
  btn.innerHTML = `<i class="bi bi-trash3"></i> <span>${t('hol.delete')}</span>`;
  try {
    await api(`/admin/holiday-calendars/${encodeURIComponent(S.hol.code)}`, {method: 'DELETE'});
    toast(t('hol.deleted'));
    S.hol = null;
    await openHolidayAdmin();
  } catch (err) { fail(err, 'eliminazione calendario'); }
}

/* ---------- tag ---------- */
function renderTagFilter() {
  const sel = document.getElementById('fTag');
  if (!sel) return;
  sel.innerHTML = `<option value="">${t('tag.all')}</option>` + S.tags.map(tg => `<option value="${esc(tg.id)}">${esc(tg.label)}</option>`).join('');
  sel.value = S.tags.some(tg => tg.id === S.filters.tag) ? S.filters.tag : '';
}

function renderTagList() {
  const box = document.getElementById('tagList');
  if (!box) return;
  if (!S.tags.length) { box.innerHTML = `<p style="color:var(--text-muted);margin:0">${t('tag.empty_list')}</p>`; return; }
  box.innerHTML = S.tags.map(tg => `
    <div class="cat-row">
      <span class="ce">${tagPill({...tg, label: '\u00a0'}).replace('margin-left:.3rem', '')}</span>
      <span class="cn">${esc(tg.label)}
        <div class="cb">${t('tag.used', {n: tg.uses})}${tg.description ? ' · ' + esc(tg.description) : ''}</div></span>
      <span class="racts">
        ${tg.editable === false ? '' : `<button class="rowbtn" data-tag-edit="${esc(tg.id)}" aria-label="${esc(t('reg.action.edit'))} ${esc(tg.label)}"><i class="bi bi-pencil"></i></button>`}
      </span></div>`).join('');
}

let tagEditing = null;
function openTagModal(id = null) {
  tagEditing = id;
  const tg = id ? tagOf(id) : null;
  document.getElementById('tagModalTitle').textContent = tg ? t('tag.edit') : t('tag.new');
  document.getElementById('tgLabel').value = tg ? tg.label : '';
  document.getElementById('tgDesc').value = tg ? tg.description : '';
  document.getElementById('tgColor').value = tg && tg.color ? tg.color : '#6366f1';
  const del = document.getElementById('tgDelete');
  del.style.display = tg ? '' : 'none';
  del.dataset.armed = '';
  del.innerHTML = `<i class="bi bi-trash3"></i> ${t('tag.delete')}`;
  openModal('tagModal');
}

async function saveTag() {
  const label = document.getElementById('tgLabel').value.trim();
  if (!label) { toast(t('tag.label_required')); document.getElementById('tgLabel').focus(); return; }
  const body = {label, description: document.getElementById('tgDesc').value.trim(), color: document.getElementById('tgColor').value};
  const btn = document.getElementById('tgSave');
  if (btn) btn.disabled = true;
  try {
    if (tagEditing) await api(`/tags/${encodeURIComponent(tagEditing)}`, {method: 'PUT', body: JSON.stringify(body)});
    else await api('/tags', {method: 'POST', body: JSON.stringify(body)});
    closeModal('tagModal');
    await loadTags();
    renderTagList(); renderTagFilter(); paintRegister();
    toast(t('tag.saved', {name: label}));
  } catch (err) { fail(err, 'salvataggio tag'); }
  finally { if (btn) btn.disabled = false; }
}

async function deleteTag() {
  const tg = tagOf(tagEditing);
  if (!tg) return;
  const btn = document.getElementById('tgDelete');
  if (btn.dataset.armed !== '1') {
    btn.dataset.armed = '1';
    btn.innerHTML = `<i class="bi bi-exclamation-triangle"></i> ${t('tag.confirm_delete', {n: tg.uses})}`;
    setTimeout(() => { btn.dataset.armed = ''; btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('tag.delete')}`; }, 5000);
    return;
  }
  try {
    await api(`/tags/${encodeURIComponent(tg.id)}`, {method: 'DELETE'});
    closeModal('tagModal');
    if (S.filters.tag === tg.id) S.filters.tag = '';
    await loadTags();
    renderTagList(); renderTagFilter(); paintRegister();
    toast(t('tag.deleted', {name: tg.label}));
  } catch (err) { fail(err, 'eliminazione tag'); }
}

/* Selettore dei tag nella scheda del movimento: chip attivabili, più tag per movimento. */
function renderTagChips() {
  const box = document.getElementById('tagChips');
  if (!box) return;
  box.innerHTML = S.tags.length
    ? S.tags.map(tg => `<button type="button" class="chip ${S.entry.tags.has(tg.id) ? 'is-on' : ''}" data-tag="${esc(tg.id)}" title="${esc(tg.description || '')}">${esc(tg.label)}</button>`).join('')
    : `<span style="font-size:.82rem;color:var(--text-muted)">${t('tag.none')}</span>`;
}

/* Imposta i tag di un movimento solo se sono cambiati rispetto a quelli già registrati. */
async function saveEntryTags(txId, tagIds) {
  const cur = new Set(S.tagMap[txId] || []);
  if (cur.size === tagIds.size && [...tagIds].every(x => cur.has(x))) return;
  await api(`/transactions/${encodeURIComponent(txId)}/tags`, {method: 'PUT', body: JSON.stringify({tag_ids: [...tagIds]})});
}

async function bulkTags(add, remove) {
  const ids = [...S.selected];
  if (!ids.length) return;
  try {
    await api('/transactions/bulk-tags', {method: 'POST', body: JSON.stringify({ids, add, remove})});
    await loadTagAssignments();
    await loadTags();
    paintRegister();
    toast(t('toast.tags_updated', {n: ids.length}));
  } catch (err) { fail(err, 'tag multipli'); }
}

/* ---------- filtri ---------- */
function clearAllFilters(silent = false) {
  S.search = '';
  S.filters = {cat: '', tag: '', tipo: '', from: '', to: '', min: '', max: ''};
  const si = document.getElementById('searchInput');
  if (si) si.value = '';
  document.getElementById('searchWrap')?.classList.remove('has-val');
  ['fCat', 'fTag', 'fTipo', 'fFrom', 'fTo', 'fMin', 'fMax'].forEach(id => {
    const el = document.getElementById(id);
    if (el) el.value = '';
  });
  document.getElementById('filterBtn')?.classList.remove('is-on');
  if (!silent) refreshRegister();
}

function syncFilters() {
  S.filters = {
    cat: document.getElementById('fCat')?.value || '',
    tag: document.getElementById('fTag')?.value || '',
    tipo: document.getElementById('fTipo')?.value || '',
    from: document.getElementById('fFrom')?.value || '',
    to: document.getElementById('fTo')?.value || '',
    min: document.getElementById('fMin')?.value || '',
    max: document.getElementById('fMax')?.value || '',
  };
  const any = Object.values(S.filters).some(v => v !== '');
  document.getElementById('filterBtn')?.classList.toggle('is-on', any);
  refreshRegister();
}

/* ============================================================
   STATISTICHE
   Una sola lettura dei movimenti per conto e periodo (con cache); conto,
   categorie, tag, tipo, importo e testo filtrano poi lato client, così ogni
   cambio di filtro ridisegna subito i grafici senza altre richieste.
   ============================================================ */
const SCH = {};                       // grafici Chart.js vivi, per id canvas
const STC = {cache: new Map(), cur: null, prev: null, seq: 0};
const ST_PALETTE = ['#4e79a7', '#f28e2b', '#e15759', '#76b7b2', '#59a14f', '#edc948', '#b07aa1', '#ff9da7', '#9c755f', '#bab0ac'];
const stColor = i => (i < ST_PALETTE.length ? ST_PALETTE[i] : `hsl(${Math.round((i * 137.5) % 360)} 55% 52%)`);
const stInvalidate = () => STC.cache.clear();

/* Mesi selezionati → intervallo di date; se è scelta una sola carta vale il ciclo di chiusura. */
function statsRange() {
  const st = S.stats;
  const {first, last} = boundsForKeys(st.k1, st.k2, cardOf(st.acct));
  const from = st.from || first, to = st.to || last;
  return from <= to ? {from, to} : {from: to, to: from};
}

function prevRange(r) {
  const giorni = Math.round((fromISO(r.to) - fromISO(r.from)) / 86400000) + 1;
  const pb = addDays(r.from, -1);
  return {from: addDays(pb, -(giorni - 1)), to: pb};
}

const statsDays = r => Math.round((fromISO(r.to) - fromISO(r.from)) / 86400000) + 1;

function statsTargets() {
  return S.stats.acct === 'all'
    ? visibleAccounts().filter(a => a.kind !== 'virtual').map(a => a.id)
    : [S.stats.acct];
}

function statsRow(row, accountId) {
  return {
    id: row.id, acct: row.account_id || accountId, date: row.date, cents: row.cents,
    desc: txDesc(row.description) || catName(row.category_id), cat: row.category_id || '',
    pair: row.pair_id || null, recId: row.rec_id || null,
    settle: row.description === 'card settlement' && row.cents > 0, card: false,
    forecast: row.date > TODAY, override: false,
  };
}

/* Movimenti di tutti i conti scelti nell'intervallo, previsioni incluse.
   `GET /transactions` vuole un conto per volta: li unisco qui, come il registro. */
async function fetchStatsScope(r) {
  const targets = statsTargets();
  const vacct = [...S.virtualOn].join(',');
  const key = `${targets.join(',')}|${r.from}|${r.to}|${vacct}`;
  if (STC.cache.has(key)) return STC.cache.get(key);
  const parts = await Promise.all(targets.map(async id => {
    const p = new URLSearchParams({from: r.from, to: r.to, account: id});
    if (vacct) p.set('vacct_included', vacct);
    return {id, data: await api(`/transactions?${p}`)};
  }));
  const opening = {};
  const rows = [];
  for (const {id, data} of parts) {
    opening[id] = data.summary.opening_cents;
    for (const day of data.days) for (const row of day.rows) rows.push(statsRow(row, id));
  }
  if (targets.length && r.to > TODAY) {
    const fc = await loadForecast(targets, addDays(TODAY, 1), r.to, true);
    const seen = new Set(rows.filter(e => e.recId).map(e => `${e.recId}|${e.date}|${e.acct}`));
    for (const f of fc) {
      if (seen.has(`${f.recId}|${f.date}|${f.acct}`)) continue;
      /* Come nel registro: ciò che è previsto prima di `from` finisce nell'apertura. */
      if (f.date < r.from) { if (!f.override) opening[f.acct] = (opening[f.acct] || 0) + f.cents; continue; }
      rows.push({id: null, acct: f.acct, date: f.date, cents: f.cents, desc: f.desc, cat: f.cat || '', pair: f.pair || null,
        recId: f.recId, settle: !!f.settle, card: !!f.cardForecast, forecast: true, override: !!f.override});
    }
  }
  const out = {range: r, targets, opening, rows};
  STC.cache.set(key, out);
  return out;
}

/* ---------- filtri ---------- */
const statsFiltersActive = () => {
  const s = S.stats;
  return s.cats.size || s.tags.size || s.tipo || s.min !== '' || s.max !== '' || s.search.trim() || s.from || s.to;
};

/* Movimenti che contano per entrate/uscite: niente chiusure di carta (calcolate) né
   addebiti carta previsti, e niente giri di conto salvo richiesta esplicita. */
function statsBaseRows(rows) {
  const st = S.stats;
  return rows.filter(e => !e.settle && !e.card && (st.transfers || !e.pair) && (st.forecast || e.date <= TODAY));
}

function statsMatch() {
  const st = S.stats;
  const q = st.search.trim().toLowerCase();
  const min = st.min !== '' ? parseAmount(st.min) : null, max = st.max !== '' ? parseAmount(st.max) : null;
  return e => {
    if (st.cats.size && !st.cats.has(e.cat)) return false;
    if (st.tags.size && !(S.tagMap[e.id] || []).some(id => st.tags.has(id))) return false;
    if (st.tipo === 'entrata' && e.cents <= 0) return false;
    if (st.tipo === 'uscita' && e.cents >= 0) return false;
    const abs = Math.abs(e.cents);
    if (min !== null && abs < min) return false;
    if (max !== null && abs > max) return false;
    if (q && !((e.desc || '').toLowerCase().includes(q) || catName(e.cat).toLowerCase().includes(q))) return false;
    return true;
  };
}

/* ---------- intervalli temporali ---------- */
const weekStart = date => addDays(date, -((fromISO(date).getDay() + 6) % 7));

function statsBuckets(r) {
  let g = S.stats.gran;
  if (g === 'auto') { const n = statsDays(r); g = n <= 45 ? 'day' : n <= 200 ? 'week' : 'month'; }
  const keyOf = date => (g === 'day' ? date : g === 'month' ? date.slice(0, 7) : weekStart(date));
  const keys = [];
  const idx = new Map();
  for (let d = r.from, guard = 0; d <= r.to && guard++ < 4000; d = addDays(d, 1)) {
    const k = keyOf(d);
    if (!idx.has(k)) { idx.set(k, keys.length); keys.push(k); }
  }
  const short = k => (g === 'month' ? `${MONTHS()[Number(k.slice(5, 7)) - 1].slice(0, 3)} ${k.slice(2, 4)}` : fmtDate(k).slice(0, 5));
  const full = k => (g === 'day' ? fmtDate(k) : g === 'week' ? t('st.week_of', {d: fmtDate(k)})
    : `${MONTHS()[Number(k.slice(5, 7)) - 1]} ${k.slice(0, 4)}`);
  return {g, keys, idx, keyOf, short, full};
}

/* ---------- aggregazioni ---------- */
const stKindOk = (e, kind) => (kind === 'in' ? e.cents > 0 : e.cents < 0);

function statsGroups(rows, by, kind) {
  const m = new Map();
  const add = (id, label, color, e) => {
    const g = m.get(id) || {id, label, color, cents: 0, n: 0};
    g.cents += Math.abs(e.cents);
    g.n += 1;
    m.set(id, g);
  };
  for (const e of rows) {
    if (!stKindOk(e, kind)) continue;
    if (by === 'cat') {
      const c = catById(e.cat);
      add(e.cat, `${c.emoji || '✳️'} ${c.name}`, stColor(Math.max(0, S.categories.findIndex(x => x.id === e.cat))), e);
    } else if (by === 'acct') {
      const a = acct(e.acct);
      add(e.acct, `${a ? emojiOf(a) : '🏦'} ${a?.name || '—'}`, stColor(Math.max(0, S.accounts.findIndex(x => x.id === e.acct))), e);
    } else {
      const ids = S.tagMap[e.id] || [];
      if (!ids.length) add('', t('st.no_tag'), '#9aa5b8', e);
      for (const id of ids) {
        const tg = tagOf(id);
        if (tg) add(id, tg.label, tg.color || stColor(Math.max(0, S.tags.findIndex(x => x.id === id))), e);
      }
    }
  }
  return [...m.values()].sort((a, b) => b.cents - a.cents);
}

const stFmtTick = v => (S.privacy ? '' : '€' + v.toLocaleString(LANG.intl, {maximumFractionDigits: 0}));
const stFmtVal = v => money(Math.round(v * 100));

function stMount(id, config) {
  SCH[id]?.destroy();
  SCH[id] = null;
  const cv = document.getElementById(id);
  if (!cv || typeof Chart === 'undefined') return;
  SCH[id] = new Chart(cv, config);
}

function stBase(extra = {}) {
  const grid = cssVar('--border');
  return {
    responsive: true, maintainAspectRatio: false, animation: {duration: 250},
    interaction: {mode: 'index', intersect: false},
    plugins: {legend: {display: false}, tooltip: {callbacks: {label: c => ` ${c.dataset.label ? c.dataset.label + ': ' : ''}${stFmtVal(c.parsed.y ?? c.raw)}`}}},
    scales: {x: {grid: {display: false}, ticks: {maxTicksLimit: 12, maxRotation: 0}}, y: {grid: {color: grid}, ticks: {callback: stFmtTick}}},
    ...extra,
  };
}

const stEmpty = (id, show) => {
  const cv = document.getElementById(id);
  if (!cv) return;
  let n = cv.parentElement.querySelector('.st-empty');
  if (show && !n) { n = document.createElement('div'); n.className = 'st-empty'; n.style.cssText = 'position:absolute;inset:0;display:flex;align-items:center;justify-content:center'; n.textContent = t('st.no_data'); cv.parentElement.appendChild(n); }
  else if (!show && n) n.remove();
};

/* ---------- barra degli strumenti ---------- */
function statsLabel() {
  const st = S.stats;
  if (st.from || st.to) { const r = statsRange(); return `${fmtDate(r.from)} – ${fmtDate(r.to)}`; }
  const nm = k => { const m = MONTHS()[Number(k.slice(5, 7)) - 1] || ''; return isMobile() ? m.slice(0, 3) : m; };
  if (st.k1 === st.k2) return `${nm(st.k1)} ${st.k1.slice(0, 4)}`;
  return st.k1.slice(0, 4) === st.k2.slice(0, 4)
    ? `${nm(st.k1)} – ${nm(st.k2)} ${st.k2.slice(0, 4)}`
    : `${nm(st.k1)} ${st.k1.slice(2, 4)} – ${nm(st.k2)} ${st.k2.slice(2, 4)}`;
}

function setStatsMonths(k1, k2) {
  if (k1 > k2) [k1, k2] = [k2, k1];
  Object.assign(S.stats, {k1, k2, from: '', to: ''});
  return renderStats();
}

const SPP = {open: false, year: Number(TODAY.slice(0, 4)), pick: null};

function openStatsPop(open) {
  SPP.open = open;
  SPP.pick = null;
  const pop = document.getElementById('stPeriodPop');
  if (!pop) return;
  pop.hidden = !open;
  document.getElementById('stMonthLabel')?.setAttribute('aria-expanded', String(open));
  if (open) { SPP.year = Number(S.stats.k2.slice(0, 4)); renderStatsPop(); }
}

function paintStatsPop(hover) {
  const pop = document.getElementById('stPeriodPop');
  if (!pop) return;
  let [a, b] = [S.stats.k1, S.stats.k2];
  if (SPP.pick) { const h = hover || SPP.pick; [a, b] = SPP.pick <= h ? [SPP.pick, h] : [h, SPP.pick]; }
  pop.querySelectorAll('.pp-m').forEach(btn => {
    const k = btn.dataset.k;
    btn.classList.toggle('in', k >= a && k <= b);
    btn.classList.toggle('edge', k === a || k === b);
  });
}

function renderStatsPop() {
  const pop = document.getElementById('stPeriodPop');
  if (!pop || !SPP.open) return;
  const cur = TODAY.slice(0, 7);
  const grid = MONTHS().map((n, i) => {
    const k = `${SPP.year}-${String(i + 1).padStart(2, '0')}`;
    return `<button type="button" class="pp-m ${k === cur ? 'today' : ''}" data-k="${k}">${esc(n.slice(0, 3))}</button>`;
  }).join('');
  const presets = [
    ['per.cur', 0, 0], ['per.last', -1, -1], ['per.prev_cur_next', -1, 1], ['per.cur_prev2', -2, 0],
    ['per.last6', -5, 0], ['per.last12', -11, 0], ['per.year', 'year'],
  ].map(([key, a, b]) => `<button type="button" data-spreset="${a}" data-b="${b ?? ''}">${esc(t(key))}</button>`).join('');
  pop.innerHTML = `
    <div class="pp-year">
      <button type="button" data-syear="-1" aria-label="${esc(t('per.prev_year'))}"><i class="bi bi-chevron-left"></i></button>
      <span>${SPP.year}</span>
      <button type="button" data-syear="1" aria-label="${esc(t('per.next_year'))}"><i class="bi bi-chevron-right"></i></button>
    </div>
    <div class="pp-grid">${grid}</div>
    <div class="pp-hint">${esc(t('per.hint'))}</div>
    <div class="pp-sep"></div>
    <div class="pp-presets">${presets}</div>`;
  paintStatsPop();
}

/* Elenco a scelta multipla (categorie, tag): un pulsante che apre l'elenco di caselle. */
function renderStatsMulti(id, items, selected, allLabel) {
  const box = document.getElementById(id);
  if (!box) return;
  const old = box.querySelector('.ms-pop');
  const open = !!old && !old.hidden;
  const scroll = old ? old.scrollTop : 0;
  const valid = [...selected].filter(v => items.some(i => i.id === v));
  if (valid.length !== selected.size) { selected.clear(); valid.forEach(v => selected.add(v)); }
  const label = !selected.size ? allLabel
    : selected.size === 1 ? items.find(i => i.id === [...selected][0])?.label || allLabel
      : t('st.ms.n', {n: selected.size});
  box.innerHTML = `<button type="button" class="ms-btn form-select ${selected.size ? 'has-val' : ''}" aria-haspopup="listbox" aria-expanded="${open}">${esc(label)}</button>
    <div class="ms-pop" role="listbox" ${open ? '' : 'hidden'}>
      ${selected.size ? `<button type="button" class="ms-clr" data-msclr="1">${esc(t('st.ms.clear'))}</button>` : ''}
      ${items.map(i => `<label><input type="checkbox" value="${esc(i.id)}" ${selected.has(i.id) ? 'checked' : ''}> ${esc(i.label)}</label>`).join('')}
    </div>`;
  const pop = box.querySelector('.ms-pop');
  if (pop) pop.scrollTop = scroll;
}

function renderStatsFilterControls() {
  const st = S.stats;
  renderStatsMulti('stCatMs', S.categories.map(c => ({id: c.id, label: `${esc(c.emoji)} ${c.name}`})), st.cats, t('common.all'));
  renderStatsMulti('stTagMs', S.tags.map(tg => ({id: tg.id, label: tg.label})), st.tags, t('tag.all'));
}

function syncStatsToolbar(r) {
  const st = S.stats;
  const sel = document.getElementById('statsAccount');
  if (sel) {
    sel.innerHTML = `<option value="all">${t('reg.all_accounts')}</option>` + visibleAccounts().map(a => accountOption(a)).join('');
    if (st.acct !== 'all' && !visibleAccounts().some(a => a.id === st.acct)) st.acct = 'all';
    sel.value = st.acct;
  }
  const lbl = document.getElementById('stMonthLabel');
  if (lbl) lbl.textContent = statsLabel();
  if (SPP.open) renderStatsPop();
  const f = document.getElementById('stFrom'), tt = document.getElementById('stTo');
  if (f) f.value = r.from;
  if (tt) tt.value = r.to;
  document.getElementById('stFrom')?._dateRefresh?.();
  document.getElementById('stTo')?._dateRefresh?.();
  const set = (id, v) => { const el = document.getElementById(id); if (el && document.activeElement !== el) el.value = v; };
  set('stTipo', st.tipo);
  set('stGran', st.gran);
  document.getElementById('stSearchWrap')?.classList.toggle('has-val', !!st.search);
  document.getElementById('stForecastBtn')?.classList.toggle('is-on', st.forecast);
  document.getElementById('stTransfersBtn')?.classList.toggle('is-on', st.transfers);
  const anyF = st.cats.size || st.tags.size || st.tipo || st.min !== '' || st.max !== '' || st.from || st.to;
  document.getElementById('stFilterBtn')?.classList.toggle('is-on', !!anyF);
  document.querySelectorAll('#page-statistiche .st-seg').forEach(seg => {
    const cur = String(st[seg.dataset.seg]);
    seg.querySelectorAll('button').forEach(b => b.classList.toggle('is-on', b.dataset.v === cur));
  });
  const split = document.getElementById('stBalSeg');
  if (split) split.style.display = statsTargets().length > 1 ? '' : 'none';
  renderStatsFilterControls();
}

/* Filtri attivi come etichette rimovibili: si vede cosa restringe i grafici. */
function renderStatsChips() {
  const box = document.getElementById('stChips');
  if (!box) return;
  const st = S.stats;
  const chips = [];
  st.cats.forEach(id => chips.push({k: 'cat', v: id, l: `${catEmoji(id)} ${catName(id)}`}));
  st.tags.forEach(id => chips.push({k: 'tag', v: id, l: `# ${tagOf(id)?.label || ''}`}));
  if (st.tipo) chips.push({k: 'tipo', l: st.tipo === 'entrata' ? t('st.kind.in') : t('st.kind.out')});
  if (st.min !== '') chips.push({k: 'min', l: `≥ ${st.min}`});
  if (st.max !== '') chips.push({k: 'max', l: `≤ ${st.max}`});
  if (st.search.trim()) chips.push({k: 'search', l: `“${st.search.trim()}”`});
  if (st.from || st.to) chips.push({k: 'days', l: t('st.chip.days')});
  box.innerHTML = chips.map(c => `<span class="chip-f">${esc(c.l)}<button type="button" data-chip="${c.k}" data-v="${esc(c.v ?? '')}" aria-label="${esc(t('st.ms.clear'))}"><i class="bi bi-x"></i></button></span>`).join('');
}

function removeStatsChip(k, v) {
  const st = S.stats;
  if (k === 'cat') st.cats.delete(v);
  else if (k === 'tag') st.tags.delete(v);
  else if (k === 'tipo') st.tipo = '';
  else if (k === 'min') { st.min = ''; document.getElementById('stMin').value = ''; }
  else if (k === 'max') { st.max = ''; document.getElementById('stMax').value = ''; }
  else if (k === 'search') { st.search = ''; document.getElementById('stSearch').value = ''; }
  else if (k === 'days') { st.from = ''; st.to = ''; return renderStats(); }
  paintStats();
}

/* ---------- caricamento e disegno ---------- */
async function renderStats() {
  const kpis = document.getElementById('statsKpis');
  if (!kpis) return;
  const r = statsRange();
  syncStatsToolbar(r);
  if (!visibleAccounts().length) {
    kpis.innerHTML = `<div class="kpi"><div class="k-l">${t('stat.title')}</div><div class="k-v num">—</div>
      <div class="k-s">${t('empty.no_accounts')}</div></div>`;
    return;
  }
  const seq = ++STC.seq;
  kpis.setAttribute('aria-busy', 'true');
  let cur, prev;
  try {
    [cur, prev] = await Promise.all([fetchStatsScope(r), fetchStatsScope(prevRange(r))]);
  } catch (err) {
    if (seq !== STC.seq) return;
    kpis.removeAttribute('aria-busy');
    kpis.innerHTML = `<div class="kpi"><div class="k-l">${t('stat.title')}</div><div class="k-v num">—</div>
      <div class="k-s">${esc(apiErrorToMessage(err))}</div></div>`;
    fail(err, 'statistiche');
    return;
  }
  if (seq !== STC.seq) return;       // nel frattempo l'utente ha cambiato periodo o conto
  kpis.removeAttribute('aria-busy');
  STC.cur = cur;
  STC.prev = prev;
  paintStats();
}

const stSum = (rows, kind) => rows.reduce((s, e) => s + (stKindOk(e, kind) ? Math.abs(e.cents) : 0), 0);

function stDelta(cur, prev, upIsGood) {
  if (!prev) return `<span style="color:var(--text-muted)">${t('stat.no_compare')}</span>`;
  const d = Math.round((cur - prev) / prev * 100);
  if (d === 0) return t('stat.in_line');
  const good = upIsGood ? d > 0 : d < 0;
  return `<span class="${good ? 'delta-down' : 'delta-up'}">${d > 0 ? '+' : ''}${d}%</span> ${t('stat.on_prev')}`;
}

function paintStats() {
  const cur = STC.cur, prev = STC.prev;
  if (!cur || !document.getElementById('statsKpis')) return;
  const r = cur.range, pr = prev.range;
  const match = statsMatch();
  const rows = statsBaseRows(cur.rows).filter(match);
  const prows = statsBaseRows(prev.rows).filter(match);
  const bk = statsBuckets(r);
  syncStatsToolbar(r);
  renderStatsChips();

  const ent = stSum(rows, 'in'), usc = stSum(rows, 'out');
  const pEnt = stSum(prows, 'in'), pUsc = stSum(prows, 'out');
  const net = ent - usc, pNet = pEnt - pUsc;
  const endEff = S.stats.forecast ? r.to : (r.to < TODAY ? r.to : TODAY);
  const giorni = Math.max(1, statsDays({from: r.from, to: endEff < r.from ? r.from : endEff}));
  const bal = statsBalance(cur, bk);
  const closing = bal.total.length ? bal.total[bal.total.length - 1].v : null;
  const opening = cur.targets.reduce((s, id) => s + (cur.opening[id] || 0), 0);

  const bar = document.getElementById('stResultBar');
  if (bar) {
    bar.innerHTML = `<span><i class="bi bi-calendar3"></i> ${fmtDate(r.from)} → ${fmtDate(r.to)}</span>
      <span>${t('kpi.income')} <b class="pos">${money(ent)}</b></span>
      <span>${t('kpi.expense')} <b class="neg">${money(usc)}</b></span>
      <span>${t('kpi.net')} <b class="${net < 0 ? 'neg' : 'pos'}">${money(net, true)}</b></span>
      <span style="margin-left:auto;font-size:.8rem;color:var(--text-muted)">${t('stat.compare_with', {from: fmtDate(pr.from), to: fmtDate(pr.to)})}</span>`;
  }

  const kpis = document.getElementById('statsKpis');
  const rate = ent > 0 ? Math.round(net / ent * 100) : null;
  kpis.innerHTML = `
    <div class="kpi"><div class="k-l">${t('kpi.income')}</div><div class="k-v num pos">${money(ent)}</div>
      <div class="k-s">${stDelta(ent, pEnt, true)}</div></div>
    <div class="kpi"><div class="k-l">${t('kpi.expense')}</div><div class="k-v num neg">${money(usc)}</div>
      <div class="k-s">${stDelta(usc, pUsc, false)}</div></div>
    <div class="kpi"><div class="k-l">${t('stat.period_balance')}</div>
      <div class="k-v num ${net < 0 ? 'neg' : 'pos'}">${money(net, true)}</div>
      <div class="k-s">${t('stat.prev_period')} ${money(pNet, true)}</div></div>
    <div class="kpi"><div class="k-l">${t('st.kpi.savings')}</div>
      <div class="k-v num ${rate !== null && rate < 0 ? 'neg' : ''}">${rate === null ? '—' : rate + '%'}</div>
      <div class="k-s">${t('st.kpi.savings_hint')}</div></div>
    <div class="kpi"><div class="k-l">${t('stat.avg_per_day')}</div>
      <div class="k-v num">${money(Math.round(usc / giorni))}</div><div class="k-s">${t('stat.over_days', {n: giorni})}</div></div>
    <div class="kpi"><div class="k-l">${t('st.kpi.closing')}</div>
      <div class="k-v num ${closing !== null && closing < 0 ? 'neg' : ''}">${closing === null ? '—' : money(closing)}</div>
      <div class="k-s">${t('st.kpi.opening', {amount: money(opening)})}</div></div>`;

  if (typeof Chart !== 'undefined') {
    Chart.defaults.color = cssVar('--text-muted');
    Chart.defaults.font.family = "system-ui,-apple-system,'Segoe UI',sans-serif";
    paintStatsBalance(bal, bk);
    paintStatsFlow(rows, bk);
    paintStatsPie(rows);
    paintStatsTrend(rows, bk);
    paintStatsPace(rows, prows, r);
    paintStatsWeek(rows, r);
  }
  paintStatsTop(rows);
  paintStatsFreq(rows);
  paintStatsBudget(rows, prows, r);
}

/* Saldo: usa tutti i movimenti (filtri esclusi). Le chiusure carta contano: azzerano il debito
   della carta mentre l'addebito corrispondente esce dal conto collegato; senza di esse la
   spesa risulterebbe contata due volte. */
function statsBalance(cur, bk) {
  const r = cur.range;
  const rows = cur.rows.filter(e => S.stats.forecast || e.date <= TODAY);
  const perAcct = new Map(cur.targets.map(id => [id, new Map()]));
  for (const e of rows) {
    const m = perAcct.get(e.acct);
    if (m) m.set(e.date, (m.get(e.date) || 0) + e.cents);
  }
  const last = S.stats.forecast ? r.to : (r.to < TODAY ? r.to : TODAY);
  const series = new Map(), total = new Map();
  for (const id of cur.targets) {
    let b = cur.opening[id] || 0;
    const pts = new Map();
    for (let d = r.from, guard = 0; d <= last && guard++ < 4000; d = addDays(d, 1)) {
      b += perAcct.get(id).get(d) || 0;
      pts.set(bk.keyOf(d), {v: b, d});          // l'ultimo giorno del bucket vince
    }
    series.set(id, pts);
    for (const [k, p] of pts) {
      const t0 = total.get(k) || {v: 0, d: p.d};
      t0.v += p.v;
      t0.d = p.d;
      total.set(k, t0);
    }
  }
  const asArr = m => bk.keys.filter(k => m.has(k)).map(k => ({k, ...m.get(k)}));
  return {total: asArr(total), series: cur.targets.map(id => ({id, pts: asArr(series.get(id))}))};
}

function paintStatsBalance(bal, bk) {
  const split = S.stats.balSplit === '1' && bal.series.length > 1;
  const labels = bk.keys, grid = cssVar('--border');
  const fcast = p => p && p.d > TODAY;
  const mk = (label, pts, color, fill) => {
    const byK = new Map(pts.map(p => [p.k, p]));
    return {
      label, data: labels.map(k => (byK.has(k) ? byK.get(k).v / 100 : null)),
      borderColor: color, backgroundColor: fill ? color + '22' : 'transparent', fill: !!fill,
      pointRadius: labels.length > 60 ? 0 : 2, pointHoverRadius: 4, tension: .2, borderWidth: 2, spanGaps: true,
      segment: {borderDash: c => (fcast(byK.get(labels[c.p1DataIndex])) ? [5, 4] : undefined)},
    };
  };
  const datasets = split
    ? bal.series.map(s => mk(`${acct(s.id) ? emojiOf(acct(s.id)) : ''} ${acct(s.id)?.name || ''}`.trim(), s.pts, stColor(Math.max(0, S.accounts.findIndex(a => a.id === s.id)))))
    : [mk(t('st.bal.total'), bal.total, cssVar('--accent'), true)];
  stEmpty('chartBal', !bal.total.length);
  stMount('chartBal', {
    type: 'line', data: {labels: labels.map(bk.short), datasets},
    options: stBase({
      plugins: {
        legend: {display: split, position: 'bottom', labels: {boxWidth: 10, usePointStyle: true}},
        tooltip: {callbacks: {title: i => bk.full(labels[i[0].dataIndex]), label: c => ` ${c.dataset.label}: ${stFmtVal(c.parsed.y)}`}},
      },
      scales: {x: {grid: {display: false}, ticks: {maxTicksLimit: 12, maxRotation: 0}}, y: {grid: {color: grid}, ticks: {callback: stFmtTick}}},
    }),
  });
}

function paintStatsFlow(rows, bk) {
  const inc = bk.keys.map(() => 0), out = bk.keys.map(() => 0);
  for (const e of rows) {
    const i = bk.idx.get(bk.keyOf(e.date));
    if (i === undefined) continue;
    if (e.cents > 0) inc[i] += e.cents; else out[i] += -e.cents;
  }
  const net = inc.map((v, i) => (v - out[i]) / 100);
  const note = document.getElementById('stFlowNote');
  if (note) note.textContent = t(`st.gran.note.${bk.g}`);
  stEmpty('chartFlow', !rows.length);
  stMount('chartFlow', {
    type: 'bar',
    data: {
      labels: bk.keys.map(bk.short),
      datasets: [
        {type: 'line', label: t('st.net'), data: net, borderColor: cssVar('--accent'), backgroundColor: cssVar('--accent'),
          borderWidth: 2, tension: .2, pointRadius: bk.keys.length > 40 ? 0 : 3, order: 0},
        {label: t('kpi.income'), data: inc.map(v => v / 100), backgroundColor: cssVar('--pos'), borderRadius: 4, order: 1},
        {label: t('kpi.expense'), data: out.map(v => v / 100), backgroundColor: cssVar('--neg'), borderRadius: 4, order: 1},
      ],
    },
    options: stBase({
      plugins: {
        legend: {display: true, position: 'bottom', labels: {boxWidth: 10, usePointStyle: true}},
        tooltip: {callbacks: {title: i => bk.full(bk.keys[i[0].dataIndex]), label: c => ` ${c.dataset.label}: ${stFmtVal(c.parsed.y)}`}},
      },
    }),
  });
}

/* Torta: ripartizione percentuale per categoria, tag o conto. Dopo le prime 9 voci il resto è «Altre». */
function paintStatsPie(rows) {
  const st = S.stats;
  const groups = statsGroups(rows, st.pieBy, st.pieKind);
  const total = groups.reduce((s, g) => s + g.cents, 0);
  const top = groups.slice(0, 9);
  const rest = groups.slice(9);
  if (rest.length) top.push({id: '__rest', label: t('st.others'), color: '#9aa5b8', cents: rest.reduce((s, g) => s + g.cents, 0), n: rest.reduce((s, g) => s + g.n, 0)});
  stEmpty('chartPie', !total);
  stMount('chartPie', {
    type: 'doughnut',
    data: {labels: top.map(g => g.label), datasets: [{data: top.map(g => g.cents / 100), backgroundColor: top.map(g => g.color), borderColor: cssVar('--surface'), borderWidth: 2}]},
    options: {
      responsive: true, maintainAspectRatio: false, cutout: '58%', animation: {duration: 250},
      plugins: {legend: {display: false}, tooltip: {callbacks: {label: c => ` ${c.label}: ${stFmtVal(c.raw)} (${total ? (c.raw * 100 / (total / 100)).toFixed(1) : 0}%)`}}},
    },
  });
  const lg = document.getElementById('stPieLegend');
  if (!lg) return;
  const sel = st.pieBy === 'cat' ? st.cats : st.pieBy === 'tag' ? st.tags : null;
  lg.innerHTML = top.length ? top.map(g => {
    const pct = total ? g.cents / total * 100 : 0;
    const clickable = sel && g.id !== '__rest' && !(st.pieBy === 'tag' && g.id === '');
    return `<button type="button" class="lg ${clickable ? '' : 'static'} ${sel?.has(g.id) ? 'is-sel' : ''}" ${clickable ? `data-pie="${esc(g.id)}"` : ''} title="${esc(g.label)} · ${g.n}">
      <i class="sw" style="background:${esc(g.color)}"></i><span class="nm">${esc(g.label)}</span>
      <span class="pc">${pct.toFixed(pct < 10 ? 1 : 0)}%</span><span class="am">${money(g.cents)}</span></button>`;
  }).join('') + (sel ? `<div class="st-note" style="padding:.4rem .35rem 0">${esc(t('st.legend_hint'))}</div>` : '')
    : `<div class="st-empty">${esc(t('st.no_data'))}</div>`;
}

/* Andamento nel tempo delle voci scelte: quelle spuntate nei filtri o, in mancanza, le prime 6. */
function paintStatsTrend(rows, bk) {
  const st = S.stats;
  const groups = statsGroups(rows, st.trendBy, st.trendKind);
  const picked = st.trendBy === 'cat' ? st.cats : st.trendBy === 'tag' ? st.tags : new Set();
  const chosen = picked.size ? groups.filter(g => picked.has(g.id)) : groups.slice(0, 6);
  const note = document.getElementById('stTrendNote');
  if (note) note.textContent = !picked.size && groups.length > 6 ? t('st.trend_top', {n: 6}) : '';
  const ids = new Set(chosen.map(g => g.id));
  const data = new Map(chosen.map(g => [g.id, bk.keys.map(() => 0)]));
  const other = bk.keys.map(() => 0);
  const addTo = (id, i, v) => { if (ids.has(id)) data.get(id)[i] += v; else other[i] += v; };
  for (const e of rows) {
    if (!stKindOk(e, st.trendKind)) continue;
    const i = bk.idx.get(bk.keyOf(e.date));
    if (i === undefined) continue;
    const v = Math.abs(e.cents);
    if (st.trendBy === 'cat') addTo(e.cat, i, v);
    else if (st.trendBy === 'acct') addTo(e.acct, i, v);
    else {
      const tg = S.tagMap[e.id] || [];
      if (!tg.length) addTo('', i, v);
      for (const id of tg) addTo(id, i, v);
    }
  }
  const stack = st.trendMode === 'stack';
  const datasets = chosen.map(g => ({
    label: g.label, data: data.get(g.id).map(v => v / 100), borderColor: g.color, backgroundColor: stack ? g.color : g.color + '33',
    borderWidth: 2, tension: .2, pointRadius: bk.keys.length > 40 ? 0 : 3, borderRadius: 3,
  }));
  if (stack && !picked.size && other.some(v => v)) {
    datasets.push({label: t('st.others'), data: other.map(v => v / 100), borderColor: '#9aa5b8', backgroundColor: '#9aa5b8', borderRadius: 3});
  }
  stEmpty('chartTrend', !datasets.length);
  stMount('chartTrend', {
    type: stack ? 'bar' : 'line',
    data: {labels: bk.keys.map(bk.short), datasets},
    options: stBase({
      plugins: {
        legend: {display: true, position: 'bottom', labels: {boxWidth: 10, usePointStyle: true}},
        tooltip: {callbacks: {title: i => bk.full(bk.keys[i[0].dataIndex]), label: c => ` ${c.dataset.label}: ${stFmtVal(c.parsed.y)}`}},
      },
      scales: {x: {stacked: stack, grid: {display: false}, ticks: {maxTicksLimit: 12, maxRotation: 0}},
        y: {stacked: stack, grid: {color: cssVar('--border')}, ticks: {callback: stFmtTick}}},
    }),
  });
}

/* Spesa (o entrata) cumulata giorno per giorno, contro il periodo precedente. */
function paintStatsPace(rows, prows, r) {
  const kind = S.stats.tipo === 'entrata' ? 'in' : 'out';
  const title = document.getElementById('stPaceTitle');
  if (title) title.textContent = t(kind === 'in' ? 'st.c.pace_in' : 'st.c.pace_out');
  const n = statsDays(r), pr = prevRange(r);
  const cum = (list, from) => {
    const a = new Array(n).fill(0);
    for (const e of list) {
      if (!stKindOk(e, kind)) continue;
      const i = Math.round((fromISO(e.date) - fromISO(from)) / 86400000);
      if (i >= 0 && i < n) a[i] += Math.abs(e.cents);
    }
    for (let i = 1; i < n; i++) a[i] += a[i - 1];
    return a.map(v => v / 100);
  };
  const c = cum(rows, r.from), p = cum(prows, pr.from);
  const todayIdx = Math.round((fromISO(TODAY) - fromISO(r.from)) / 86400000);
  const ds = (label, data, color, extra = {}) => ({label, data, borderColor: color, backgroundColor: color, borderWidth: 2, pointRadius: 0, tension: .15, ...extra});
  stEmpty('chartPace', !c[n - 1] && !p[n - 1]);
  stMount('chartPace', {
    type: 'line',
    data: {
      labels: c.map((_, i) => i + 1),
      datasets: [
        ds(t('st.this_period'), c, cssVar('--accent'), {segment: {borderDash: x => (x.p1DataIndex > todayIdx ? [5, 4] : undefined)}}),
        ds(t('st.prev_period_short'), p, cssVar('--text-muted'), {borderDash: [2, 3], borderWidth: 1.5}),
      ],
    },
    options: stBase({
      plugins: {
        legend: {display: true, position: 'bottom', labels: {boxWidth: 10, usePointStyle: true}},
        tooltip: {callbacks: {title: i => t('st.day_n', {n: i[0].label, date: fmtDate(addDays(r.from, i[0].dataIndex))}), label: x => ` ${x.dataset.label}: ${stFmtVal(x.parsed.y)}`}},
      },
    }),
  });
}

/* Media giornaliera per giorno della settimana (lunedì → domenica). */
function paintStatsWeek(rows, r) {
  const kind = S.stats.tipo === 'entrata' ? 'in' : 'out';
  const title = document.getElementById('stWeekTitle');
  if (title) title.textContent = t(kind === 'in' ? 'st.c.weekday_in' : 'st.c.weekday_out');
  const tot = new Array(7).fill(0), occ = new Array(7).fill(0);
  const last = S.stats.forecast ? r.to : (r.to < TODAY ? r.to : TODAY);
  for (let d = r.from, g = 0; d <= last && g++ < 4000; d = addDays(d, 1)) occ[fromISO(d).getDay()]++;
  for (const e of rows) if (stKindOk(e, kind)) tot[fromISO(e.date).getDay()] += Math.abs(e.cents);
  const order = [1, 2, 3, 4, 5, 6, 0];
  const avg = order.map(i => (occ[i] ? tot[i] / occ[i] / 100 : 0));
  const hi = Math.max(...avg);
  stEmpty('chartWeek', !hi);
  stMount('chartWeek', {
    type: 'bar',
    data: {labels: order.map(i => DOW()[i]), datasets: [{data: avg, borderRadius: 5,
      backgroundColor: avg.map(v => (v === hi && hi > 0 ? cssVar('--accent') : cssVar('--accent') + '88'))}]},
    options: stBase({
      plugins: {legend: {display: false}, tooltip: {callbacks: {label: c => ` ${t('st.avg_short')}: ${stFmtVal(c.raw)} · ${t('st.total_short')}: ${money(tot[order[c.dataIndex]])}`}}},
    }),
  });
}

function paintStatsTop(rows) {
  const box = document.getElementById('stTop');
  if (!box) return;
  const kind = S.stats.topKind;
  const list = rows.filter(e => stKindOk(e, kind)).sort((a, b) => Math.abs(b.cents) - Math.abs(a.cents)).slice(0, 10);
  const many = statsTargets().length > 1;
  box.innerHTML = list.length ? `<div class="st-list">${list.map(e => `
    <div class="r"><div class="d">${esc(e.desc)}</div>
      <div class="a ${e.cents < 0 ? 'neg' : 'pos'}">${money(e.cents, true)}</div>
      <div class="m">${fmtDate(e.date)} · ${esc(catEmoji(e.cat))} ${esc(catName(e.cat))}${many ? ` · ${esc(acct(e.acct)?.name || '')}` : ''}${e.forecast ? ` · ${esc(t('st.forecast_tag'))}` : ''}</div></div>`).join('')}</div>`
    : `<div class="st-empty">${esc(t('st.no_data'))}</div>`;
}

function paintStatsFreq(rows) {
  const box = document.getElementById('stFreq');
  if (!box) return;
  const kind = S.stats.tipo === 'entrata' ? 'in' : 'out';
  const m = new Map();
  for (const e of rows) {
    if (!stKindOk(e, kind)) continue;
    const key = e.desc.trim().toLowerCase();
    const g = m.get(key) || {label: e.desc.trim(), cents: 0, n: 0, cat: e.cat};
    g.cents += Math.abs(e.cents);
    g.n += 1;
    m.set(key, g);
  }
  const list = [...m.values()].sort((a, b) => b.cents - a.cents).slice(0, 10);
  box.innerHTML = list.length ? `<div class="st-list">${list.map(g => `
    <div class="r"><div class="d">${esc(g.label)}</div><div class="a ${kind === 'in' ? 'pos' : 'neg'}">${money(g.cents)}</div>
      <div class="m">${esc(t('st.n_times', {n: g.n}))} · ${esc(t('st.avg_short'))} ${money(Math.round(g.cents / g.n))}</div></div>`).join('')}</div>`
    : `<div class="st-empty">${esc(t('st.no_data'))}</div>`;
}

/* Budget: il tetto è mensile, quindi si scala sulla durata del periodo. */
function paintStatsBudget(rows, prows, r) {
  const box = document.getElementById('budgetList');
  if (!box) return;
  const months = S.stats.from || S.stats.to ? statsDays(r) / 30.4375 : monthsBetween(S.stats.k1, S.stats.k2);
  const note = document.getElementById('stBudgetNote');
  if (note) note.textContent = months > 1.05 ? t('st.budget_scaled', {n: Math.round(months * 10) / 10}) : '';
  const spent = new Map(), pspent = new Map();
  rows.forEach(e => { if (e.cents < 0) spent.set(e.cat, (spent.get(e.cat) || 0) - e.cents); });
  prows.forEach(e => { if (e.cents < 0) pspent.set(e.cat, (pspent.get(e.cat) || 0) - e.cents); });
  const cats = S.categories.filter(c => c.kind === 'out' && (c.budget_cents || spent.get(c.id) || pspent.get(c.id)))
    .sort((a, b) => (spent.get(b.id) || 0) - (spent.get(a.id) || 0));
  box.innerHTML = cats.length ? cats.map(c => {
    const s = spent.get(c.id) || 0, ps = pspent.get(c.id) || 0;
    const budget = Math.round((c.budget_cents || 0) * months);
    const pct = budget ? Math.min(100, Math.round(s / budget * 100)) : 0;
    const cls = budget && s > budget ? 'over' : pct > 85 ? 'warn' : '';
    const d = ps ? Math.round((s - ps) / ps * 100) : null;
    return `<div class="budget-row">
      <div class="b-name">${esc(c.emoji)} ${esc(c.name)}</div>
      <div class="b-val">${money(s)}${budget ? ` <span style="color:var(--text-muted)">/ ${money(budget)}</span>` : ''}</div>
      <div class="b-bar"><div class="meter"><i class="${cls}" style="width:${pct}%"></i></div></div>
      <div class="b-sub"><span>${budget ? t('stat.pct_of_budget', {pct: Math.round(s / budget * 100)}) : t('stat.no_budget')}</span>
        <span>${d === null ? '—' : `<span class="${d > 0 ? 'delta-up' : 'delta-down'}">${d > 0 ? '+' : ''}${d}%</span> ${t('stat.vs_prev')}`}</span></div>
    </div>`;
  }).join('') : `<p style="color:var(--text-muted);margin:0">${t('st.no_data')}</p>`;
}

function resetStatsFilters() {
  const st = S.stats;
  st.cats.clear(); st.tags.clear();
  Object.assign(st, {from: '', to: '', search: '', tipo: '', min: '', max: ''});
  ['stMin', 'stMax', 'stSearch'].forEach(id => { const el = document.getElementById(id); if (el) el.value = ''; });
}

/* ============================================================
   RICORRENTI
   ============================================================ */
const PATTERN_LABEL = p => {
  if (!p) return '—';
  const s = String(p).toUpperCase();
  if (s.includes('FREQ=DAILY')) return t('rec.pattern.daily');
  if (s.includes('FREQ=WEEKLY')) return t('rec.pattern.weekly');
  if (s.includes('FREQ=YEARLY')) return t('rec.pattern.yearly');
  if (s.includes('BYMONTHDAY=-1')) return t('rec.pattern.monthly_last');
  if (s.includes('BYDAY')) return t('rec.pattern.monthly_weekday');
  return t('rec.pattern.monthly_day');
};
const WEEKEND_LABEL = {create: 'rec.weekend.create', skip: 'rec.weekend.skip', friday: 'modal.rec.weekend_friday', monday: 'modal.rec.weekend_monday'};

/* La prossima occorrenza è un calcolo del backend: la chiedo per le regole
   attive e riempio le celle quando arriva, senza duplicare qui la logica
   di ricorrenza. */
/* Ricorrenze: ordinamento per colonna e raggruppamento per conto (attivo di default). */
const REC_VIEW = {key: 'desc', dir: 1, group: true, next: {}};
try { if (localStorage.getItem('pfm.recGroup') === '0') REC_VIEW.group = false; } catch { /* storage non disponibile */ }

function recSortValue(r, key) {
  switch (key) {
    case 'acct': return (acct(r.account_id)?.name || '').toLowerCase();
    case 'amount': return r.amount_cents;
    case 'pattern': return String(PATTERN_LABEL(r.pattern)).toLowerCase();
    case 'next': return REC_VIEW.next[r.id] || (r.active ? '9999-99-99' : '9999-99-99z');
    case 'weekend': return t(WEEKEND_LABEL[r.weekend] || 'rec.weekend.create').toLowerCase();
    case 'end': return r.end_type === 'never' ? '9999' : r.end_type === 'until' ? (r.end_date || '') : String(r.end_count || 0).padStart(6, '0');
    case 'active': return r.active ? 0 : 1;
    default: return (r.description || '').toLowerCase();
  }
}

function recSorted(list) {
  const {key, dir} = REC_VIEW;
  return [...list].sort((x, y) => {
    const a = recSortValue(x, key), b = recSortValue(y, key);
    const c = typeof a === 'number' && typeof b === 'number' ? a - b : String(a).localeCompare(String(b), undefined, {numeric: true});
    return c * dir || (x.description || '').localeCompare(y.description || '');
  });
}

function recRowHTML(r) {
  const a = acct(r.account_id);
  const wk = t(WEEKEND_LABEL[r.weekend] || 'rec.weekend.create');
  const end = r.end_type === 'never' ? t('rec.end.never')
    : r.end_type === 'until' ? (r.end_date ? fmtDate(r.end_date) : '—')
      : t('rec.end.count', {n: r.end_count || 0});
  return `<tr data-rec-row="${esc(r.id)}" class="${S.hlRec === r.id ? 'is-hl' : ''}" style="${r.active ? '' : 'opacity:.55'}">
      <td><div style="font-weight:600">${esc(r.description)}</div>
          ${r.note ? `<div style="font-size:.75rem;color:var(--text-muted)">${esc(r.note)}</div>` : ''}</td>
      <td><div>${esc(a?.name || '—')}</div>
          ${a?.note ? `<div style="font-size:.75rem;color:var(--text-muted)">${esc(a.note)}</div>` : ''}</td>
      <td class="r ${r.amount_cents > 0 ? 'pos' : 'neg'}" style="font-weight:600">
        ${r.mode === 'avg'
      ? `≈ ${money(r.amount_cents, true)}<div style="font-weight:500;font-size:.72rem;color:var(--text-muted)">${t('rec.avg_of', {n: r.avg_n || 6})}</div>`
      : money(r.amount_cents, true)}</td>
      <td>${PATTERN_LABEL(r.pattern)}</td>
      <td data-rec-next="${esc(r.id)}">${r.active ? (REC_VIEW.next[r.id] ? fmtDate(REC_VIEW.next[r.id]) : '…') : '—'}</td>
      <td style="font-size:.8rem;color:var(--text-muted)">${esc(wk)}</td>
      <td>${esc(end)}</td>
      <td style="text-align:center"><span class="${r.active ? 'dot-on' : 'dot-off'}" title="${esc(r.active ? t('rec.active') : t('rec.paused'))}">●</span></td>
      <td class="r" style="white-space:nowrap">
        <button class="rowbtn" data-recedit="${esc(r.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>
        <button class="rowbtn del" data-recdel="${esc(r.id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i></button>
      </td></tr>`;
}

function recGroupedHTML(list) {
  const sorted = recSorted(list);
  if (!REC_VIEW.group) return sorted.map(recRowHTML).join('');
  const groups = new Map();
  for (const r of sorted) {
    if (!groups.has(r.account_id)) groups.set(r.account_id, []);
    groups.get(r.account_id).push(r);
  }
  const ids = [...groups.keys()].sort((x, y) => (acct(x)?.name || '').localeCompare(acct(y)?.name || ''));
  return ids.map(id => {
    const rows = groups.get(id);
    const tot = rows.filter(r => r.active).reduce((s, r) => s + r.amount_cents, 0);
    return `<tr class="rec-group"><td colspan="9" style="background:var(--surface-2, rgba(128,128,128,.1));font-weight:700">
      <i class="bi bi-wallet2"></i> ${esc(acct(id)?.name || '—')}
      <span style="font-weight:500;color:var(--text-muted);font-size:.8rem"> · ${rows.length} · ${money(tot, true)}</span></td></tr>`
      + rows.map(recRowHTML).join('');
  }).join('');
}

function paintRecHeaders() {
  document.querySelectorAll('th[data-recsort]').forEach(th => {
    th.style.cursor = 'pointer';
    th.style.userSelect = 'none';
    th.querySelector('.sort-ind')?.remove();
    if (th.dataset.recsort === REC_VIEW.key) {
      const i = document.createElement('span');
      i.className = 'sort-ind';
      i.textContent = REC_VIEW.dir === 1 ? ' ▲' : ' ▼';
      th.appendChild(i);
    }
  });
  const g = document.getElementById('recGroup');
  if (g) g.checked = REC_VIEW.group;
}

async function renderRecurrences() {
  const body = document.getElementById('recBody');
  const kpiBox = document.getElementById('recKpis');
  if (!body) return;
  let ent = 0, usc = 0;
  S.recurrences.filter(r => r.active).forEach(r => { r.amount_cents > 0 ? ent += r.amount_cents : usc -= r.amount_cents; });
  if (kpiBox) {
    kpiBox.innerHTML = `
      <div class="kpi"><div class="k-l">${t('rec.kpi.in')}</div><div class="k-v num pos">${money(ent)}</div></div>
      <div class="kpi"><div class="k-l">${t('rec.kpi.out')}</div><div class="k-v num neg">${money(usc)}</div></div>
      <div class="kpi"><div class="k-l">${t('rec.kpi.margin')}</div>
        <div class="k-v num ${ent - usc < 0 ? 'neg' : 'pos'}">${money(ent - usc, true)}</div>
        <div class="k-s">${t('rec.kpi.margin_sub')}</div></div>`;
  }
  if (!S.recurrences.length) {
    body.innerHTML = `<tr><td colspan="9"><div class="empty"><i class="bi bi-arrow-repeat"></i>
      <h6>${t('rec.empty')}</h6><p>${t('rec.empty_hint')}</p></div></td></tr>`;
    return;
  }
  body.innerHTML = recGroupedHTML(S.recurrences);
  paintRecHeaders();
  fillNextOccurrences();
}

async function fillNextOccurrences() {
  const to = addDays(TODAY, 400);
  await Promise.all(S.recurrences.filter(r => r.active).map(async r => {
    const cell = document.querySelector(`[data-rec-next="${r.id}"]`);
    if (!cell) return;
    try {
      const occ = await api(`/recurrences/${encodeURIComponent(r.id)}/occurrences?from=${TODAY}&to=${to}`);
      const next = occ.find(o => o.date >= TODAY);
      REC_VIEW.next[r.id] = next ? next.date : '';
      cell.textContent = next ? fmtDate(next.date) : '—';
    } catch {
      cell.textContent = '—';
    }
  }));
  /* Ordinando per "Prossima" le date arrivano dopo il primo disegno: riordina una volta sola. */
  if (REC_VIEW.key === 'next' && !REC_VIEW.resorted) {
    REC_VIEW.resorted = true;
    renderRecurrences();
  }
  REC_VIEW.resorted = false;
}

async function deleteRecurrence(id) {
  const r = S.recurrences.find(x => x.id === id);
  try {
    await api(`/recurrences/${encodeURIComponent(id)}`, {method: 'DELETE'});
    await loadRecurrences();
    renderRecurrences();
    await refreshRegister();
    toast(t('toast.rec_deleted', {name: r?.description || ''}));
  } catch (err) { fail(err, 'eliminazione ricorrenza'); }
}

/* ============================================================
   TRASFERIMENTI
   ============================================================ */
function transferAccountOptions() {
  return writableAccounts().map(a => accountOption(a, ' · ' + money(balShown(a.id)))).join('');
}

function updateTransferAvail() {
  const el = document.getElementById('trAvail');
  if (!el) return;
  const id = document.getElementById('trFrom')?.value;
  const a = acct(id);
  if (!a) { el.textContent = ''; return; }
  const real = a.kind !== 'virtual' ? balReal(id) : balShown(id);
  const al = a.kind !== 'virtual' ? allocatedOn(id) : 0;
  el.textContent = t('tr.available', {amount: money(real)}) + (al ? ' · ' + t('tr.allocated', {amount: money(al)}) : '');
}

/* Cella conto: nome e, sotto, la nota del conto, per distinguere conti con lo stesso nome. */
function trAcctCell(id) {
  const a = acct(id);
  if (!a) return '—';
  return `<div>${esc(a.name)}</div>${a.note ? `<div style="font-size:.75rem;color:var(--text-muted)">${esc(a.note)}</div>` : ''}`;
}

async function renderTransfers() {
  const from = document.getElementById('trFrom'), to = document.getElementById('trTo');
  const body = document.getElementById('trHistory');
  if (!from || !to || !body) return;
  const opts = transferAccountOptions();
  const keepFrom = from.value, keepTo = to.value;
  from.innerHTML = opts;
  to.innerHTML = opts;
  if (keepFrom) from.value = keepFrom;
  if (keepTo) to.value = keepTo;
  else {
    const second = writableAccounts()[1];
    if (second) to.value = second.id;
  }
  const dateEl = document.getElementById('trDate');
  if (dateEl && !dateEl.value) dateEl.value = TODAY;
  const trPer = document.getElementById('trPeriod');
  if (trPer) trPer.value = S.trPeriod;
  updateTransferAvail();

  body.setAttribute('aria-busy', 'true');
  let forecasts = [];
  try {
    const r = transfersRange();
    /* Le chiusure carta (pair_id `cc:…`) sono registrate come coppie di righe ma non sono
       trasferimenti tra conti scelti dall'utente: restano fuori dall'elenco. */
    S.transfers = (await api(`/transfers?from=${r.from}&to=${r.to}`)).filter(x => !String(x.pair_id || '').startsWith('cc:'));
    forecasts = await transferForecasts(r);
  } catch (err) {
    body.removeAttribute('aria-busy');
    body.innerHTML = `<tr><td colspan="6"><div class="empty"><i class="bi bi-exclamation-triangle"></i>
      <h6>${t('tr.load_failed')}</h6><p>${esc(apiErrorToMessage(err))}</p></div></td></tr>`;
    return;
  }
  body.removeAttribute('aria-busy');
  const real = S.transfers.map(x => ({date: x.date, html: `
    <tr class="${S.trEditing === x.pair_id ? 'is-hl' : ''}"><td>${fmtDate(x.date)}</td>
      <td>${trAcctCell(x.from_account_id)}</td><td>${trAcctCell(x.to_account_id)}</td>
      <td class="r num">${money(x.cents)}</td><td>${esc(x.description || '')}</td>
      <td class="r" style="white-space:nowrap">
        <button class="rowbtn" data-tredit="${esc(x.pair_id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>
        <button class="rowbtn del" data-trdel="${esc(x.pair_id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i></button>
      </td></tr>`}));
  const future = forecasts.map(x => ({date: x.date, html: `
    <tr class="is-future"><td>${fmtDate(x.date)}</td>
      <td>${trAcctCell(x.from_account_id)}</td><td>${trAcctCell(x.to_account_id)}</td>
      <td class="r num">${money(x.cents)}</td>
      <td>${esc(x.description || '')}<span class="tag-prev">${t('tag.forecast')}</span></td>
      <td class="r" style="white-space:nowrap">
        <button class="rowbtn" data-trrec="${esc(x.rec_id)}" title="${esc(t('tr.edit_rule'))}" aria-label="${esc(t('tr.edit_rule'))}"><i class="bi bi-arrow-repeat"></i></button>
      </td></tr>`}));
  const rows = [...real, ...future].sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
  body.innerHTML = rows.length ? rows.map(x => x.html).join('')
    : `<tr><td colspan="6"><div class="empty"><i class="bi bi-arrow-left-right"></i>
        <h6>${t('tr.empty')}</h6><p>${t('tr.empty_hint')}</p></div></td></tr>`;
}

/* Periodo dell'elenco trasferimenti: scelta propria della pagina, indipendente dal registro
   (che di default mostra il solo mese corrente e nasconderebbe storico e mesi futuri). */
function transfersRange() {
  const d = fromISO(TODAY), y = d.getFullYear(), m = d.getMonth();
  switch (S.trPeriod) {
    case 'year': return {from: `${y}-01-01`, to: `${y}-12-31`};
    case 'last12': return {from: som(y, m - 11), to: eom(y, m)};
    case 'next12': return {from: som(y, m), to: eom(y, m + 11)};
    case 'all': return {from: '2000-01-01', to: eom(y, m + 12)};
    default: return {from: som(y, m - 3), to: eom(y, m + 3)};
  }
}

/* Occorrenze future delle ricorrenze di tipo trasferimento: non sono ancora righe del database
   (si registrano alla scadenza), ma devono comparire tra i trasferimenti previsti. */
async function transferForecasts(r) {
  const registered = new Set(S.transfers.map(x => x.pair_id));
  const out = [];
  for (const rec of S.recurrences.filter(x => x.linked_account_id && x.active !== false)) {
    const occ = await api(`/recurrences/${encodeURIComponent(rec.id)}/occurrences?from=${r.from}&to=${r.to}`);
    for (const o of occ) {
      if (o.date <= TODAY || registered.has(`rec-pair:${rec.id}:${o.date}`)) continue;
      out.push({date: o.date, rec_id: rec.id, from_account_id: rec.account_id, to_account_id: rec.linked_account_id,
        cents: Math.abs(o.amount_cents), description: o.description || rec.description});
    }
  }
  return out;
}

function resetTransferForm() {
  S.trEditing = null;
  const set = (id, v) => { const el = document.getElementById(id); if (el) el.value = v; };
  set('trAmount', ''); set('trDesc', ''); set('trDate', TODAY);
  const submit = document.getElementById('trSubmit');
  if (submit) submit.textContent = t('tr.submit');
  const cancel = document.getElementById('trCancel');
  if (cancel) cancel.style.display = 'none';
  renderTransfers();
}

async function submitTransfer() {
  const fromId = document.getElementById('trFrom')?.value;
  const toId = document.getElementById('trTo')?.value;
  const cents = parseAmount(document.getElementById('trAmount')?.value);
  const date = document.getElementById('trDate')?.value || TODAY;
  const desc = (document.getElementById('trDesc')?.value || '').trim();
  if (!fromId || !toId) { toast(t('toast.no_writable')); return; }
  if (fromId === toId) { toast(t('tr.same_account')); return; }
  if (!cents) { toast(t('toast.amount_required')); return; }
  const submit = document.getElementById('trSubmit');
  if (submit) submit.disabled = true;
  try {
    const body = {from_account_id: fromId, to_account_id: toId, date, cents, description: desc || null};
    if (S.trEditing) {
      await api(`/transfers/${encodeURIComponent(S.trEditing)}`, {method: 'PUT', body: JSON.stringify(body)});
      toast(t('tr.updated'));
    } else {
      await api('/transfers', {method: 'POST', body: JSON.stringify(body)});
      toast(t('tr.done', {amount: money(cents), from: acct(fromId)?.name || '', to: acct(toId)?.name || ''}));
    }
    S.trEditing = null;
    await refreshAll();
    resetTransferForm();
  } catch (err) {
    fail(err, 'trasferimento');
  } finally {
    if (submit) submit.disabled = false;
  }
}

async function deleteTransfer(pairId) {
  const x = S.transfers.find(y => y.pair_id === pairId);
  try {
    await api(`/transfers/${encodeURIComponent(pairId)}`, {method: 'DELETE'});
    if (S.trEditing === pairId) S.trEditing = null;
    await refreshAll();
    resetTransferForm();
    toast(t('tr.deleted', {amount: money(x?.cents || 0)}));
  } catch (err) { fail(err, 'eliminazione trasferimento'); }
}

function editTransfer(pairId) {
  const x = S.transfers.find(y => y.pair_id === pairId);
  if (!x) return;
  S.trEditing = pairId;
  const set = (id, v) => { const el = document.getElementById(id); if (el) el.value = v; };
  set('trFrom', x.from_account_id);
  set('trTo', x.to_account_id);
  set('trAmount', formatAmountInput(x.cents));
  set('trDate', x.date);
  set('trDesc', x.description || '');
  updateTransferAvail();
  const submit = document.getElementById('trSubmit');
  if (submit) submit.textContent = t('tr.update');
  const cancel = document.getElementById('trCancel');
  if (cancel) cancel.style.display = '';
  renderTransfers();
  document.getElementById('trAmount')?.focus();
}

/* ============================================================
   CONTI
   ============================================================ */
function renderAccountsPage() {
  const kpiBox = document.getElementById('contiKpis');
  const grid = document.getElementById('acctGrid');
  if (!grid) return;
  const vis = visibleAccounts();
  if (!vis.length) {
    if (kpiBox) kpiBox.innerHTML = '';
    grid.innerHTML = `<div class="empty"><i class="bi bi-bank"></i><h6>${t('empty.no_accounts')}</h6>
      <p>${t('empty.no_accounts_hint')}</p></div>`;
    return;
  }
  const liquid = vis.filter(a => a.kind !== 'card' && a.kind !== 'virtual').reduce((s, a) => s + balReal(a.id), 0);
  const carte = vis.filter(a => a.kind === 'card').reduce((s, a) => s + balReal(a.id), 0);
  const alloc = vis.filter(a => a.kind === 'virtual').reduce((s, a) => s + balShown(a.id), 0);
  if (kpiBox) {
    kpiBox.innerHTML = `
      <div class="kpi"><div class="k-l">${t('acct.kpi.networth')}</div><div class="k-v num">${money(netWorth())}</div>
        <div class="k-s">${t('acct.kpi.networth_sub', {real: vis.filter(a => a.kind !== 'virtual').length, piggy: vis.filter(a => a.kind === 'virtual').length})}</div></div>
      <div class="kpi"><div class="k-l">${t('acct.kpi.liquidity')}</div><div class="k-v num pos">${money(liquid)}</div>
        <div class="k-s">${t('acct.kpi.liquidity_sub')}</div></div>
      <div class="kpi"><div class="k-l">${t('acct.kpi.cards')}</div><div class="k-v num ${carte < 0 ? 'neg' : ''}">${money(carte)}</div>
        <div class="k-s">${t('acct.kpi.cards_sub')}</div></div>
      <div class="kpi"><div class="k-l">${t('acct.kpi.allocated')}</div><div class="k-v num">${money(alloc)}</div>
        <div class="k-s">${t('acct.kpi.allocated_sub')}</div></div>`;
  }

  grid.innerHTML = vis.map(a => {
    const b = a.kind === 'virtual' ? balShown(a.id) : balReal(a.id);
    const al = a.kind === 'virtual' ? 0 : allocatedOn(a.id);
    let extra = '';
    if (a.kind === 'card') {
      const used = cardUsed(a, b);
      const mass = a.limit_cents || 0;
      const pct = mass ? Math.round(used / mass * 100) : 0;
      extra = `<div class="meter"><i class="${pct > 85 ? 'over' : pct > 60 ? 'warn' : ''}" style="width:${Math.min(100, pct)}%"></i></div>
        <div class="a-meta">${t('acct.card_avail', {avail: money(mass - used), limit: money(mass)})}</div>
        ${a.charge_day && a.primary_account_id && acct(a.primary_account_id)
        ? `<div class="a-meta">${t('acct.card_charge', {day: a.charge_day, account: esc(acct(a.primary_account_id).name)})}${a.closing_day ? ' · ' + t('acct.card_closing', {day: a.closing_day}) : ''}</div>` : ''}`;
    } else if (a.kind === 'deposit') {
      extra = `<div class="a-meta">${a.rate_pct_bps ? t('acct.rate', {pct: nf.format(a.rate_pct_bps / 100)}) : t('kpi.no_rate')}${a.primary_account_id && acct(a.primary_account_id)
        ? ' · ' + t('acct.linked_to', {account: esc(acct(a.primary_account_id).name)}) : ''}</div>`;
    } else if (a.kind === 'virtual') {
      const pct = a.goal_cents ? Math.min(100, Math.round(b / a.goal_cents * 100)) : 0;
      extra = `${a.goal_cents ? `<div class="meter"><i style="width:${pct}%"></i></div>
        <div class="a-meta">${money(b)} / ${money(a.goal_cents)} · ${t('acct.of_goal', {pct})}</div>` : ''}
        ${a.primary_account_id && acct(a.primary_account_id) ? `<div class="a-meta">${t('acct.set_aside_on', {account: esc(acct(a.primary_account_id).name)})}</div>` : ''}
        <div class="a-meta">${S.virtualOn.has(a.id) ? t('acct.piggy_shown') : t('acct.piggy_hidden')}</div>`;
    } else if (a.iban) {
      extra = `<div class="a-meta">IBAN ····${esc(a.iban.slice(-4))}</div>`;
    }
    const shares = a.shares || [];
    const isOwned = a.owner_id === S.user?.id;
    return `<div class="acct-card" draggable="true" data-acct-drag="${esc(a.id)}">
      ${isOwned ? `<span class="acct-drag-handle" title="Trascina per riordinare"><i class="bi bi-grip-vertical"></i></span>` : ''}
      <div class="a-top"><span class="a-name"><i class="bi ${iconOf(a)}"></i> ${esc(a.name)}</span>
        <span class="badge-tipo t-${tipoOf(a)}">${esc(kindLabel(a))}</span></div>
      ${!isOwned
      ? `<div class="a-meta">${t('acct.owned_by', {name: esc(userName(a.owner_id))})}</div>`
      : (shares.length ? `<div class="a-meta">${t('acct.shared_with', {names: shares.map(s => esc(userName(s.user_id)) + (s.level === 'write' ? '' : ' (' + t('acct.read') + ')')).join(', ')})}</div>` : '')}
      <div class="a-bal ${b < 0 ? 'neg' : ''}">${money(b)}</div>
      ${a.note ? `<div class="a-meta">${esc(a.note)}</div>` : ''}
      ${al ? `<div class="a-meta">${t('acct.of_which_allocated', {amount: money(al)})}</div>` : ''}
      ${feeSummary(a.id).map(f => `<div class="a-meta">${esc(f.nome)}: ${f.dovuto ? money(f.cents) + ' ' + t('rule.on', {date: fmtDate(f.date)}) : t('rule.not_due')}</div>`).join('')}
      ${extra}
      <div class="a-actions">
        <button class="btn-tool" data-open-acct="${esc(a.id)}">${t('acct.open_register')}</button>
        ${canWrite(a) ? `<button class="btn-tool" data-rules-acct="${esc(a.id)}" title="${esc(t('acct.fees_title'))}"><i class="bi bi-percent"></i> ${t('acct.fees')}</button>
        ${a.kind === 'card' ? `<button class="btn-tool" data-card-charge="${esc(a.id)}"><i class="bi bi-credit-card"></i> ${t('acct.charge')}</button>` : ''}
        ${a.kind === 'deposit' ? `<button class="btn-tool" data-deposit="${esc(a.id)}"><i class="bi bi-percent"></i> ${t('acct.consolidate')}</button>` : ''}
        <button class="btn-tool" data-import-acct="${esc(a.id)}" title="${esc(t('reg.import'))}"><i class="bi bi-upload"></i> ${t('reg.import')}</button>
        <button class="btn-tool" data-export-acct="${esc(a.id)}" title="${esc(t('acct.export'))}"><i class="bi bi-download"></i></button>
        <button class="btn-tool" data-edit-acct="${esc(a.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>`
      : `<span class="badge-ro" style="align-self:center"><i class="bi bi-eye"></i> ${t('reg.readonly_short')}</span>
        <button class="btn-tool" data-export-acct="${esc(a.id)}" title="${esc(t('acct.export'))}"><i class="bi bi-download"></i></button>`}
      </div></div>`;
  }).join('');
  wireAcctDnd();
}

/* ---------- ordinamento conti (drag-and-drop) ---------- */
let _dndSrc = null;

function wireAcctDnd() {
  const grid = document.getElementById('acctGrid');
  if (!grid) return;
  const cards = grid.querySelectorAll('.acct-card[data-acct-drag]');
  cards.forEach(card => {
    card.addEventListener('dragstart', e => {
      _dndSrc = card;
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', card.dataset.acctDrag);
      setTimeout(() => card.classList.add('dnd-dragging'), 0);
    });
    card.addEventListener('dragend', () => {
      card.classList.remove('dnd-dragging');
      grid.querySelectorAll('.acct-card').forEach(c => c.classList.remove('dnd-over'));
      _dndSrc = null;
    });
    card.addEventListener('dragover', e => {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      if (!_dndSrc || card === _dndSrc) return;
      grid.querySelectorAll('.acct-card').forEach(c => c.classList.remove('dnd-over'));
      card.classList.add('dnd-over');
      const rect = card.getBoundingClientRect();
      const insertAfter = e.clientX > rect.left + rect.width / 2;
      grid.insertBefore(_dndSrc, insertAfter ? card.nextSibling : card);
    });
    card.addEventListener('drop', async e => {
      e.preventDefault();
      grid.querySelectorAll('.acct-card').forEach(c => c.classList.remove('dnd-over'));
      const ids = [...grid.querySelectorAll('.acct-card[data-acct-drag]')].map(c => c.dataset.acctDrag);
      await reorderAccounts(ids);
    });
  });
}

async function reorderAccounts(ids) {
  try {
    await api('/accounts/reorder', {method: 'POST', body: JSON.stringify({ids})});
    await loadAccounts();
    renderSidebarAccounts();
    renderAccountsPage();
  } catch (err) {
    fail(err, 'riordinamento conti');
    await loadAccounts();
    renderAccountsPage();
  }
}

/* ---------- azioni carta / deposito ---------- */
async function cardCharge(accountId) {
  try {
    const dry = await api(`/accounts/${encodeURIComponent(accountId)}/card/catch-up`, {method: 'POST', body: JSON.stringify({dry_run: true})});
    if (!dry.total_charge_cents) { toast(t('acct.no_charge')); return; }
    toast(t('acct.charge_preview', {amount: money(dry.total_charge_cents), date: fmtDate(dry.latest_charge_date), cycles: dry.cycles_settled}), t('acct.charge_confirm'), async () => {
      try {
        const res = await api(`/accounts/${encodeURIComponent(accountId)}/card/catch-up`, {method: 'POST', body: JSON.stringify({dry_run: false})});
        await refreshAll();
        toast(t('acct.charge_done', {amount: money(res.total_charge_cents), cycles: res.cycles_settled}));
      } catch (err) { fail(err, 'addebito carta'); }
    }, 12000);
  } catch (err) { fail(err, 'anteprima addebito'); }
}

async function depositConsolidate(accountId) {
  const a = acct(accountId);
  const {first, last} = monthBounds(S.month);
  const from = a?.start_date && a.start_date > first ? a.start_date : first;
  try {
    const dry = await api(`/accounts/${encodeURIComponent(accountId)}/deposit/consolidate`, {
      method: 'POST', body: JSON.stringify({from, to: last, dry_run: true}),
    });
    if (!dry.net_cents) { toast(t('acct.no_interest')); return; }
    toast(t('acct.interest_preview', {gross: money(dry.gross_cents), tax: money(dry.withholding_cents), net: money(dry.net_cents)}),
      t('acct.interest_confirm'), async () => {
        try {
          const res = await api(`/accounts/${encodeURIComponent(accountId)}/deposit/consolidate`, {
            method: 'POST', body: JSON.stringify({from, to: last, dry_run: false}),
          });
          await refreshAll();
          toast(t('acct.interest_done', {amount: money(res.net_cents)}));
        } catch (err) { fail(err, 'consolidamento interessi'); }
      }, 12000);
  } catch (err) { fail(err, 'anteprima interessi'); }
}

/* ---------- export ---------- */
async function exportCSV(accountId) {
  const id = accountId === 'all'
    ? (visibleAccounts().find(a => a.kind !== 'virtual')?.id)
    : accountId;
  if (!id) { toast(t('empty.no_accounts')); return; }
  const r = registerRange();
  try {
    setBusy(true);
    const res = await fetch(`${API}/export?${registerQuery(id, r.from, r.to)}`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: {'X-CSRF-Token': getCsrfCookie() || S.csrfToken},
    });
    if (!res.ok) {
      let err = {};
      try { err = await res.json(); } catch { /* corpo non JSON */ }
      throw new ApiError(err.code || 'errors.internal', err.detail, res.status);
    }
    const blob = await res.blob();
    const cd = res.headers.get('content-disposition') || '';
    const m = cd.match(/filename="?([^"]+)"?/);
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = m ? m[1] : `${(acct(id)?.name || 'export').replace(/\s+/g, '_')}_${TODAY}.csv`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast(t('acct.exported', {account: acct(id)?.name || ''}));
  } catch (err) {
    fail(err, 'export');
  } finally {
    setBusy(false);
  }
}

/* ============================================================
   IMPOSTAZIONI
   ============================================================ */
function showSetTab(tab) {
  if (['usr', 'acc', 'hol'].includes(tab) && !isAdmin()) tab = 'vis';
  S.setTab = tab;
  document.querySelectorAll('#setTabs button').forEach(b => b.classList.toggle('is-on', b.dataset.tab === tab));
  document.querySelectorAll('.set-panel').forEach(p => p.classList.toggle('is-on', p.id === 'tab-' + tab));
  if (tab === 'cat') renderCatList();
  if (tab === 'tag') renderTagList();
  if (tab === 'hol') openHolidayAdmin();
  if (tab === 'usr') renderUsers();
  if (tab === 'acc') renderAdminAccounts();
}

function renderSettings() {
  /* Le schede riservate sono già governate da `body.is-admin .admin-only` nel
     CSS: basta la classe sul body, senza stili inline che la contraddicano. */
  document.body.classList.toggle('is-admin', isAdmin());
  showSetTab(S.setTab);
}

function syncSettingsControls() {
  const set = (id, prop, v) => { const el = document.getElementById(id); if (el) el[prop] = v; };
  set('setTheme', 'value', S.theme);
  set('setHolidays', 'value', S.holidayCalendar);
  set('setSign', 'checked', S.showSign);
  set('setMSaldo', 'checked', S.showMSaldo);
  set('setPrivacy', 'checked', S.privacy);
  set('setEmptyDays', 'checked', S.showEmpty);
  set('setFuture', 'checked', S.showFuture);
  set('setAbove', 'value', S.rows.above);
  set('setBelow', 'value', S.rows.below);
  set('setStep', 'value', S.rows.step);
  document.getElementById('emptyDaysBtn')?.classList.toggle('is-on', S.showEmpty);
  document.body.classList.toggle('show-msaldo', S.showMSaldo);
  const pi = document.getElementById('privacyIcon');
  if (pi) pi.className = S.privacy ? 'bi bi-eye-slash' : 'bi bi-eye';
  const pb = document.getElementById('privacyBtn');
  if (pb) pb.title = S.privacy ? t('set.privacy_show') : t('set.privacy');
}

/* Colori degli importi. Il valore finisce in una proprietà CSS: accetto solo
   '#rrggbb' (come il server) e '' per il predefinito del tema. */
const AMOUNT_COLOR_DEFAULTS = {pos: '#0e7a4a', neg: '#c3282f'};
const validAmountColor = c => c === '' || /^#[0-9a-fA-F]{6}$/.test(c || '');
function applyAmountColors() {
  const root = document.documentElement.style;
  if (S.colorPos) root.setProperty('--amt-pos', S.colorPos); else root.removeProperty('--amt-pos');
  if (S.colorNeg) root.setProperty('--amt-neg', S.colorNeg); else root.removeProperty('--amt-neg');
  const pos = document.getElementById('setColorPos');
  const neg = document.getElementById('setColorNeg');
  if (pos) pos.value = S.colorPos || AMOUNT_COLOR_DEFAULTS.pos;
  if (neg) neg.value = S.colorNeg || AMOUNT_COLOR_DEFAULTS.neg;
  document.getElementById('setColorPosReset')?.toggleAttribute('disabled', !S.colorPos);
  document.getElementById('setColorNegReset')?.toggleAttribute('disabled', !S.colorNeg);
}

/* `empty_days` è un intero nel DTO delle preferenze: lo uso come flag 0/1,
   non come booleano — era il motivo dei 422 sul salvataggio. */
async function savePrefs() {
  try {
    await api('/auth/prefs', {
      method: 'PUT',
      body: JSON.stringify({
        theme: S.theme,
        show_sign: S.showSign,
        privacy: S.privacy,
        show_m_saldo: S.showMSaldo,
        empty_days: S.showEmpty ? 1 : 0,
        show_future: S.showFuture,
        rows_above: S.rows.above,
        rows_below: S.rows.below,
        rows_step: S.rows.step,
        holiday_calendar: S.holidayCalendar,
        color_pos: S.colorPos,
        color_neg: S.colorNeg,
      }),
    });
  } catch (err) {
    fail(err, 'salvataggio preferenze');
  }
}

async function loadPrefs() {
  try {
    const p = await api('/auth/prefs');
    if (!p) return;
    S.theme = ['auto', 'light', 'dark'].includes(p.theme) ? p.theme : 'auto';
    S.showSign = p.show_sign !== false;
    S.privacy = !!p.privacy;
    S.showMSaldo = !!p.show_m_saldo;
    S.showEmpty = Number(p.empty_days) !== 0;
    S.showFuture = p.show_future !== false;
    if (p.rows_above >= 0) S.rows.above = p.rows_above;
    if (p.rows_below > 0) S.rows.below = p.rows_below;
    if (p.rows_step > 0) S.rows.step = p.rows_step;
    if (p.holiday_calendar) S.holidayCalendar = p.holiday_calendar;
    S.colorPos = validAmountColor(p.color_pos) ? p.color_pos : '';
    S.colorNeg = validAmountColor(p.color_neg) ? p.color_neg : '';
    applyAmountColors();
  } catch (err) {
    console.warn('preferenze non caricate', err);
  }
}

/* ---------- tema ---------- */
const mq = window.matchMedia('(prefers-color-scheme: dark)');
function applyTheme() {
  const dark = S.theme === 'dark' || (S.theme === 'auto' && mq.matches);
  document.body.classList.toggle('theme-dark', dark);
  const icon = document.getElementById('themeIcon');
  if (icon) icon.className = dark ? 'bi bi-sun' : 'bi bi-moon-stars';
  const meta = document.querySelector('meta[name=theme-color]');
  if (meta) meta.setAttribute('content', dark ? '#0b0e15' : '#161b2e');
  if (S.page === 'statistiche') renderStats();
}
mq.addEventListener('change', () => { if (S.theme === 'auto') applyTheme(); });

async function setPrivacy(v) {
  S.privacy = v;
  syncSettingsControls();
  await savePrefs();
  await refreshAll();
}

/* ============================================================
   CATEGORIE
   ============================================================ */
const EMOJI_SET = ['🛒', '🍎', '🥖', '🍽️', '☕', '🍕', '🏠', '🔑', '🛋️', '💡', '🔥', '💧', '📱', '💻', '📶',
  '🚗', '⛽', '🚌', '🚕', '✈️', '🚲', '💊', '🩺', '🦷', '🏋️', '⚽', '🎬', '🎮', '🎵', '📚', '🎓', '🛍️', '👕',
  '👟', '🎁', '🐶', '🐱', '👶', '💼', '💰', '📈', '🏦', '🧾', '🧰', '✂️', '🧴', '🌿', '🎉', '✳️', '❓'];
let catEditing = null;
let catForm = {tipo: 'uscita', emoji: '✳️'};

function catUseCount(id) {
  let n = 0;
  LED.forEach(d => d.entries.forEach(e => { if (e.cat === id) n++; }));
  return n;
}

function renderCatList() {
  const box = document.getElementById('catList');
  if (!box) return;
  if (!S.categories.length) {
    box.innerHTML = `<p style="color:var(--text-muted);margin:0">${t('modal.entry.no_cats')}</p>`;
    return;
  }
  const gruppi = [['out', t('common.outflow') + 'e'], ['in', t('common.inflow') + 'e']];
  box.innerHTML = gruppi.map(([kind, label]) => {
    const list = S.categories.filter(c => c.kind === kind);
    if (!list.length) return '';
    return `<div class="cat-group">${esc(label)} · ${list.length}</div>` + list.map(c => `
      <div class="cat-row">
        <span class="ce">${esc(c.emoji)}</span>
        <span class="cn">${esc(c.name)}
          <div class="cb">${t('cat.used', {n: catUseCount(c.id)})}${c.budget_cents ? ' · ' + t('cat.budget', {amount: money(c.budget_cents)}) : ''}${c.is_fallback ? ' · ' + t('cat.fallback') : ''}</div></span>
        <span class="racts">
          ${c.editable === false ? '' : `<button class="rowbtn" data-cat-edit="${esc(c.id)}" aria-label="${esc(t('reg.action.edit'))} ${esc(c.name)}"><i class="bi bi-pencil"></i></button>`}
        </span></div>`).join('');
  }).join('');
}

function openCatModal(id = null) {
  catEditing = id;
  const c = id ? S.categories.find(x => x.id === id) : null;
  catForm.tipo = c ? (c.kind === 'in' ? 'entrata' : 'uscita') : 'uscita';
  catForm.emoji = c ? (c.emoji || '✳️') : '✳️';
  document.getElementById('catModalTitle').textContent = c ? t('modal.cat.edit') : t('modal.cat.title');
  document.getElementById('ctNome').value = c ? c.name : '';
  document.getElementById('ctBudget').value = c && c.budget_cents ? formatAmountInput(c.budget_cents) : '';
  document.getElementById('ctEmoji').value = '';
  const del = document.getElementById('ctDelete');
  del.style.display = c && !c.is_fallback ? '' : 'none';
  del.dataset.armed = '';
  del.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.cat.delete')}`;
  syncCatForm();
  openModal('catModal');
}

function syncCatForm() {
  document.querySelectorAll('#ctTipo button').forEach(b => {
    b.classList.toggle('is-on', b.dataset.tipo === catForm.tipo);
    b.disabled = !!catEditing;
  });
  document.getElementById('ctPreview').textContent = catForm.emoji;
  document.getElementById('ctBudgetBox').style.display = catForm.tipo === 'uscita' ? '' : 'none';
  document.getElementById('ctTipoHint').textContent = catEditing ? t('modal.cat.kind_locked') : '';
  document.getElementById('ctEmojiGrid').innerHTML = EMOJI_SET.map(e =>
    `<button type="button" class="${e === catForm.emoji ? 'is-on' : ''}" data-emoji="${e}">${e}</button>`).join('');
}

async function saveCategory() {
  const nome = document.getElementById('ctNome').value.trim();
  if (!nome) { toast(t('modal.cat.name_required')); document.getElementById('ctNome').focus(); return; }
  const budget = catForm.tipo === 'uscita' ? (parseAmount(document.getElementById('ctBudget').value) || null) : null;
  const btn = document.getElementById('ctSave');
  if (btn) btn.disabled = true;
  try {
    if (catEditing) {
      await api(`/categories/${encodeURIComponent(catEditing)}`, {
        method: 'PUT', body: JSON.stringify({name: nome, emoji: catForm.emoji, budget_cents: budget}),
      });
    } else {
      await api('/categories', {
        method: 'POST',
        body: JSON.stringify({name: nome, emoji: catForm.emoji, kind: catForm.tipo === 'entrata' ? 'in' : 'out', budget_cents: budget}),
      });
    }
    closeModal('catModal');
    await loadCategories();
    refreshCategoryUI();
    toast(t('modal.cat.saved', {name: nome}));
  } catch (err) {
    fail(err, 'salvataggio categoria');
  } finally {
    if (btn) btn.disabled = false;
  }
}

/* Doppio clic deliberato sullo stesso pulsante, con conteggio dell'impatto:
   nessun confirm() di sistema, come nel mockup. */
async function deleteCategory() {
  const c = S.categories.find(x => x.id === catEditing);
  if (!c) return;
  if (c.is_fallback) { toast(t('modal.cat.fallback_protected')); return; }
  const btn = document.getElementById('ctDelete');
  if (btn.dataset.armed !== '1') {
    btn.dataset.armed = '1';
    btn.innerHTML = `<i class="bi bi-exclamation-triangle"></i> ${t('modal.cat.confirm_delete', {n: catUseCount(c.id)})}`;
    setTimeout(() => {
      btn.dataset.armed = '';
      btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.cat.delete')}`;
    }, 5000);
    return;
  }
  btn.dataset.armed = '';
  btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.cat.delete')}`;
  try {
    await api(`/categories/${encodeURIComponent(c.id)}`, {method: 'DELETE'});
    closeModal('catModal');
    await loadCategories();
    refreshCategoryUI();
    toast(t('modal.cat.deleted', {name: c.name}));
  } catch (err) { fail(err, 'eliminazione categoria'); }
}

/* Le categorie compaiono in mezza applicazione: ridisegno tutto ciò che le usa. */
function refreshCategoryUI() {
  const catOpts = `<option value="">${t('common.all')}</option>`
    + S.categories.map(c => `<option value="${esc(c.id)}">${esc(c.emoji)} ${esc(c.name)}</option>`).join('');
  const fCat = document.getElementById('fCat');
  if (fCat) { fCat.innerHTML = catOpts; fCat.value = S.filters.cat; }
  renderCatList();
  refreshAll();
}

/* ============================================================
   UTENTI (solo amministratori)
   ============================================================ */
let userEditing = null;

function renderUsers() {
  const body = document.getElementById('usersBody');
  if (!body || !isAdmin()) return;
  body.innerHTML = S.users.map(u => {
    const propri = S.adminAccounts.filter(a => a.owner_id === u.id).length;
    const isMe = u.id === S.user?.id;
    return `<tr style="${u.active === false ? 'opacity:.55' : ''}">
      <td><b>${esc(u.username)}</b>${isMe ? ` <span style="font-size:.72rem;color:var(--text-muted)">(${t('set.users.you')})</span>` : ''}</td>
      <td>${esc(u.email)}</td>
      <td><span class="badge-role ${u.role === 'admin' ? 'role-admin' : 'role-user'}">${u.role === 'admin' ? t('set.users.admin') : t('set.users.user')}</span></td>
      <td>${u.active === false ? `<span class="dot-off">●</span> ${t('set.users.suspended')}` : `<span class="dot-on">●</span> ${t('set.users.active')}`}</td>
      <td style="font-size:.82rem">${t('set.users.owned', {n: propri})}</td>
      <td>${u.last_seen_at ? fmtDate(u.last_seen_at.slice(0, 10)) : t('set.users.never')}</td>
      <td class="r" style="white-space:nowrap">
        <button class="rowbtn" data-user-imp="${esc(u.id)}" title="${esc(t('set.users.impersonate'))}" ${isMe ? 'disabled style="opacity:.35"' : ''}><i class="bi bi-person-badge"></i></button>
        <button class="rowbtn" data-user-edit="${esc(u.id)}" aria-label="${esc(t('reg.action.edit'))} ${esc(u.username)}"><i class="bi bi-pencil"></i></button>
        <button class="rowbtn del" data-user-del="${esc(u.id)}" aria-label="${esc(t('reg.action.delete'))} ${esc(u.username)}" ${isMe ? 'disabled style="opacity:.35"' : ''}><i class="bi bi-trash3"></i></button>
      </td></tr>`;
  }).join('');
}

function openUserModal(id = null) {
  userEditing = id;
  const u = id ? S.users.find(x => x.id === id) : null;
  document.getElementById('userModalTitle').textContent = u ? t('modal.user.edit') : t('modal.user.title');
  document.getElementById('usNome').value = u ? u.username : '';
  document.getElementById('usNome').disabled = !!u;   // lo username non è modificabile via PUT /users/{id}
  document.getElementById('usEmail').value = u ? u.email : '';
  document.getElementById('usEmail').disabled = !!u;  // idem per l'email
  document.getElementById('usRuolo').value = u ? u.role : 'user';
  document.getElementById('usAttivo').checked = u ? u.active !== false : true;
  document.getElementById('usPass').value = '';
  const del = document.getElementById('usDelete');
  del.style.display = (u && u.id !== S.user?.id) ? '' : 'none';
  del.dataset.armed = '';
  del.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.user.delete')}`;
  openModal('userModal');
}

async function saveUser() {
  const username = document.getElementById('usNome').value.trim();
  const email = document.getElementById('usEmail').value.trim();
  const role = document.getElementById('usRuolo').value;
  const active = document.getElementById('usAttivo').checked;
  const password = document.getElementById('usPass').value;
  if (!username || !email) { toast(t('modal.user.required')); return; }
  const btn = document.getElementById('usSave');
  if (btn) btn.disabled = true;
  try {
    if (userEditing) {
      await api(`/users/${encodeURIComponent(userEditing)}`, {method: 'PUT', body: JSON.stringify({role, active, ...(password ? {password} : {})})});
    } else {
      if (!password) { toast(t('modal.user.password_required')); return; }
      await api('/users', {method: 'POST', body: JSON.stringify({username, email, password, role})});
    }
    closeModal('userModal');
    await loadUsers();
    renderUsers();
    renderSideUser();
    toast(t('modal.user.saved', {name: username}));
  } catch (err) {
    fail(err, 'salvataggio utente');
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function deleteUser(id) {
  const u = S.users.find(x => x.id === id);
  if (!u || u.id === S.user?.id) return;
  try {
    await api(`/users/${encodeURIComponent(id)}`, {method: 'DELETE', body: JSON.stringify({})});
    closeModal('userModal');
    await Promise.all([loadUsers(), loadAdminAccounts()]);
    renderUsers();
    renderAdminAccounts();
    toast(t('modal.user.deleted', {name: u.username}));
  } catch (err) { fail(err, 'eliminazione utente'); }
}

/* ---------- impersonificazione ---------- */
async function startImpersonation(id) {
  if (id === S.user?.id) return;
  try {
    await api(`/admin/impersonate/${encodeURIComponent(id)}`, {method: 'POST'});
    if (!S.realUser) S.realUser = S.user;
    S.impersonating = id;
    const me = await api('/auth/me');
    S.user = me.user;
    await afterIdentityChange();
    toast(t('toast.impersonating', {name: S.user.username}));
  } catch (err) { fail(err, 'impersonificazione'); }
}

async function stopImpersonation() {
  try {
    await api('/admin/impersonate/stop', {method: 'POST'});
    S.impersonating = null;
    const me = await api('/auth/me');
    S.user = me.user;
    S.realUser = null;
    await afterIdentityChange();
    toast(t('toast.back_to_own'));
  } catch (err) { fail(err, 'fine impersonificazione'); }
}

function renderImpBar() {
  const bar = document.getElementById('impBar');
  if (!bar) return;
  bar.classList.toggle('is-on', !!S.realUser);
  if (!S.realUser) { bar.innerHTML = ''; return; }
  bar.innerHTML = `<i class="bi bi-person-badge"></i>
    ${t('imp.bar', {name: esc(S.user?.username || '')})}
    <button id="impExit">${t('imp.exit', {name: esc(S.realUser.username)})}</button>`;
}

/* Un cambio di identità cambia tutto ciò che dipende dai permessi. */
async function afterIdentityChange() {
  S.accountId = 'all';
  S.stats.acct = 'all';
  resetStatsFilters();
  stInvalidate();
  S.virtualOn.clear();
  const sa = document.getElementById('statsAccount');
  if (sa) sa.innerHTML = '';
  await loadAll();
  renderImpBar();
  renderSideUser();
  renderUsers();
  renderAdminAccounts();
  refreshCategoryUI();
  renderSettings();
  await refreshAll();
}

/* ---------- pannello amministrativo dei conti ---------- */
function renderAdminAccounts() {
  const body = document.getElementById('adminAcctBody');
  if (!body || !isAdmin()) return;
  if (!S.adminAccounts.length) {
    body.innerHTML = `<tr><td colspan="7"><div class="empty"><i class="bi bi-bank"></i><h6>${t('empty.no_accounts')}</h6></div></td></tr>`;
    return;
  }
  body.innerHTML = S.adminAccounts.map(a => {
    const local = acct(a.id);
    const sh = (local?.shares || []).map(x => `${esc(userName(x.user_id))} (${x.level === 'write' ? t('acct.write') : t('acct.read')})`).join(', ');
    return `<tr>
      <td><b><i class="bi ${iconOf(local || a)}"></i> ${esc(a.name)}</b>
        ${local?.note ? `<div style="font-size:.76rem;color:var(--text-muted)">${esc(local.note)}</div>` : ''}</td>
      <td><span class="badge-tipo t-${KIND_TO_TIPO[a.kind] || 'corrente'}">${esc(t(`acct_type.${a.kind}`))}</span></td>
      <td>${esc(a.owner_username)}</td>
      <td style="font-size:.82rem;color:var(--text-muted)">${sh || '—'}</td>
      <td class="r num">${money(balReal(a.id))}</td>
      <td>${a.active ? t('set.users.active') : t('set.users.suspended')}</td>
      <td class="r" style="white-space:nowrap">
        <button class="rowbtn" data-admin-edit="${esc(a.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>
        <button class="rowbtn del" data-admin-del="${esc(a.id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i></button>
      </td></tr>`;
  }).join('');
}

async function deleteAdminAccount(id) {
  const a = S.adminAccounts.find(x => x.id === id);
  try {
    await api(`/admin/accounts/${encodeURIComponent(id)}`, {method: 'DELETE', body: JSON.stringify({})});
    await Promise.all([loadAccounts(), loadAdminAccounts()]);
    if (S.accountId === id) S.accountId = 'all';
    renderAdminAccounts();
    await refreshAll();
    toast(t('acct.deleted', {name: a?.name || ''}));
  } catch (err) { fail(err, 'eliminazione conto'); }
}

/* Markup statico di index.html: [data-i18n] sostituisce il testo con t(chiave);
   il testo italiano nell'HTML resta solo come ripiego. Il resto dell'HTML statico
   non è tradotto (scelta esplicita): si traduce ciò che ha data-i18n.
   I placeholder degli importi seguono invece il formato numerico attivo. */
const AMOUNT_PLACEHOLDERS = {trAmount: 0, eAmount: 0, ruValore: 0, ruSoglia: 0, acBal: 0,
  acMass: 300000, acTasso: 300, acGoal: 150000, rAmount: 0};
function applyStaticI18n() {
  document.documentElement.lang = I18N_LOCALE;
  document.querySelectorAll('[data-i18n]').forEach(el => { el.textContent = t(el.dataset.i18n); });
  const tbMoreBtn = document.getElementById('tbMoreBtn');
  if (tbMoreBtn) tbMoreBtn.setAttribute('aria-label', t('reg.more_actions'));
  syncKpiToggle();
  for (const [id, cents] of Object.entries(AMOUNT_PLACEHOLDERS)) {
    const el = document.getElementById(id);
    if (el) el.placeholder = nf.format(cents / 100);
  }
}

function renderProfile() {
  const u = S.user;
  const info = document.getElementById('profileInfo');
  if (info && u) {
    const row = (k, v) => `<dt class="col-sm-4">${esc(t(k))}</dt><dd class="col-sm-8">${esc(v || '—')}</dd>`;
    info.innerHTML = row('profile.username', u.username) + row('profile.email', u.email)
      + row('profile.role', isAdmin() ? t('set.users.admin') : t('set.users.user'));
  }
  const sel = document.getElementById('profLang');
  if (sel) {
    sel.innerHTML = LANGS.map(l => `<option value="${esc(l.code)}">${esc(l.label)}</option>`).join('');
    sel.value = LANG.code;
  }
}

/* Cambio lingua: salva sul server, poi ricarica catalogo e ridisegna tutta la UI. */
async function changeLocale(code) {
  if (!LANGS.some(l => l.code === code)) return;
  try {
    await api('/auth/locale', {method: 'PUT', body: JSON.stringify({locale: code})});
  } catch (err) {
    fail(err, 'cambio lingua');
    renderProfile();
    return;
  }
  if (S.user) S.user.locale = code;
  await loadI18n(code);
  applyStaticI18n();
  renderSideUser();
  renderImpBar();
  renderSettings();
  renderProfile();
  refreshCategoryUI();
  await refreshAll(); // sidebar conti, titolo, registro (etichetta mese) e pagina corrente
  toast(t('profile.locale_saved'));
}

function renderSideUser() {
  const el = document.getElementById('sideUser');
  if (!el) return;
  if (!S.user) { el.innerHTML = ''; return; }
  el.title = t('profile.open');
  el.style.cursor = 'pointer';
  el.innerHTML = `<div class="av">${esc((S.user.username || '?').slice(0, 1).toUpperCase())}</div>
    <div style="flex:1;min-width:0">
      <div style="font-weight:600">${esc(S.user.username)}</div>
      <div style="font-size:.74rem;color:var(--sidebar-muted)">${isAdmin() ? t('set.users.admin') : t('set.users.user')}</div>
    </div>
    <button class="icon-btn" id="logoutBtn" aria-label="${esc(t('set.logout'))}" title="${esc(t('set.logout'))}"><i class="bi bi-box-arrow-right"></i></button>`;
}

/* ============================================================
   MODALI
   `bootstrap.bundle.min.js` è caricato da index.html e intercetta da sé
   `data-bs-dismiss="modal"`: aprire i modali a mano aggiungendo `.show`
   lascerebbe Bootstrap senza istanza e la sua chiusura scriverebbe un
   `display:none` inline che impedisce la riapertura. Quindi si usa la sua
   API quando c'è, con un ripiego manuale se il bundle non è disponibile.
   ============================================================ */
const hasBootstrap = () => typeof bootstrap !== 'undefined' && bootstrap.Modal;

function openModal(id) {
  const m = document.getElementById(id);
  if (!m) return;
  if (m.classList.contains('sheet-wrap')) {
    m.classList.add('is-on');
    document.body.style.overflow = 'hidden';
    return;
  }
  if (hasBootstrap()) {
    bootstrap.Modal.getOrCreateInstance(m).show();
  } else {
    m.classList.add('show');
    m.removeAttribute('aria-hidden');
    document.getElementById('scrim')?.classList.add('is-on');
    document.body.style.overflow = 'hidden';
  }
  m.querySelector('input:not([type=hidden]):not([disabled]), select, button')?.focus();
}

function closeModal(id) {
  const m = document.getElementById(id);
  if (!m) return;
  if (m.classList.contains('sheet-wrap')) { closeEntry(); return; }
  if (hasBootstrap()) {
    bootstrap.Modal.getOrCreateInstance(m).hide();
    return;
  }
  m.classList.remove('show');
  m.setAttribute('aria-hidden', 'true');
  if (!document.querySelector('.modal.show, .sheet-wrap.is-on')) {
    document.getElementById('scrim')?.classList.remove('is-on');
    document.body.style.overflow = '';
  }
}

function closeAllModals() {
  document.querySelectorAll('.modal.show').forEach(m => closeModal(m.id));
  const sheet = document.getElementById('entrySheet');
  if (sheet?.classList.contains('is-on')) closeEntry();
  if (!hasBootstrap()) {
    document.getElementById('scrim')?.classList.remove('is-on');
    document.body.style.overflow = '';
  }
}

/* ============================================================
   MASCHERA CONTO
   ============================================================ */
const ICON_SET = ['bi-bank', 'bi-piggy-bank', 'bi-credit-card', 'bi-credit-card-2-front', 'bi-cash-coin',
  'bi-wallet2', 'bi-people', 'bi-airplane', 'bi-house-door', 'bi-receipt', 'bi-gift', 'bi-mortarboard',
  'bi-heart-pulse', 'bi-car-front', 'bi-bookmark-star', 'bi-cloud'];
const TIPO_HINT = {
  corrente: 'modal.acct.hint_current',
  deposito: 'modal.acct.hint_deposit',
  carta: 'modal.acct.hint_card',
  virtuale: 'modal.acct.hint_virtual',
};
let acctEditing = null;
let acForm = {tipo: 'corrente', icon: 'bi-bank'};
let acShares = [];

function syncAcctForm() {
  document.querySelectorAll('#acTipo .chip').forEach(c => {
    c.classList.toggle('is-on', c.dataset.tipo === acForm.tipo);
    // Il tipo di conto non è modificabile via `PUT /accounts/{id}`.
    c.disabled = !!acctEditing;
  });
  const hint = document.getElementById('acTipoHint');
  if (hint) hint.textContent = t(TIPO_HINT[acForm.tipo]);
  const icons = document.getElementById('acIcons');
  if (icons) {
    icons.innerHTML = ICON_SET.map(ic =>
      `<button type="button" class="chip ${acForm.icon === ic ? 'is-on' : ''}" data-icon="${ic}" aria-label="${ic}"><i class="bi ${ic}"></i></button>`).join('');
  }
  const carta = acForm.tipo === 'carta', virt = acForm.tipo === 'virtuale', dep = acForm.tipo === 'deposito';
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('acMassBox', carta);
  show('acChiusuraBox', carta);
  show('acAddebitoBox', carta);
  show('acTassoBox', dep);
  show('acPrimaryBox', carta || virt || dep);
  show('acGoalBox', virt);
  show('acIbanBox', !(carta || virt));
  show('acBalBox', !virt);
  show('acRulesBox', !virt);
  const note = document.getElementById('acNoteBox');
  if (note) note.className = virt ? 'col-12' : 'col-sm-7';
  const lblP = document.querySelector('label[for="acPrimary"]');
  if (lblP) lblP.textContent = dep ? t('modal.acct.linked_current') : t('modal.acct.backing');
  const sel = document.getElementById('acPrimary');
  if (sel) {
    const cur = sel.value;
    sel.innerHTML = `<option value="">—</option>` + S.accounts
      .filter(x => x.kind === 'current' && x.id !== acctEditing)
      .map(x => accountOption(x)).join('');
    if (cur) sel.value = cur;
  }
}

function renderShareUI(a) {
  const box = document.getElementById('acShareBox');
  if (!box) return;
  const puoGestire = !a || isAdmin() || a.owner_id === S.user?.id;
  box.style.display = puoGestire ? '' : 'none';
  if (!puoGestire) return;
  const ownerBox = document.getElementById('acOwnerBox');
  if (ownerBox) ownerBox.style.display = isAdmin() ? '' : 'none';
  const owner = document.getElementById('acOwner');
  if (owner) {
    owner.innerHTML = S.users.map(u => `<option value="${esc(u.id)}">${esc(u.username)}</option>`).join('');
    owner.value = a ? a.owner_id : (S.user?.id || '');
    // La proprietà non è modificabile dagli endpoint attuali: campo informativo.
    owner.disabled = true;
  }
  renderShareList();
}

function renderShareList() {
  const box = document.getElementById('acShareList');
  if (!box) return;
  box.innerHTML = acShares.length ? acShares.map(sh => `
    <div class="share-row">
      <span>${esc(userName(sh.user_id))}</span>
      <select data-share-level="${esc(sh.user_id)}" class="form-select" style="height:32px;font-size:.84rem;max-width:150px">
        <option value="read"${sh.level === 'read' ? ' selected' : ''}>${t('acct.read')}</option>
        <option value="write"${sh.level === 'write' ? ' selected' : ''}>${t('acct.write')}</option>
      </select>
      <button type="button" class="rowbtn del" data-share-del="${esc(sh.user_id)}" aria-label="${esc(t('acct.unshare'))}"><i class="bi bi-x-lg"></i></button>
    </div>`).join('')
    : `<p style="font-size:.82rem;color:var(--text-muted);margin:.3rem 0 0">${t('acct.no_shares')}</p>`;
}

function renderAcctRulesList(a) {
  const box = document.getElementById('acRulesList');
  if (!box) return;
  if (!a) { box.innerHTML = `<p style="font-size:.82rem;color:var(--text-muted);margin:0">${t('acct.rules_after_save')}</p>`; return; }
  box.innerHTML = `<p style="font-size:.82rem;color:var(--text-muted);margin:0">${t('acct.rules_loading')}</p>`;
  api(`/accounts/${encodeURIComponent(a.id)}/rules`).then(rules => {
    box.innerHTML = rules.length
      ? rules.map(r => `<div class="a-meta" style="margin-bottom:.15rem">
          <b style="color:var(--text)">${esc(r.name)}</b> · ${esc(ruleDescription(r))}${r.active ? '' : ' · ' + t('rule.paused')}</div>`).join('')
      : `<p style="font-size:.82rem;color:var(--text-muted);margin:0">${t('acct.no_rules')}</p>`;
  }).catch(() => { box.innerHTML = `<p style="font-size:.82rem;color:var(--text-muted);margin:0">${t('acct.no_rules')}</p>`; });
}

async function openAcctModal(id = null) {
  acctEditing = id;
  const a = id ? acct(id) : null;
  acForm.tipo = a ? tipoOf(a) : 'corrente';
  acForm.icon = a ? iconOf(a) : 'bi-bank';
  document.getElementById('acctModalTitle').textContent = a ? t('modal.acct.edit') : t('modal.acct.title');
  const set = (elId, v) => { const el = document.getElementById(elId); if (el) el.value = v; };
  set('acName', a ? a.name : '');
  set('acNote', a?.note || '');
  set('acBal', a ? formatAmountInput(a.start_balance_cents || 0) : '');
  set('acIban', a?.iban || '');
  set('acMass', a?.limit_cents ? formatAmountInput(a.limit_cents) : '');
  set('acTasso', a?.rate_pct_bps ? formatAmountInput(a.rate_pct_bps) : '');
  set('acChiusura', a?.closing_day || a?.charge_day || 5);
  set('acAddebito', a?.charge_day || 5);
  set('acGoal', a?.goal_cents ? formatAmountInput(a.goal_cents) : '');
  set('acShareEmail', '');
  acShares = a ? (a.shares || []).map(s => ({...s})) : [];
  const del = document.getElementById('acDelete');
  del.style.display = a ? '' : 'none';
  del.dataset.armed = '';
  del.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.acct.delete')}`;
  syncAcctForm();
  if (a) set('acPrimary', a.primary_account_id || '');
  renderShareUI(a);
  renderAcctRulesList(a);
  openModal('acctModal');
}

async function saveAccount() {
  const name = document.getElementById('acName').value.trim();
  if (!name) { toast(t('modal.acct.name_required')); document.getElementById('acName').focus(); return; }
  const virt = acForm.tipo === 'virtuale', carta = acForm.tipo === 'carta', dep = acForm.tipo === 'deposito';
  const primary = document.getElementById('acPrimary').value || null;
  const body = {
    name,
    icon: acForm.icon,
    note: document.getElementById('acNote').value.trim() || null,
    iban: (carta || virt) ? null : (document.getElementById('acIban').value.trim() || null),
    limit_cents: carta ? (parseAmount(document.getElementById('acMass').value) || null) : null,
    closing_day: carta ? Math.min(28, Math.max(1, Number(document.getElementById('acChiusura').value) || 5)) : null,
    charge_day: carta ? Math.min(28, Math.max(1, Number(document.getElementById('acAddebito').value) || 5)) : null,
    primary_account_id: (carta || virt || dep) ? primary : null,
    rate_pct_bps: dep ? (Math.round(parsePct(document.getElementById('acTasso').value) * 100) || null) : null,
    goal_cents: virt ? (parseAmount(document.getElementById('acGoal').value) || null) : null,
  };
  const btn = document.getElementById('acSave');
  if (btn) btn.disabled = true;
  try {
    let saved;
    if (acctEditing) {
      saved = await api(`/accounts/${encodeURIComponent(acctEditing)}`, {method: 'PUT', body: JSON.stringify(body)});
    } else {
      saved = await api('/accounts', {
        method: 'POST',
        body: JSON.stringify({
          ...body,
          kind: TIPO_TO_KIND[acForm.tipo],
          currency: S.user?.default_currency || 'EUR',
          start_date: TODAY,
          start_balance_cents: virt ? 0 : parseAmount(document.getElementById('acBal').value),
        }),
      });
    }
    await syncSharesFor(saved?.id || acctEditing);
    closeModal('acctModal');
    await loadAccounts();
    await loadShares();
    if (isAdmin()) { await loadAdminAccounts(); renderAdminAccounts(); }
    if (saved?.kind === 'virtual') S.virtualOn.add(saved.id);
    await refreshAll();
    toast(acctEditing ? t('acct.updated', {name}) : t('acct.created', {name}));
  } catch (err) {
    fail(err, 'salvataggio conto');
  } finally {
    if (btn) btn.disabled = false;
  }
}

/* Le condivisioni si modificano nella maschera come bozza e si allineano al
   salvataggio: differenza fra la bozza e ciò che il server ha già. */
async function syncSharesFor(accountId) {
  if (!accountId) return;
  const original = (acct(accountId)?.shares || []);
  const byUser = new Map(original.map(s => [s.user_id, s.level]));
  for (const sh of acShares) {
    if (byUser.get(sh.user_id) !== sh.level) {
      const email = S.users.find(u => u.id === sh.user_id)?.email;
      if (!email) continue;
      await api(`/accounts/${encodeURIComponent(accountId)}/shares`, {
        method: 'POST', body: JSON.stringify({email, level: sh.level}),
      });
    }
    byUser.delete(sh.user_id);
  }
  for (const userId of byUser.keys()) {
    await api(`/accounts/${encodeURIComponent(accountId)}/shares/${encodeURIComponent(userId)}`, {method: 'DELETE'});
  }
}

async function addShareByEmail() {
  const input = document.getElementById('acShareEmail');
  const email = input?.value.trim();
  if (!email) return;
  if (!acctEditing) { toast(t('acct.share_after_save')); return; }
  try {
    const res = await api(`/accounts/${encodeURIComponent(acctEditing)}/shares/search?email=${encodeURIComponent(email)}`);
    if (!res || res.shared === false) {
      // `shared: false` significa "utente non condivisibile o inesistente".
      toast(t('toast.no_user_email'));
      return;
    }
    await api(`/accounts/${encodeURIComponent(acctEditing)}/shares`, {method: 'POST', body: JSON.stringify({email, level: 'read'})});
    const fresh = await api(`/accounts/${encodeURIComponent(acctEditing)}/shares`);
    acShares = fresh;
    const a = acct(acctEditing);
    if (a) a.shares = fresh;
    renderShareList();
    input.value = '';
    toast(t('toast.shared', {name: email}));
  } catch (err) { fail(err, 'condivisione'); }
}

async function deleteAccount() {
  const a = acct(acctEditing);
  if (!a) return;
  const btn = document.getElementById('acDelete');
  if (btn.dataset.armed !== '1') {
    btn.dataset.armed = '1';
    btn.innerHTML = `<i class="bi bi-exclamation-triangle"></i> ${t('modal.acct.confirm_delete')}`;
    setTimeout(() => {
      btn.dataset.armed = '';
      btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.acct.delete')}`;
    }, 5000);
    return;
  }
  btn.dataset.armed = '';
  btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.acct.delete')}`;
  try {
    await api(`/accounts/${encodeURIComponent(a.id)}`, {method: 'DELETE'});
    closeModal('acctModal');
    S.virtualOn.delete(a.id);
    if (S.accountId === a.id) S.accountId = 'all';
    await loadAccounts();
    await loadShares();
    if (isAdmin()) { await loadAdminAccounts(); renderAdminAccounts(); }
    await refreshAll();
    toast(t('acct.deleted', {name: a.name}));
  } catch (err) { fail(err, 'eliminazione conto'); }
}

/* ============================================================
   REGOLE DI SPESA
   ============================================================ */
const CALC_TO_WIRE = {fisso: 'fixed', pct_giacenza: 'pct_avg_balance', pct_saldo: 'pct_end_balance'};
const WIRE_TO_CALC = {fixed: 'fisso', pct_avg_balance: 'pct_giacenza', pct_end_balance: 'pct_saldo'};
const CADENCE_TO_WIRE = {mensile: 'monthly', bimestrale: 'bimonthly', trimestrale: 'quarterly', semestrale: 'semiannual', annuale: 'annual'};
const WIRE_TO_CADENCE = {monthly: 'mensile', bimonthly: 'bimestrale', quarterly: 'trimestrale', semiannual: 'semestrale', annual: 'annuale'};
const RULE_BASI = {
  always: {label: 'rule.base.always', money: false},
  avg_balance: {label: 'rule.base.avg_balance', money: true},
  end_balance: {label: 'rule.base.end_balance', money: true},
  total_out: {label: 'rule.base.total_out', money: true},
  total_in: {label: 'rule.base.total_in', money: true},
  tx_count: {label: 'rule.base.tx_count', money: false},
};
const PRESETS = {
  bollo: {name: 'Imposta di bollo', calc_type: 'fixed', value: 3420, is_annual: true, prorata_days: true, cadence: 'quarterly',
    charge_day: 0, cond_base: 'avg_balance', cond_op: '>', cond_threshold: 500000, active: true},
  bolloPct: {name: 'Imposta di bollo', calc_type: 'pct_avg_balance', value: 20, is_annual: true, cadence: 'quarterly',
    charge_day: 0, cond_base: 'always', active: true},
  canone: {name: 'Canone conto', calc_type: 'fixed', value: 200, is_annual: false, cadence: 'monthly',
    charge_day: 1, cond_base: 'always', active: true},
};

let rulesAcctId = null, ruleEditing = null, acRules = [];

function ruleDescription(r) {
  const calc = r.calc_type === 'fixed' ? money(r.value) : `${nf.format(r.value / 100)}%`;
  const cad = t(`rule.cadence.${r.cadence}`);
  const day = r.charge_day === 0 ? t('rule.last_day') : t('rule.on_day', {day: r.charge_day});
  const cond = !r.cond_base || r.cond_base === 'always'
    ? t('rule.base.always').toLowerCase()
    : `${t(RULE_BASI[r.cond_base]?.label || 'rule.base.always').toLowerCase()} ${r.cond_op || '>'} ${RULE_BASI[r.cond_base]?.money ? money(r.cond_threshold || 0) : (r.cond_threshold || 0)}`;
  return `${calc}${r.is_annual ? ' ' + t(r.prorata_days ? 'rule.prorata_short' : 'rule.annual_short') : ''} · ${cad} · ${day} · ${cond}`;
}

async function openRulesModal(id) {
  rulesAcctId = id;
  ruleEditing = null;
  const el = document.getElementById('ruAcctName');
  if (el) el.textContent = '· ' + (acct(id)?.name || '');
  showRulesList();
  openModal('rulesModal');
  await reloadRules();
}

async function reloadRules() {
  try {
    acRules = await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules`);
  } catch (err) {
    acRules = [];
    fail(err, 'regole');
  }
  renderRulesList();
}

function showRulesList() {
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('ruListPanel', true);
  show('ruEditPanel', false);
  show('ruSave', false);
  show('ruCancel', false);
  show('ruDelete', false);
  show('ruClose', true);
  renderRulesList();
}

function renderRulesList() {
  const box = document.getElementById('ruList');
  if (!box) return;
  box.innerHTML = acRules.length ? acRules.map(r => `
    <div class="rule-row ${r.active ? '' : 'off'}">
      <div class="rn">
        <b>${esc(r.name)}</b>
        <div class="rd">${esc(ruleDescription(r))}</div>
        <div class="rnext" data-rule-next="${esc(r.id)}"></div>
      </div>
      <div class="racts">
        <button class="rowbtn" data-rule-toggle="${esc(r.id)}" title="${esc(r.active ? t('rule.suspend') : t('rule.resume'))}">
          <i class="bi ${r.active ? 'bi-pause' : 'bi-play'}"></i></button>
        <button class="rowbtn" data-rule-gen="${esc(r.id)}" title="${esc(t('rule.generate'))}"><i class="bi bi-lightning"></i></button>
        <button class="rowbtn" data-rule-edit="${esc(r.id)}" aria-label="${esc(t('reg.action.edit'))}"><i class="bi bi-pencil"></i></button>
        <button class="rowbtn del" data-rule-del="${esc(r.id)}" aria-label="${esc(t('reg.action.delete'))}"><i class="bi bi-trash3"></i></button>
      </div></div>`).join('')
    : `<div class="empty" style="padding:1.4rem"><i class="bi bi-percent"></i>
        <h6>${t('rule.empty')}</h6><p>${t('rule.empty_hint')}</p></div>`;
  fillRuleNext();
}

/* L'esito della prossima scadenza è un calcolo del backend: lo chiedo con
   `POST /accounts/{id}/rules/preview`, che è l'endpoint dedicato. */
async function fillRuleNext() {
  if (!acRules.length) return;
  const from = TODAY;
  const to = addDays(TODAY, 400);
  await Promise.all(acRules.map(async r => {
    const cell = document.querySelector(`[data-rule-next="${r.id}"]`);
    if (!cell) return;
    try {
      const res = await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/preview`, {
        method: 'POST', body: JSON.stringify({from, to, rule_id: r.id}),
      });
      const rows = (Array.isArray(res) ? res : [res]).flatMap(x => x?.rows || []);
      cell.innerHTML = rows.length
        ? `<span class="ru-ok">${money(rows[0].cents)}</span> ${t('rule.on', {date: fmtDate(rows[0].date)})}`
        : `<span class="ru-no">${t('rule.not_due')}</span>`;
    } catch {
      cell.textContent = '';
    }
  }));
}

function openRuleEditor(rule) {
  ruleEditing = rule && rule.id ? rule.id : null;
  const r = rule || {name: '', calc_type: 'fixed', value: 0, is_annual: false, prorata_days: false, annual_cap_cents: null,
    cadence: 'quarterly', charge_day: 0, charge_month: 12, cond_base: 'always', cond_op: '>', cond_threshold: null, active: true};
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('ruListPanel', false);
  show('ruEditPanel', true);
  show('ruSave', true);
  show('ruCancel', true);
  show('ruClose', false);
  show('ruDelete', !!ruleEditing);

  document.getElementById('ruCat').innerHTML = S.categories
    .map(c => `<option value="${esc(c.id)}">${esc(c.emoji)} ${esc(c.name)}</option>`).join('');
  document.getElementById('ruBase').innerHTML = Object.entries(RULE_BASI)
    .map(([k, v]) => `<option value="${k}">${t(v.label)}</option>`).join('');
  document.getElementById('ruMese').innerHTML = MONTHS().map((m, i) => `<option value="${i + 1}">${m}</option>`).join('');
  document.getElementById('ruGiorno').innerHTML = `<option value="0">${t('rule.last_day')}</option>`
    + Array.from({length: 28}, (_, i) => `<option value="${i + 1}">${t('rule.on_day', {day: i + 1})}</option>`).join('');

  const set = (id, v) => { const el = document.getElementById(id); if (el) el.value = v; };
  set('ruNome', r.name);
  set('ruCat', r.category_id || S.categories.find(c => c.kind === 'out')?.id || '');
  set('ruTipo', WIRE_TO_CALC[r.calc_type] || 'fisso');
  set('ruValore', r.value ? formatAmountInput(r.value) : '');
  set('ruCap', r.annual_cap_cents ? formatAmountInput(r.annual_cap_cents) : '');
  document.getElementById('ruAnnuale').checked = !!r.is_annual;
  document.getElementById('ruProrata').checked = !!r.prorata_days;
  set('ruCadenza', WIRE_TO_CADENCE[r.cadence] || 'trimestrale');
  set('ruMese', String(r.charge_month || 12));
  set('ruGiorno', String(r.charge_day ?? 0));
  set('ruBase', r.cond_base || 'always');
  set('ruOp', r.cond_op || '>');
  set('ruSoglia', r.cond_threshold != null
    ? (RULE_BASI[r.cond_base]?.money ? formatAmountInput(r.cond_threshold) : r.cond_threshold) : '');
  document.getElementById('ruAttiva').checked = r.active !== false;
  syncRuleForm();
}

function readRuleForm() {
  const calc = CALC_TO_WIRE[document.getElementById('ruTipo').value] || 'fixed';
  const base = document.getElementById('ruBase').value;
  const rawVal = document.getElementById('ruValore').value;
  // `value` è in centesimi per l'importo fisso e in punti base per le percentuali.
  const value = calc === 'fixed' ? parseAmount(rawVal) : Math.round(parsePct(rawVal) * 100);
  const rawSoglia = document.getElementById('ruSoglia').value;
  const cadence = CADENCE_TO_WIRE[document.getElementById('ruCadenza').value] || 'quarterly';
  return {
    name: document.getElementById('ruNome').value.trim() || t('rule.default_name'),
    category_id: document.getElementById('ruCat').value,
    calc_type: calc,
    value,
    is_annual: document.getElementById('ruAnnuale').checked,
    prorata_days: prorataApplies() && document.getElementById('ruProrata').checked,
    annual_cap_cents: parseAmount(document.getElementById('ruCap').value) || null,
    cadence,
    charge_day: Number(document.getElementById('ruGiorno').value) || 0,
    charge_month: cadence === 'annual' ? Number(document.getElementById('ruMese').value) || 12 : null,
    cond_base: base,
    cond_op: base === 'always' ? null : document.getElementById('ruOp').value,
    cond_threshold: base === 'always' ? null
      : (RULE_BASI[base]?.money ? parseAmount(rawSoglia) : (parseInt(rawSoglia, 10) || 0)),
    active: document.getElementById('ruAttiva').checked,
  };
}

// The per-day option only makes sense for a fixed annual amount on a sub-annual cadence.
function prorataApplies() {
  return document.getElementById('ruAnnuale').checked
    && document.getElementById('ruTipo').value === 'fisso'
    && document.getElementById('ruCadenza').value !== 'annuale';
}

function syncRuleForm() {
  const tipo = document.getElementById('ruTipo').value;
  const base = document.getElementById('ruBase').value;
  document.getElementById('ruValoreLbl').textContent = tipo === 'fisso' ? t('rule.amount_eur') : t('rule.pct');
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('ruMeseBox', document.getElementById('ruCadenza').value === 'annuale');
  show('ruProrataBox', prorataApplies());
  show('ruOpBox', base !== 'always');
  show('ruSogliaBox', base !== 'always');
  document.getElementById('ruSogliaLbl').textContent = RULE_BASI[base]?.money ? t('rule.threshold_eur') : t('rule.threshold_n');
  updateRulePreview();
}

let previewTimer = null;
function updateRulePreview() {
  const box = document.getElementById('ruPreview');
  if (!box) return;
  const rule = readRuleForm();
  if (!rule.value) { box.innerHTML = `<span class="ru-no">${t('rule.preview_need_amount')}</span>`; return; }
  clearTimeout(previewTimer);
  box.innerHTML = `<span class="ru-no">${t('rule.preview_loading')}</span>`;
  /* Debounce: l'anteprima è una chiamata al server e i campi sono di testo.
     Senza questo, ogni tasto sarebbe una richiesta. */
  previewTimer = setTimeout(async () => {
    try {
      /* Con una regola non ancora salvata non c'è nulla da anteprimare lato
         server: lo dico invece di mostrare numeri inventati qui. */
      if (!ruleEditing) {
        box.innerHTML = `<span class="ru-no">${t('rule.preview_after_save')}</span>`;
        return;
      }
      const res = await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/preview`, {
        method: 'POST',
        body: JSON.stringify({from: TODAY, to: addDays(TODAY, 400), rule_id: ruleEditing}),
      });
      const rows = (Array.isArray(res) ? res : [res]).flatMap(x => x?.rows || []).slice(0, 3);
      box.innerHTML = rows.length
        ? rows.map(r => `<div style="display:flex;gap:.6rem;justify-content:space-between;padding:.15rem 0">
            <span>${fmtDate(r.date)}</span><span class="ru-ok">${money(r.cents)}</span></div>`).join('')
        : `<span class="ru-no">${t('rule.preview_none')}</span>`;
    } catch (err) {
      box.innerHTML = `<span class="ru-no">${esc(apiErrorToMessage(err))}</span>`;
    }
  }, 400);
}

async function saveRule() {
  const body = readRuleForm();
  if (!body.value) { toast(t('modal.rule.amount_required')); return; }
  const btn = document.getElementById('ruSave');
  if (btn) btn.disabled = true;
  try {
    if (ruleEditing) {
      await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/${encodeURIComponent(ruleEditing)}`, {method: 'PUT', body: JSON.stringify(body)});
    } else {
      await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules`, {method: 'POST', body: JSON.stringify(body)});
    }
    await reloadRules();
    showRulesList();
    renderAcctRulesList(acct(rulesAcctId));
    if (S.page === 'conti') renderAccountsPage();
    toast(t('modal.rule.saved', {name: body.name}));
  } catch (err) {
    fail(err, 'salvataggio regola');
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function deleteRule(id) {
  const r = acRules.find(x => x.id === id);
  try {
    await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/${encodeURIComponent(id)}`, {method: 'DELETE'});
    await reloadRules();
    showRulesList();
    renderAcctRulesList(acct(rulesAcctId));
    toast(t('modal.rule.deleted', {name: r?.name || ''}));
  } catch (err) { fail(err, 'eliminazione regola'); }
}

async function toggleRule(id) {
  const r = acRules.find(x => x.id === id);
  if (!r) return;
  try {
    await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/${encodeURIComponent(id)}`, {
      method: 'PUT', body: JSON.stringify({active: !r.active}),
    });
    await reloadRules();
    renderAcctRulesList(acct(rulesAcctId));
    if (S.page === 'conti') renderAccountsPage();
  } catch (err) { fail(err, 'sospensione regola'); }
}

async function generateRule(id) {
  const r = acRules.find(x => x.id === id);
  try {
    const res = await api(`/accounts/${encodeURIComponent(rulesAcctId)}/rules/${encodeURIComponent(id)}/generate`, {
      method: 'POST', body: JSON.stringify({}),
    });
    const n = Array.isArray(res) ? res.length : (res?.rows?.length ?? 0);
    await refreshAll();
    toast(t('rule.generated', {name: r?.name || '', n}));
  } catch (err) { fail(err, 'generazione regola'); }
}

/* ============================================================
   IMPORT CSV
   La lettura del file resta nel browser solo per la mappatura delle
   colonne; il riconoscimento dei duplicati e l'inserimento sono del
   backend (`/import/preview` e `/import/commit`).
   ============================================================ */
let impAcctId = null, impRows = [], impCols = [], impText = '', impSep = ';';

function splitCSV(text) {
  const lines = text.split(/\r?\n/).filter(l => l.trim() !== '');
  if (!lines.length) return {rows: [], sep: ';'};
  const sep = (lines[0].split(';').length > lines[0].split(',').length) ? ';' : ',';
  const rows = lines.map(l => {
    const out = [];
    let cur = '', q = false;
    for (let i = 0; i < l.length; i++) {
      const ch = l[i];
      if (ch === '"') { q = !q; continue; }
      if (ch === sep && !q) { out.push(cur.trim()); cur = ''; continue; }
      cur += ch;
    }
    out.push(cur.trim());
    return out;
  });
  return {rows, sep};
}

const looksLikeDate = v => /^\d{1,4}[/\-.]\d{1,2}[/\-.]\d{2,4}$/.test((v || '').trim());

function openImport(id) {
  impAcctId = id;
  impRows = []; impCols = []; impText = '';
  const el = document.getElementById('impAcctName');
  if (el) el.textContent = '· ' + (acct(id)?.name || '');
  const show = (i, on) => { const e = document.getElementById(i); if (e) e.style.display = on ? '' : 'none'; };
  show('impStep1', true);
  show('impStep2', false);
  show('impConfirm', false);
  show('impBack', false);
  /* Nel mockup "prova con un estratto conto di esempio" carica righe finte.
     Qui il commit scriverebbe quelle righe sul conto vero: il controllo resta
     nascosto invece di offrire un'importazione di dati inventati. */
  show('impDemo', false);
  const f = document.getElementById('impFile');
  if (f) f.value = '';
  document.getElementById('impStats').innerHTML = '';
  document.getElementById('impPreview').innerHTML = '';
  /* `ImportRequest` non trasporta né `skip_dup` né l'assegnazione automatica
     della categoria: la prima vive nel profilo di import lato server, la
     seconda non è implementata. Meglio disattivarli che simularli. */
  ['impSkipDup', 'impAutoCat'].forEach(elId => {
    const el = document.getElementById(elId);
    if (!el) return;
    el.disabled = true;
    el.title = t('modal.imp.server_side');
  });
  openModal('importModal');
}

function loadCSV(text) {
  const {rows, sep} = splitCSV(text);
  if (rows.length < 2) { toast(t('modal.imp.no_rows')); return; }
  impText = text;
  impSep = sep;
  const hasHeader = !rows[0].some(looksLikeDate);
  impCols = hasHeader ? rows[0] : rows[0].map((_, i) => t('modal.imp.column', {n: i + 1}));
  impRows = hasHeader ? rows.slice(1) : rows;

  const opts = extra => `${extra ? `<option value="-1">— ${t('common.all').toLowerCase()} —</option>` : ''}`
    + impCols.map((c, i) => `<option value="${i}">${esc(c)}</option>`).join('');
  ['impColDate', 'impColDesc', 'impColAmount'].forEach(id => { document.getElementById(id).innerHTML = opts(false); });
  const amt2 = document.getElementById('impColAmount2');
  amt2.innerHTML = opts(true);
  amt2.disabled = true;
  amt2.title = t('modal.imp.amount2_unsupported');

  const firstRow = impRows[0] || [];
  const isNum = v => /^[-+]?[\d.,\s]+$/.test((v || '').trim()) && /\d/.test(v || '');
  const named = (...w) => impCols.map((c, i) => [String(c).toLowerCase(), i])
    .filter(([c]) => w.some(x => c.includes(x))).map(([, i]) => i);
  const firstOf = (list, test) => list.find(i => test(firstRow[i]));
  const dateCols = impCols.map((_, i) => i).filter(i => looksLikeDate(firstRow[i]));
  const dateIdx = firstOf(named('data operazione', 'data contabile', 'data'), looksLikeDate) ?? dateCols[0] ?? 0;
  const descIdx = firstOf(named('descr', 'causal', 'memo', 'dettagl', 'beneficiar'), v => !isNum(v) && !looksLikeDate(v))
    ?? impCols.map((_, i) => i).find(i => i !== dateIdx && !isNum(firstRow[i]) && !looksLikeDate(firstRow[i]))
    ?? Math.min(2, impCols.length - 1);
  const numCols = impCols.map((_, i) => i).filter(i => i !== dateIdx && i !== descIdx && isNum(firstRow[i]));
  const amtIdx = firstOf(named('importo', 'amount', 'uscite', 'addebit', 'dare'), isNum)
    ?? numCols[numCols.length - 1] ?? impCols.length - 1;
  const amt2Idx = firstOf(named('entrate', 'accredit', 'avere'), isNum) ?? -1;

  document.getElementById('impColDate').value = String(Math.max(0, dateIdx));
  document.getElementById('impColDesc').value = String(descIdx >= 0 ? descIdx : Math.min(2, impCols.length - 1));
  document.getElementById('impColAmount').value = String(amtIdx >= 0 ? amtIdx : impCols.length - 1);
  document.getElementById('impColAmount2').value = String(amt2Idx >= 0 && amt2Idx !== amtIdx ? amt2Idx : -1);
  document.getElementById('impDateFmt').value = impRows.some(r => (r[dateIdx] || '').includes('-')) ? 'ymd' : 'dmy';

  const show = (i, on) => { const e = document.getElementById(i); if (e) e.style.display = on ? '' : 'none'; };
  show('impStep1', false);
  show('impStep2', true);
  show('impConfirm', true);
  show('impBack', true);
  buildImportPreview();
}

function importBody() {
  const di = document.getElementById('impColDate').value;
  const si = document.getElementById('impColDesc').value;
  const ai = document.getElementById('impColAmount').value;
  /* `ColumnRole` lato backend conosce solo date / amount / description / other:
     una colonna "entrate" separata non è mappabile, quindi il campo resta
     disabilitato invece di far credere che venga usato. */
  const colMap = {[di]: 'date', [si]: 'description', [ai]: 'amount'};
  return {
    account_id: impAcctId,
    bank_name: acct(impAcctId)?.name || 'import',
    csv_content: impText,
    delimiter: impSep,
    date_format: document.getElementById('impDateFmt').value,
    decimal_sep: document.getElementById('impDec').value,
    col_map: colMap,
  };
}

async function buildImportPreview() {
  const stats = document.getElementById('impStats');
  const prev = document.getElementById('impPreview');
  const confirm = document.getElementById('impConfirm');
  if (!stats || !prev) return;
  stats.innerHTML = `<span>${t('modal.imp.checking')}</span>`;
  prev.innerHTML = '';
  if (confirm) confirm.disabled = true;
  try {
    const res = await api('/import/preview', {method: 'POST', body: JSON.stringify(importBody())});
    const nuovi = res.to_import;
    const dup = res.duplicates.length + res.already_imported.length;
    let ent = 0, usc = 0;
    nuovi.forEach(r => (r.cents > 0 ? ent += r.cents : usc -= r.cents));
    stats.innerHTML = `<span><b>${nuovi.length}</b> ${t('modal.imp.to_import')}</span>
      <span>${dup} ${t('modal.imp.already')}</span>
      <span>${t('kpi.income')} <b class="pos">${money(ent)}</b></span>
      <span>${t('kpi.expense')} <b class="neg">${money(usc)}</b></span>
      ${res.skipped ? `<span>${res.skipped} ${t('modal.imp.skipped')}</span>` : ''}`;
    const all = [...nuovi.map(r => ({...r, dup: false})),
      ...res.duplicates.map(r => ({...r, dup: true})),
      ...res.already_imported.map(r => ({...r, dup: true}))]
      .sort((a, b) => (a.date < b.date ? -1 : 1));
    prev.innerHTML = `<thead><tr><th>${t('common.date')}</th><th>${t('common.description')}</th>
        <th style="text-align:right">${t('common.amount')}</th></tr></thead><tbody>`
      + all.slice(0, 60).map(r => `<tr class="${r.dup ? 'dup' : ''}">
          <td>${fmtDate(r.date)}</td><td>${esc(r.description || '')}</td>
          <td style="text-align:right" class="${r.cents > 0 ? 'pos' : 'neg'}">${money(r.cents, true)}</td></tr>`).join('')
      + `</tbody>`;
    if (confirm) {
      confirm.disabled = nuovi.length === 0;
      confirm.textContent = t('modal.imp.confirm_n', {n: nuovi.length});
    }
  } catch (err) {
    stats.innerHTML = `<span>${esc(apiErrorToMessage(err))}</span>`;
    if (confirm) confirm.disabled = true;
    fail(err, 'anteprima import');
  }
}

async function runImport() {
  const confirm = document.getElementById('impConfirm');
  if (confirm) confirm.disabled = true;
  try {
    const res = await api('/import/commit', {method: 'POST', body: JSON.stringify(importBody())});
    if (!res.inserted) { toast(t('modal.imp.nothing_new')); return; }
    closeModal('importModal');
    await setAccount(impAcctId);
    goPage('registro');
    await refreshAll();
    toast(t('modal.imp.done', {count: res.inserted, account: acct(impAcctId)?.name || ''}));
  } catch (err) {
    fail(err, 'import');
  } finally {
    if (confirm) confirm.disabled = false;
  }
}

/* ============================================================
   RICORRENZA (modale avanzata)
   ============================================================ */
let recEditing = null, recMode = 'fixed';

function patternFromForm() {
  const p = document.getElementById('rPattern').value;
  const skip = Math.max(0, Number(document.getElementById('rSkip').value) || 0);
  const interval = 1 + skip;
  const first = document.getElementById('rFirst').value || TODAY;
  const d = fromISO(first);
  if (p === 'daily') return `FREQ=DAILY;INTERVAL=${interval}`;
  if (p === 'weekly') return `FREQ=WEEKLY;INTERVAL=${interval}`;
  if (p === 'yearly') return `FREQ=YEARLY;INTERVAL=${interval}`;
  if (p === 'monthly-last') return `FREQ=MONTHLY;INTERVAL=${interval};BYMONTHDAY=-1`;
  if (p === 'monthly-weekday') {
    const nth = Math.ceil(d.getDate() / 7);
    const days = ['SU', 'MO', 'TU', 'WE', 'TH', 'FR', 'SA'];
    return `FREQ=MONTHLY;INTERVAL=${interval};BYDAY=${nth}${days[d.getDay()]}`;
  }
  return `FREQ=MONTHLY;INTERVAL=${interval};BYMONTHDAY=${d.getDate()}`;
}

function patternToForm(pattern) {
  const s = String(pattern || '').toUpperCase();
  const m = s.match(/INTERVAL=(\d+)/);
  const interval = m ? Number(m[1]) : 1;
  let kind = 'monthly-day';
  if (s.includes('FREQ=DAILY')) kind = 'daily';
  else if (s.includes('FREQ=WEEKLY')) kind = 'weekly';
  else if (s.includes('FREQ=YEARLY')) kind = 'yearly';
  else if (s.includes('BYMONTHDAY=-1')) kind = 'monthly-last';
  else if (s.includes('BYDAY')) kind = 'monthly-weekday';
  return {kind, skip: Math.max(0, interval - 1)};
}

/* Dal 29 in poi la data manca in alcuni mesi: la ricorrenza li salta. Avviso l'utente e indico
   la regola "ultimo giorno del mese". */
function syncPatternHint() {
  const el = document.getElementById('rPatternHint');
  if (!el) return;
  const first = document.getElementById('rFirst')?.value;
  const day = first ? fromISO(first).getDate() : 0;
  const kind = document.getElementById('rPattern')?.value;
  el.style.display = kind === 'monthly-day' && day >= 29 ? '' : 'none';
}

function openRecModal(id = null) {
  recEditing = id;
  document.getElementById('rFormRows').style.display = '';
  document.getElementById('rDatePanel').style.display = 'none';
  document.getElementById('rFooterMain').style.display = '';
  document.getElementById('rFooterDate').style.display = 'none';
  const r = id ? S.recurrences.find(x => x.id === id) : null;
  recMode = r?.mode === 'avg' ? 'avg' : 'fixed';
  const set = (elId, v) => { const el = document.getElementById(elId); if (el) el.value = v; };
  set('rDesc', r ? r.description : '');
  set('rAmount', r ? formatAmountInput(Math.abs(r.amount_cents)) : '');
  set('rAvgN', r?.avg_n || 6);
  document.getElementById('rActive').checked = r ? r.active : true;
  const tipo = r ? (r.amount_cents > 0 ? 'entrata' : 'uscita') : 'uscita';
  document.querySelectorAll('#rTipo button').forEach(b => b.classList.toggle('is-on', b.dataset.tipo === tipo));

  document.getElementById('rAcct').innerHTML = writableAccounts()
    .filter(a => a.kind !== 'virtual')
    .map(a => accountOption(a)).join('');
  if (r) {
    set('rAcct', r.account_id);
    set('rWeekend', r.weekend || 'create');
    set('rFirst', r.first_date);
    const pf = patternToForm(r.pattern);
    set('rPattern', pf.kind);
    set('rSkip', pf.skip);
    set('rEndType', r.end_type || 'never');
    set('rEndDate', r.end_date || '');
    set('rEndCount', r.end_count || 12);
  } else {
    set('rWeekend', 'create');
    set('rFirst', TODAY);
    set('rPattern', 'monthly-day');
    set('rSkip', 0);
    set('rEndType', 'never');
    set('rEndDate', '');
    set('rEndCount', 12);
    if (S.accountId !== 'all') set('rAcct', S.accountId);
  }
  syncRecCategories(tipo);
  if (r) set('rCat', r.category_id);
  const isTransfer = !!(r?.linked_account_id);
  document.getElementById('rIsTransfer').checked = isTransfer;
  syncRecTransfer();
  if (r?.linked_account_id) set('rLinkedAcct', r.linked_account_id);
  if (r?.linked_category_id) set('rLinkedCat', r.linked_category_id);
  document.getElementById('rApplyExisting').checked = false;
  document.getElementById('rOverwriteManual').checked = false;
  set('rApplyFrom', '');
  syncRecEnd();
  syncRecMode();
  syncRecApply();
  syncPatternHint();
  updateRecPreview();
  openModal('recModal');
}

function syncRecEnd() {
  const v = document.getElementById('rEndType').value;
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('rEndDateBox', v === 'until');
  show('rEndCountBox', v === 'count');
}

/* Il riallineamento dei movimenti già registrati ha senso solo modificando
   una regola a importo fisso (il server rifiuta le regole "media"). */
function syncRecApply() {
  const show = (id, on) => { const el = document.getElementById(id); if (el) el.style.display = on ? '' : 'none'; };
  show('rApplyBox', !!recEditing && recMode === 'fixed');
  show('rApplyOpts', !!document.getElementById('rApplyExisting')?.checked);
}

function syncRecMode() {
  document.querySelectorAll('#rMode .chip').forEach(b => b.classList.toggle('is-on', b.dataset.mode === recMode));
  const box = document.getElementById('rAvgBox');
  if (box) box.style.display = recMode === 'avg' ? '' : 'none';
  const lbl = document.getElementById('rAmountLabel');
  if (lbl) lbl.textContent = recMode === 'avg' ? t('modal.rec.start_amount') : t('modal.rec.amount');
  const hint = document.getElementById('rAvgHint');
  if (hint && recMode === 'avg') hint.textContent = t('modal.rec.avg_hint');
  syncRecApply();
}

function syncRecCategories(tipoOverride) {
  const tipo = tipoOverride || (document.querySelector('#rTipo button.is-on')?.dataset.tipo || 'uscita');
  const srcKind = tipo === 'entrata' ? 'in' : 'out';
  const linkedKind = srcKind === 'in' ? 'out' : 'in';
  const toOpts = cats => cats.map(c => `<option value="${esc(c.id)}">${esc(c.emoji)} ${esc(c.name)}</option>`).join('');
  const catEl = document.getElementById('rCat');
  if (catEl) catEl.innerHTML = toOpts(S.categories.filter(c => c.kind === srcKind));
  const linkedCatEl = document.getElementById('rLinkedCat');
  if (linkedCatEl) linkedCatEl.innerHTML = toOpts(S.categories.filter(c => c.kind === linkedKind));
}

function syncRecTransfer() {
  const on = document.getElementById('rIsTransfer')?.checked;
  const box = document.getElementById('rTransferBox');
  if (box) box.style.display = on ? '' : 'none';
  if (!on) return;
  const srcId = document.getElementById('rAcct')?.value;
  const linked = document.getElementById('rLinkedAcct');
  if (!linked) return;
  const prev = linked.value;
  linked.innerHTML = writableAccounts()
    .filter(a => a.kind !== 'virtual' && a.id !== srcId)
    .map(a => accountOption(a)).join('');
  if (prev && linked.querySelector(`option[value="${CSS.escape(prev)}"]`)) linked.value = prev;
}

/* L'anteprima delle date viene dal backend quando la regola esiste: è lo
   stesso motore che genererà le righe previste. */
async function updateRecPreview() {
  const box = document.getElementById('rPreview');
  if (!box) return;
  if (!recEditing) { box.textContent = t('modal.rec.preview_after_save'); return; }
  box.textContent = t('rule.preview_loading');
  try {
    const first = document.getElementById('rFirst').value || TODAY;
    const occ = await api(`/recurrences/${encodeURIComponent(recEditing)}/occurrences?from=${first}&to=${addDays(first, 200)}`);
    const list = occ.slice(0, 4);
    box.innerHTML = list.length
      ? list.map(o => `${fmtDayLong(o.date)}${isWeekend(o.date) ? ` <span style="color:var(--text-muted)">(${t('modal.rec.weekend')})</span>` : ''}`).join(' · ')
      : t('modal.rec.preview_none');
  } catch (err) {
    box.textContent = apiErrorToMessage(err);
  }
}

/* Mostra nel modale della ricorrenza il pannello di scelta; risolve con i campi da
   aggiungere alla richiesta, oppure `null` se l'utente torna indietro. Se non c'è
   nulla da decidere risolve con `{}` senza mostrare niente. */
async function askRecDateChoice(orig, body) {
  let impact;
  try {
    impact = await api(`/recurrences/${encodeURIComponent(orig.id)}/date-impact`, {method: 'POST', body: JSON.stringify(body)});
  } catch (err) { fail(err, 'anteprima date ricorrenza'); return null; }
  const manual = impact.stale_manual, auto = impact.stale_auto;
  if (!impact.to_create && !auto && !manual) return {};

  const diff = orig ? Math.round((fromISO(body.first_date) - fromISO(orig.first_date)) / 864e5) : 0;
  const summary = diff < 0 ? t('modal.rec.date.earlier', {n: -diff, from: fmtDate(orig.first_date), to: fmtDate(body.first_date)})
    : diff > 0 ? t('modal.rec.date.later', {n: diff, from: fmtDate(orig.first_date), to: fmtDate(body.first_date)})
    : t('modal.rec.date.pattern');
  const radio = (name, value, label, checked) => `<div class="form-check">
      <input class="form-check-input" type="radio" name="${name}" id="${name}_${value}" value="${value}" ${checked ? 'checked' : ''}>
      <label class="form-check-label" for="${name}_${value}">${label}</label></div>`;
  let groups = '';
  if (impact.to_create) {
    groups += `<div class="mb-3"><div class="fw-semibold mb-1">${t('modal.rec.date.missing', {n: impact.to_create})}</div>
      ${radio('rdCreate', 'yes', t('modal.rec.date.create', {n: impact.to_create}), true)}
      ${radio('rdCreate', 'no', t('modal.rec.date.nocreate'), false)}</div>`;
  }
  if (auto || manual) {
    groups += `<div class="mb-3"><div class="fw-semibold mb-1">${t('modal.rec.date.stale', {n: auto + manual, manual})}</div>
      ${radio('rdStale', 'keep', t('modal.rec.date.keep'), true)}
      ${auto ? radio('rdStale', 'delete_auto', t('modal.rec.date.del_auto', {n: auto}), false) : ''}
      ${manual ? radio('rdStale', 'delete_all', t('modal.rec.date.del_all', {n: auto + manual}), false) : ''}</div>`;
  }
  document.getElementById('rDateSummary').textContent = summary;
  document.getElementById('rDateGroups').innerHTML = groups;
  const toggle = on => {
    document.getElementById('rFormRows').style.display = on ? 'none' : '';
    document.getElementById('rDatePanel').style.display = on ? '' : 'none';
    document.getElementById('rFooterMain').style.display = on ? 'none' : '';
    document.getElementById('rFooterDate').style.display = on ? '' : 'none';
  };
  toggle(true);
  return new Promise(resolve => {
    const done = v => {
      toggle(false);
      document.getElementById('rDateBack').onclick = null;
      document.getElementById('rDateConfirm').onclick = null;
      resolve(v);
    };
    document.getElementById('rDateBack').onclick = () => done(null);
    document.getElementById('rDateConfirm').onclick = () => {
      const val = n => document.querySelector(`input[name="${n}"]:checked`)?.value;
      done({
        create_missing: val('rdCreate') === 'yes',
        stale_action: val('rdStale') || 'keep',
      });
    };
  });
}

async function saveRecurrence() {
  const desc = document.getElementById('rDesc').value.trim() || t('modal.rec.default_name');
  const cents = parseAmount(document.getElementById('rAmount').value);
  if (!cents) { toast(t('toast.amount_required')); return; }
  const tipo = document.querySelector('#rTipo button.is-on')?.dataset.tipo || 'uscita';
  const accountId = document.getElementById('rAcct').value;
  if (!accountId) { toast(t('toast.no_writable')); return; }
  const endType = document.getElementById('rEndType').value;
  /* `amount_cents` della ricorrenza è firmato, come nel dominio: il segno lo
     decide il selettore uscita/entrata, non il valore digitato. */
  const isTransfer = document.getElementById('rIsTransfer')?.checked;
  const linkedAcctId = isTransfer ? (document.getElementById('rLinkedAcct')?.value || null) : null;
  if (isTransfer && (!linkedAcctId || linkedAcctId === accountId)) {
    toast(t('toast.transfer_acct_required'));
    return;
  }
  const body = {
    description: desc,
    category_id: document.getElementById('rCat').value,
    kind: tipo === 'entrata' ? 'in' : 'out',
    amount_cents: tipo === 'entrata' ? cents : -cents,
    account_id: accountId,
    pattern: patternFromForm(),
    weekend: document.getElementById('rWeekend').value,
    first_date: document.getElementById('rFirst').value || TODAY,
    end_type: endType,
    end_date: endType === 'until' ? (document.getElementById('rEndDate').value || null) : null,
    end_count: endType === 'count' ? (Number(document.getElementById('rEndCount').value) || 12) : null,
    mode: recMode,
    avg_n: Math.max(2, Number(document.getElementById('rAvgN').value) || 6),
    active: document.getElementById('rActive').checked,
    linked_account_id: linkedAcctId,
    linked_category_id: isTransfer ? (document.getElementById('rLinkedCat')?.value || null) : null,
  };
  const align = !!(recEditing && recMode === 'fixed' && document.getElementById('rApplyExisting')?.checked);
  if (align) {
    body.apply_to_existing = true;
    body.apply_from = document.getElementById('rApplyFrom').value || null;
    body.overwrite_manual = !!document.getElementById('rOverwriteManual').checked;
  }
  /* A ogni salvataggio si confrontano le date previste con i movimenti già registrati (anche per
     modifiche fatte in passato): se c'è qualcosa da decidere si chiede all'utente, con opzioni che
     dipendono da quanto cambia rispetto alla data attuale. */
  if (recEditing) {
    const orig = S.recurrences.find(x => x.id === recEditing);
    const asked = await askRecDateChoice(orig, body);
    if (asked === null) return;
    Object.assign(body, asked);
  }
  const btn = document.getElementById('saveRec');
  if (btn) btn.disabled = true;
  try {
    let report = null;
    if (recEditing) {
      report = await api(`/recurrences/${encodeURIComponent(recEditing)}`, {method: 'PUT', body: JSON.stringify(body)});
    } else {
      await api('/recurrences', {method: 'POST', body: JSON.stringify(body)});
    }
    closeModal('recModal');
    await loadRecurrences();
    renderRecurrences();
    await refreshRegister();
    toast(report && (report.created || report.removed)
      ? t('toast.rec_date_done', {name: desc, created: report.created, removed: report.removed})
      : align && report
      ? t('toast.rec_aligned', {name: desc, n: report.aligned, skipped: report.skipped_manual})
      : recEditing ? t('toast.rec_updated', {name: desc}) : t('toast.rec_saved', {name: desc}));
  } catch (err) {
    fail(err, 'salvataggio ricorrenza');
  } finally {
    if (btn) btn.disabled = false;
  }
}

/* ============================================================
   NAVIGAZIONE
   ============================================================ */
function openSidebar() {
  document.getElementById('sidebar')?.classList.add('is-on');
  document.getElementById('scrim')?.classList.add('is-on');
}
function closeSidebar() {
  document.getElementById('sidebar')?.classList.remove('is-on');
  if (!document.querySelector('.modal.show, .sheet-wrap.is-on')) {
    document.getElementById('scrim')?.classList.remove('is-on');
  }
}

function goPage(p) {
  if (p === '__menu') { openSidebar(); return; }
  S.page = p;
  document.querySelectorAll('.page').forEach(s => s.classList.toggle('is-on', s.id === 'page-' + p));
  document.querySelectorAll('#navList .nav-btn').forEach(b => b.classList.toggle('is-on', b.dataset.page === p));
  document.querySelectorAll('#bottomNav button').forEach(b => b.classList.toggle('is-on', b.dataset.page === p));
  closeSidebar();
  window.scrollTo({top: 0});
  if (p === 'registro') {
    requestAnimationFrame(fitRegisterScroll);
    requestAnimationFrame(layoutToolbar);
  }
  if (p === 'statistiche') { stInvalidate(); renderStats(); }
  if (p === 'ricorrenti') renderRecurrences();
  if (p === 'conti') renderAccountsPage();
  if (p === 'trasferimenti') renderTransfers();
  if (p === 'impostazioni') renderSettings();
  if (p === 'profilo') renderProfile();
}

async function shiftMonth(n) {
  if (S.period === 'around' || S.period === 'month') {
    const [y, m] = S.month.split('-').map(Number);
    const d = new Date(y, m - 1 + n, 1);
    S.month = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
    await applyPeriod('month');
    return;
  }
  /* Con un intervallo di più mesi le frecce lo fanno scorrere di un mese,
     mantenendone l'ampiezza. */
  const [k1, k2] = periodKeys(S.period);
  await setPeriodRange(addMonths(k1, n), addMonths(k2, n));
}

/* ---------- selettore dei mesi ---------- */
const monthIdx = k => Number(k.slice(0, 4)) * 12 + Number(k.slice(5, 7)) - 1;
const addMonths = (k, n) => { const i = monthIdx(k) + n; return `${Math.floor(i / 12)}-${String(i % 12 + 1).padStart(2, '0')}`; };
const PP = {open: false, year: Number(TODAY.slice(0, 4)), pick: null};

function setPeriodRange(k1, k2) {
  if (k1 > k2) [k1, k2] = [k2, k1];
  S.range = {k1, k2};
  return applyPeriod('custom');
}

function currentKeys() { return S.period === 'around' ? [S.month, S.month] : periodKeys(S.period); }

function syncPeriodUI() {
  const lbl = document.getElementById('monthLabel');
  if (lbl) lbl.textContent = monthLabelText();
  if (PP.open) renderPeriodPop();
}

function openPeriodPop(open) {
  PP.open = open;
  PP.pick = null;
  const pop = document.getElementById('periodPop');
  if (!pop) return;
  pop.hidden = !open;
  document.getElementById('monthLabel')?.setAttribute('aria-expanded', String(open));
  if (open) { PP.year = Number(currentKeys()[1].slice(0, 4)); renderPeriodPop(); }
}

/* Evidenzia l'intervallo attuale, o quello in costruzione dopo il primo clic. */
function paintPeriodRange(hover) {
  const pop = document.getElementById('periodPop');
  if (!pop) return;
  let [a, b] = currentKeys();
  if (PP.pick) { const h = hover || PP.pick; [a, b] = PP.pick <= h ? [PP.pick, h] : [h, PP.pick]; }
  pop.querySelectorAll('.pp-m').forEach(btn => {
    const k = btn.dataset.k;
    btn.classList.toggle('in', k >= a && k <= b);
    btn.classList.toggle('edge', k === a || k === b);
  });
}

function renderPeriodPop() {
  const pop = document.getElementById('periodPop');
  if (!pop || !PP.open) return;
  const cur = TODAY.slice(0, 7);
  const [r1, r2] = currentKeys();
  const before = Math.max(0, monthIdx(cur) - monthIdx(r1)), after = Math.max(0, monthIdx(r2) - monthIdx(cur));
  const names = MONTHS();
  const grid = names.map((n, i) => {
    const k = `${PP.year}-${String(i + 1).padStart(2, '0')}`;
    return `<button type="button" class="pp-m ${k === cur ? 'today' : ''}" data-k="${k}">${esc(n.slice(0, 3))}</button>`;
  }).join('');
  const presets = [
    ['per.cur', 'cur', 0, 0], ['per.last', 'cur', -1, -1], ['per.prev_cur_next', 'cur', -1, 1],
    ['per.cur_prev2', 'cur', -2, 0], ['per.cur_next2', 'cur', 0, 2], ['per.last6', 'cur', -5, 0],
    ['per.last12', 'cur', -11, 0], ['per.year', 'year'],
  ].map(([key, kind, a, b]) => `<button type="button" data-preset="${kind}" data-a="${a ?? ''}" data-b="${b ?? ''}">${esc(t(key))}</button>`).join('')
    + `<button type="button" data-preset="around">${esc(t('per.around'))}</button>`
    + `<button type="button" data-preset="goto"><i class="bi bi-calendar-event"></i> ${esc(t('per.goto'))}</button>`;
  pop.innerHTML = `
    <div class="pp-year">
      <button type="button" data-year="-1" aria-label="${esc(t('per.prev_year'))}"><i class="bi bi-chevron-left"></i></button>
      <span>${PP.year}</span>
      <button type="button" data-year="1" aria-label="${esc(t('per.next_year'))}"><i class="bi bi-chevron-right"></i></button>
    </div>
    <div class="pp-grid">${grid}</div>
    <div class="pp-hint">${esc(t('per.hint'))}</div>
    <div class="pp-sep"></div>
    <div class="pp-presets">${presets}</div>
    <div class="pp-sep"></div>
    <div class="pp-rel">
      <label>${esc(t('per.before'))} <input type="number" class="form-control" id="ppBefore" min="0" max="36" value="${before}"></label>
      <label>${esc(t('per.after'))} <input type="number" class="form-control" id="ppAfter" min="0" max="36" value="${after}"></label>
    </div>`;
  paintPeriodRange();
}

async function pickPeriodMonth(k, shift) {
  if (shift) {
    const [a, b] = currentKeys();
    PP.pick = null;
    await setPeriodRange(k < a ? k : a, k > b ? k : b);
  } else if (PP.pick) {
    const first = PP.pick;
    PP.pick = null;
    openPeriodPop(false);
    await setPeriodRange(first, k);
  } else {
    PP.pick = k;
    await setPeriodRange(k, k);
  }
}

async function pickPeriodPreset(btn) {
  const kind = btn.dataset.preset;
  const cur = TODAY.slice(0, 7);
  if (kind === 'goto') {
    openPeriodPop(false);
    const j = document.getElementById('jumpDate');
    if (!j) return;
    j.value = S.month === cur ? TODAY : monthBounds(S.month).first;
    if (j.showPicker) { j.style.pointerEvents = 'auto'; j.showPicker(); }
    else { j.style.cssText = 'height:34px;width:150px'; j.focus(); }
    return;
  }
  openPeriodPop(false);
  if (kind === 'around') return applyPeriod('around');
  if (kind === 'year') return setPeriodRange(`${TODAY.slice(0, 4)}-01`, `${TODAY.slice(0, 4)}-12`);
  return setPeriodRange(addMonths(cur, Number(btn.dataset.a)), addMonths(cur, Number(btn.dataset.b)));
}

/* ---------- riepilogo saldi nascondibile (layout stretto) ----------
   Preferenza per dispositivo, non per utente: resta in localStorage. */
function kpisOff() {
  try { return localStorage.getItem('pfm.kpisOff') === '1'; } catch { return false; }
}
function syncKpiToggle() {
  const off = kpisOff();
  document.body.classList.toggle('kpis-off', off);
  const b = document.getElementById('kpiToggleBtn');
  if (!b) return;
  const label = t(off ? 'reg.kpi_show' : 'reg.kpi_hide');
  b.setAttribute('aria-expanded', off ? 'false' : 'true');
  b.setAttribute('aria-label', label);
  b.title = label;
  const i = b.querySelector('i');
  if (i) i.className = off ? 'bi bi-chevron-bar-down' : 'bi bi-chevron-bar-up';
}

/* ---------- menu toolbar collassabile ---------- */
const TB_COLLAPSE_ORDER = ['emptyDaysBtn', 'selModeBtn', 'regImportBtn', 'regExportBtn'];
let tbLayoutRunning = false;
/* Dove rimettere inline una voce, se non subito prima del ⋯ (ordine originale della toolbar). */
const TB_ANCHOR = {emptyDaysBtn: 'virtualBtn'};

/* Righe visive occupate dai figli visibili della toolbar (raggruppa per posizione verticale). */
function toolbarRows(toolbar) {
  const tops = [...toolbar.children].filter(c => c.getClientRects().length).map(c => c.offsetTop).sort((x, y) => x - y);
  let rows = 0, prev = -1e9;
  for (const v of tops) { if (v - prev > 14) rows++; prev = v; }
  return rows;
}

/* Ogni voce rientra inline solo se non aumenta le righe della toolbar rispetto al caso "tutto nel menu".
   Vale anche sotto i 768px: lì le voci `.no-mobile` non possono stare inline (restano nel menu, dove il CSS
   le mostra) e "Giorni vuoti" esce dal menu solo se c'è spazio. */
function layoutToolbar() {
  if (tbLayoutRunning) return;
  const toolbar = document.querySelector('#page-registro .toolbar');
  const tbMore = document.getElementById('tbMore');
  const tbMorePop = document.getElementById('tbMorePop');
  if (!toolbar || !tbMore || !tbMorePop) return;
  if (toolbar.offsetWidth === 0) return; // Toolbar non visibile

  tbLayoutRunning = true;
  try {
    const items = TB_COLLAPSE_ORDER.map(id => document.getElementById(id)).filter(Boolean);
    const inlineAt = btn => (TB_ANCHOR[btn.id] && document.getElementById(TB_ANCHOR[btn.id])) || tbMore;
    const toMenu = btn => tbMorePop.append(btn);

    // Stato di partenza: tutto nel menu, ⋯ visibile
    items.forEach(toMenu);
    tbMore.hidden = false;
    const baseRows = toolbarRows(toolbar);

    // Rimetti inline, in ordine, finché la toolbar non cresce di una riga
    for (const btn of items) {
      toolbar.insertBefore(btn, inlineAt(btn));
      if (getComputedStyle(btn).display === 'none') { toMenu(btn); continue; } // nascosto (mobile / sola lettura)
      if (toolbarRows(toolbar) > baseRows) { toMenu(btn); break; }
    }
    // Nel menu le voci restano nell'ordine originale della toolbar
    items.filter(b => b.parentElement === tbMorePop).forEach(toMenu);

    // Il ⋯ serve solo se nel menu resta almeno una voce visibile
    const hasVisible = [...tbMorePop.children].some(c => getComputedStyle(c).display !== 'none');
    tbMore.hidden = !hasVisible;
    if (!hasVisible) {
      tbMorePop.hidden = true;
      document.getElementById('tbMoreBtn')?.setAttribute('aria-expanded', 'false');
    }
    syncTbMoreState();
  } finally {
    tbLayoutRunning = false;
  }
}

function syncTbMoreState() {
  const tbMoreBtn = document.getElementById('tbMoreBtn');
  const tbMorePop = document.getElementById('tbMorePop');
  if (!tbMoreBtn || !tbMorePop) return;

  const hasActiveItem = [...tbMorePop.querySelectorAll('.btn-tool.is-on')].length > 0;
  const isOpen = !tbMorePop.hidden;
  tbMoreBtn.classList.toggle('is-on', hasActiveItem || isOpen);
}

/* ============================================================
   EVENTI
   Tutto passa da qui una volta sola: nessun listener aggiunto
   dentro le funzioni di rendering, che girano a ogni ridisegno.
   ============================================================ */
function on(id, ev, fn) { const el = document.getElementById(id); if (el) el.addEventListener(ev, fn); }

function bindEvents() {
  /* ---------- login ---------- */
  document.getElementById('loginForm')?.addEventListener('submit', async e => {
    e.preventDefault();
    const btn = e.target.querySelector('button[type=submit]');
    const username = document.getElementById('loginUser').value;
    const password = document.getElementById('loginPass').value;
    if (btn) btn.disabled = true;
    try {
      const data = await api('/auth/login', {method: 'POST', body: JSON.stringify({username, password})});
      S.user = data.user;
      S.csrfToken = data.csrf_token || '';
      document.getElementById('loginPass').value = '';
      await startSession();
    } catch (err) {
      toast(apiErrorToMessage(err));
    } finally {
      if (btn) btn.disabled = false;
    }
  });

  document.getElementById('pwForm')?.addEventListener('submit', async e => {
    e.preventDefault();
    const cur = document.getElementById('pwCur').value;
    const nw = document.getElementById('pwNew').value;
    if (nw !== document.getElementById('pwNew2').value) { toast(t('set.pw.mismatch')); return; }
    const btn = document.getElementById('pwSave');
    if (btn) btn.disabled = true;
    try {
      await api('/auth/password', {method: 'PUT', body: JSON.stringify({current_password: cur, new_password: nw})});
      e.target.reset();
      toast(t('set.pw.changed'));
    } catch (err) {
      fail(err, 'cambio password');
    } finally {
      if (btn) btn.disabled = false;
    }
  });

  on('profLang', 'change', e => changeLocale(e.target.value));

  document.getElementById('sideUser')?.addEventListener('click', async e => {
    if (!e.target.closest('#logoutBtn')) { if (S.user) goPage('profilo'); return; }
    try { await api('/auth/logout', {method: 'POST'}); } catch { /* la sessione va chiusa comunque */ }
    handleUnauthorized();
  });

  /* ---------- navigazione ---------- */
  on('navList', 'click', e => { const b = e.target.closest('.nav-btn'); if (b) goPage(b.dataset.page); });
  on('bottomNav', 'click', e => { const b = e.target.closest('button'); if (b) goPage(b.dataset.page); });
  document.querySelectorAll('.menuBtn').forEach(b => b.addEventListener('click', openSidebar));
  on('menuBtn', 'click', openSidebar);
  on('kpiToggleBtn', 'click', () => {
    try { localStorage.setItem('pfm.kpisOff', kpisOff() ? '0' : '1'); } catch { /* storage non disponibile */ }
    syncKpiToggle();
  });
  syncKpiToggle();
  on('scrim', 'click', () => { closeSidebar(); closeAllModals(); });
  on('acctList', 'click', async e => {
    const b = e.target.closest('.acct-item');
    if (!b) return;
    await setAccount(b.dataset.acct);
    goPage('registro');
    closeSidebar();
  });
  on('newAcctSide', 'click', e => { e.stopPropagation(); openAcctModal(); });

  /* ---------- barra superiore ---------- */
  on('themeBtn', 'click', async () => {
    S.theme = document.body.classList.contains('theme-dark') ? 'light' : 'dark';
    syncSettingsControls();
    applyTheme();
    await savePrefs();
  });
  on('privacyBtn', 'click', () => setPrivacy(!S.privacy));

  /* ---------- mese e periodo ---------- */
  on('prevMonth', 'click', () => shiftMonth(-1));
  on('nextMonth', 'click', () => shiftMonth(1));
  on('todayBtn', 'click', () => applyPeriod('thisMonth'));
  on('monthLabel', 'click', e => { e.stopPropagation(); openPeriodPop(!PP.open); });
  const pop = document.getElementById('periodPop');
  if (pop) {
    pop.addEventListener('click', e => {
      e.stopPropagation();
      const m = e.target.closest('.pp-m'), y = e.target.closest('[data-year]'), pr = e.target.closest('[data-preset]');
      if (m) pickPeriodMonth(m.dataset.k, e.shiftKey);
      else if (y) { PP.year += Number(y.dataset.year); renderPeriodPop(); }
      else if (pr) pickPeriodPreset(pr);
    });
    pop.addEventListener('mouseover', e => {
      const m = e.target.closest('.pp-m');
      if (m && PP.pick) paintPeriodRange(m.dataset.k);
    });
    pop.addEventListener('change', e => {
      if (e.target.id !== 'ppBefore' && e.target.id !== 'ppAfter') return;
      const n = id => Math.max(0, Math.min(36, Math.floor(Number(document.getElementById(id)?.value) || 0)));
      const cur = TODAY.slice(0, 7);
      setPeriodRange(addMonths(cur, -n('ppBefore')), addMonths(cur, n('ppAfter')));
    });
  }
  document.addEventListener('click', e => { if (PP.open && !e.target.closest('.mp-wrap')) openPeriodPop(false); });
  document.addEventListener('keydown', e => { if (e.key === 'Escape' && PP.open) openPeriodPop(false); });
  on('jumpDate', 'change', async e => {
    const key = e.target.value;
    if (!key) return;
    S.month = key.slice(0, 7);
    const lbl = document.getElementById('monthLabel');
    if (lbl) lbl.textContent = monthLabelText();
    await scrollToDate(key);
  });

  /* ---------- ricerca e filtri ---------- */
  let searchTimer;
  on('searchInput', 'input', e => {
    const v = e.target.value;
    document.getElementById('searchWrap')?.classList.toggle('has-val', !!v);
    clearTimeout(searchTimer);
    // La ricerca è lato server: senza debounce sarebbe una richiesta per tasto.
    searchTimer = setTimeout(() => { S.search = v; refreshRegister(); }, 300);
  });
  on('searchClear', 'click', () => {
    const el = document.getElementById('searchInput');
    if (el) el.value = '';
    document.getElementById('searchWrap')?.classList.remove('has-val');
    S.search = '';
    refreshRegister();
  });
  on('filterBtn', 'click', () => document.getElementById('filters')?.classList.toggle('is-on'));
  ['fCat', 'fTag', 'fTipo', 'fFrom', 'fTo'].forEach(id => on(id, 'change', syncFilters));
  let amtTimer;
  ['fMin', 'fMax'].forEach(id => on(id, 'input', () => {
    clearTimeout(amtTimer);
    amtTimer = setTimeout(syncFilters, 350);
  }));
  on('fReset', 'click', () => clearAllFilters());

  /* ---------- interruttori del registro ---------- */
  on('emptyDaysBtn', 'click', async e => {
    S.showEmpty = !S.showEmpty;
    e.currentTarget.classList.toggle('is-on', S.showEmpty);
    syncSettingsControls();
    await savePrefs();
    await refreshRegister();
  });
  on('selModeBtn', 'click', e => {
    S.selMode = !S.selMode;
    S.selected.clear();
    e.currentTarget.classList.toggle('is-on', S.selMode);
    paintRegister();
  });
  initRubberBand();
  on('selAll', 'change', e => {
    if (e.target.checked) document.querySelectorAll('#regBody [data-sel]').forEach(c => S.selected.add(c.dataset.sel));
    else S.selected.clear();
    paintRegister();
  });
  on('quickRowBtn', 'click', e => {
    S.quickRow = !S.quickRow;
    S.quickFocus = S.quickRow;
    e.currentTarget.classList.toggle('is-on', S.quickRow);
    paintRegister();
  });
  on('newEntryBtn', 'click', () => openEntry());
  on('fab', 'click', () => openEntry());
  on('virtualBtn', 'click', () => {
    document.getElementById('vPanel')?.classList.toggle('is-on');
    renderVirtualPanel();
  });
  on('vPanel', 'change', async e => {
    const c = e.target.closest('[data-virt]');
    if (!c) return;
    const id = c.dataset.virt;
    if (c.checked) S.virtualOn.add(id); else S.virtualOn.delete(id);
    renderVirtualPanel();
    await refreshAll();
  });
  ['rowsTop', 'rowsBottom'].forEach(id => on(id, 'click', e => {
    const b = e.target.closest('[data-rows]');
    if (!b) return;
    const d = b.dataset.rows;
    extendWindow(d === 'up' ? -1 : d === 'down' ? 1 : 'reset');
  }));
  on('regImportBtn', 'click', () => {
    const id = S.accountId === 'all' ? writableAccounts().find(a => a.kind !== 'virtual')?.id : S.accountId;
    if (!id) { toast(t('toast.no_writable')); return; }
    openImport(id);
  });
  on('regExportBtn', 'click', () => exportCSV(S.accountId));

  // Click sul bottone ⋯ per aprire/chiudere il menu
  on('tbMoreBtn', 'click', e => {
    e.stopPropagation();
    const tbMorePop = document.getElementById('tbMorePop');
    const tbMoreBtn = document.getElementById('tbMoreBtn');
    if (!tbMorePop || !tbMoreBtn) return;

    const isOpen = !tbMorePop.hidden;
    tbMorePop.hidden = isOpen;
    tbMoreBtn.setAttribute('aria-expanded', isOpen ? 'false' : 'true');
    syncTbMoreState();
  });

  // Chiudi il menu al click fuori
  document.addEventListener('click', e => {
    const tbMore = document.getElementById('tbMore');
    const tbMorePop = document.getElementById('tbMorePop');
    if (tbMore && tbMorePop && !e.target.closest('#tbMore')) {
      tbMorePop.hidden = true;
      document.getElementById('tbMoreBtn')?.setAttribute('aria-expanded', 'false');
      syncTbMoreState();
    }
  });

  // Chiudi il menu al tasto Escape
  document.addEventListener('keydown', e => {
    if (e.key === 'Escape') {
      const tbMorePop = document.getElementById('tbMorePop');
      if (tbMorePop && !tbMorePop.hidden) {
        tbMorePop.hidden = true;
        document.getElementById('tbMoreBtn')?.setAttribute('aria-expanded', 'false');
        syncTbMoreState();
      }
    }
  });

  // Chiudi il menu quando si clicca una voce (il handler originale del bottone già corre)
  const tbMorePop = document.getElementById('tbMorePop');
  if (tbMorePop) {
    tbMorePop.addEventListener('click', e => {
      const btn = e.target.closest('.btn-tool');
      if (btn) {
        tbMorePop.hidden = true;
        document.getElementById('tbMoreBtn')?.setAttribute('aria-expanded', 'false');
        syncTbMoreState();
      }
    });

    // MutationObserver per sincronizzare lo stato is-on quando cambiano i bottoni nel menu
    const mutObserver = new MutationObserver(() => { syncTbMoreState(); });
    mutObserver.observe(tbMorePop, {subtree: true, attributeFilter: ['class']});
  }

  // ResizeObserver sulla toolbar del registro per agganciare layoutToolbar
  const regToolbar = document.querySelector('#page-registro .toolbar');
  if (regToolbar) {
    let rafId;
    const resizeObserver = new ResizeObserver(() => {
      cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(layoutToolbar);
    });
    resizeObserver.observe(regToolbar);
  }

  /* ---------- azioni sulle righe (delega su tutta la pagina registro) ---------- */
  on('page-registro', 'click', async e => {
    if (e.target.closest('#showFutureBtn')) {
      S.showFuture = true;
      syncSettingsControls();
      await savePrefs();
      await refreshRegister();
      return;
    }
    if (e.target.closest('#clearAll')) { clearAllFilters(); return; }
    if (e.target.closest('#qSave')) { saveQuickRow(); return; }
    const qt = e.target.closest('#qTipo button');
    if (qt) {
      qTipo = qt.dataset.tipo;
      document.querySelectorAll('#qTipo button').forEach(x => x.classList.toggle('is-on', x === qt));
      const cat = document.getElementById('qCat');
      if (cat) cat.innerHTML = catOptionsFor(qTipo);
      qCatTouched = false;
      return;
    }
    const bulkDelBtn = e.target.closest('#bulkDel');
    if (bulkDelBtn) { armedClick(bulkDelBtn, 'reg.confirm_bulk_delete', () => bulkDelete(), {count: S.selected.size}); return; }
    if (e.target.closest('#bulkOff')) { S.selected.clear(); paintRegister(); return; }
    const del = e.target.closest('[data-del]');
    if (del) { armedClick(del, 'reg.confirm_delete', () => deleteTx(del.dataset.del)); return; }
    const ed = e.target.closest('[data-edit]');
    if (ed) {
      if (ed.dataset.recId) openEntry(null, {recId: ed.dataset.recId, date: ed.dataset.recDate});
      else openEntry(ed.dataset.edit);
      return;
    }
    const op = e.target.closest('[data-open]');
    if (op) {
      const row = op.closest('.m-row');
      if (row?.classList.contains('swiped')) { row.classList.remove('swiped'); return; }
      openEntry(op.dataset.open);
      return;
    }
    const ro = e.target.closest('[data-rec-open]');
    if (ro && ro.dataset.recOpen) { openEntry(null, {recId: ro.dataset.recOpen, date: ro.dataset.recDate}); return; }
    const rec = e.target.closest('[data-rec]');
    if (rec) { e.preventDefault(); focusRecurrence(rec.dataset.rec); return; }
    const lk = e.target.closest('[data-pair]');
    if (lk) {
      e.preventDefault();
      let other = null;
      LED.forEach(d => d.entries.forEach(x => { if (x.pair === lk.dataset.pair && x.id && x.id !== lk.dataset.from) other = x; }));
      if (other) { focusTx(other.id); }
      else {
        // The other leg is on a different account (e.g. card settlement
        // backing account). Fetch it from the API.
        api(`/transactions/by-pair/${encodeURIComponent(lk.dataset.pair)}`)
          .then(rows => {
            const other = rows.find(r => r.id && r.id !== lk.dataset.from);
            if (other) focusTx(other.id, other); else toast(t('reg.pair_gone'));
          })
          .catch(() => toast(t('reg.pair_gone')));
      }
    }
  });
  on('regBody', 'change', e => {
    const sel = e.target.closest('[data-sel]');
    if (!sel) return;
    const id = sel.dataset.sel;
    if (sel.checked) S.selected.add(id); else S.selected.delete(id);
    renderBulkBar();
  });
  on('bulkBar', 'change', e => {
    const sel = e.target.closest('#bulkCat');
    if (sel && sel.value) bulkCategory(sel.value);
    const add = e.target.closest('#bulkTagAdd'), rem = e.target.closest('#bulkTagRemove');
    if (add && add.value) bulkTags([add.value], []);
    if (rem && rem.value) bulkTags([], [rem.value]);
  });
  /* Doppio clic su una data del registro: diventa la data della riga rapida. */
  on('regBody', 'dblclick', e => {
    const cell = e.target.closest('td.c-data[data-date]');
    const input = document.getElementById('qDate');
    if (!cell || !input || cell.closest('.quick-row')) return;
    input.value = qLastDate = cell.dataset.date;
    window.getSelection()?.removeAllRanges();
    document.getElementById('qAmount')?.focus();
  });
  initDescAutocomplete();
  on('regBody', 'keydown', e => {
    if (!e.target.closest('.quick-row')) return;
    /* Le option iniziano con l'emoji: la ricerca nativa per lettera non funziona. Cerchiamo sul nome. */
    if (e.target.id === 'qCat' && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== ' ') {
      const sel = e.target;
      const now = Date.now();
      qCatBuf = (now - qCatBufAt < 800 ? qCatBuf : '') + e.key.toLowerCase();
      qCatBufAt = now;
      const opts = [...sel.options];
      const name = o => o.textContent.replace(/^[^\p{L}\p{N}]+/u, '').toLowerCase();
      let hit = opts.find(o => name(o).startsWith(qCatBuf));
      if (!hit && qCatBuf.length > 1 && [...qCatBuf].every(c => c === qCatBuf[0])) {
        const same = opts.filter(o => name(o).startsWith(qCatBuf[0]));
        if (same.length) hit = same[(same.findIndex(o => o.selected) + 1) % same.length];
      }
      if (hit) { sel.value = hit.value; qCatTouched = true; sel.dispatchEvent(new Event('change', {bubbles: true})); }
      e.preventDefault();
      return;
    }
    if (e.key === 'Enter') { e.preventDefault(); saveQuickRow(); }
    if (e.key === 'Escape') { S.quickRow = false; document.getElementById('quickRowBtn')?.classList.remove('is-on'); paintRegister(); }
  });

  /* ---------- sheet nuova voce ---------- */
  on('entrySheet', 'click', e => {
    if (e.target.closest('[data-close]')) { closeEntry(); return; }
    const tg = e.target.closest('#tipoToggle button');
    if (tg) { S.entry.tipo = tg.dataset.tipo; syncTipo(); renderCatGrid(); return; }
    const c = e.target.closest('[data-cat]');
    if (c && !c.disabled) { S.entry.cat = c.dataset.cat; renderCatGrid(); return; }
    const a = e.target.closest('[data-eacct]');
    if (a && !a.disabled) { S.entry.acct = a.dataset.eacct; renderAcctChips(); return; }
    const d = e.target.closest('#dateChips .chip[data-day]');
    if (d && !d.disabled) { S.entry.date = addDays(TODAY, Number(d.dataset.day)); syncDateChips(); return; }
    const f = e.target.closest('#recQuick .chip');
    if (f) {
      document.querySelectorAll('#recQuick .chip').forEach(x => x.classList.toggle('is-on', x === f));
      S.entry.freq = f.dataset.freq;
    }
  });
  on('eDate', 'change', e => { if (e.target.value) { S.entry.date = e.target.value; syncDateChips(); } });
  on('moreToggle', 'click', () => document.getElementById('moreBox')?.classList.toggle('is-on'));
  on('eRecurring', 'change', e => {
    const box = document.getElementById('eRecBox');
    if (box) box.style.display = e.target.checked ? 'block' : 'none';
  });
  on('toAdvanced', 'click', () => { closeEntry(); openRecModal(); });
  on('saveEntry', 'click', () => saveEntry(false));
  on('saveAndNew', 'click', () => saveEntry(true));
  on('eAmount', 'keydown', e => { if (e.key === 'Enter') { e.preventDefault(); saveEntry(false); } });

  /* ---------- calendari festività (admin) ---------- */
  on('holCal', 'change', e => openHolidayAdmin(e.target.value));
  on('holNewBtn', 'click', holCreate);
  on('holRename', 'click', holRename);
  on('holDelete', 'click', holDelete);
  on('holSave', 'click', holSave);
  on('holPreviewBtn', 'click', holPreview);
  on('holAddRule', 'click', () => {
    if (!S.hol) return;
    S.hol.rules.push({label: '', ...HOL_NEW.fixed});
    renderHolidayRules();
  });
  on('holRules', 'change', e => {
    const row = e.target.closest('[data-hrule]');
    const f = e.target.dataset.f;
    if (row && f) holEditRule(Number(row.dataset.hrule), f, e.target.value);
  });
  on('holRules', 'input', e => {
    const row = e.target.closest('[data-hrule]');
    if (row && e.target.dataset.f === 'label') S.hol.rules[Number(row.dataset.hrule)].label = e.target.value;
  });
  on('holRules', 'click', e => {
    const b = e.target.closest('[data-hact]');
    const row = e.target.closest('[data-hrule]');
    if (!b || !row || b.disabled) return;
    const i = Number(row.dataset.hrule), rules = S.hol.rules;
    if (b.dataset.hact === 'del') rules.splice(i, 1);
    else {
      const j = b.dataset.hact === 'up' ? i - 1 : i + 1;
      [rules[i], rules[j]] = [rules[j], rules[i]];
    }
    renderHolidayRules();
  });

  /* ---------- tag ---------- */
  on('newTagBtn', 'click', () => openTagModal());
  on('tagList', 'click', e => {
    const b = e.target.closest('[data-tag-edit]');
    if (b) openTagModal(b.dataset.tagEdit);
  });
  on('tgSave', 'click', saveTag);
  on('tgDelete', 'click', deleteTag);
  on('tagChips', 'click', e => {
    const b = e.target.closest('[data-tag]');
    if (!b) return;
    const id = b.dataset.tag;
    if (S.entry.tags.has(id)) S.entry.tags.delete(id); else S.entry.tags.add(id);
    b.classList.toggle('is-on', S.entry.tags.has(id));
  });

  /* ---------- categorie ---------- */
  on('newCatBtn', 'click', () => openCatModal());
  on('catList', 'click', e => {
    const b = e.target.closest('[data-cat-edit]');
    if (b) openCatModal(b.dataset.catEdit);
  });
  on('ctTipo', 'click', e => {
    const b = e.target.closest('[data-tipo]');
    if (!b || b.disabled) return;
    catForm.tipo = b.dataset.tipo;
    syncCatForm();
  });
  on('ctEmojiGrid', 'click', e => {
    const b = e.target.closest('[data-emoji]');
    if (!b) return;
    catForm.emoji = b.dataset.emoji;
    document.getElementById('ctEmoji').value = '';
    syncCatForm();
  });
  on('ctEmoji', 'input', e => {
    const v = [...e.target.value.trim()][0];
    if (v) { catForm.emoji = v; syncCatForm(); }
  });
  on('ctSave', 'click', saveCategory);
  on('ctDelete', 'click', deleteCategory);

  /* ---------- utenti ---------- */
  on('newUserBtn', 'click', () => openUserModal());
  on('usersBody', 'click', e => {
    const im = e.target.closest('[data-user-imp]');
    if (im && !im.disabled) { startImpersonation(im.dataset.userImp); return; }
    const ed = e.target.closest('[data-user-edit]');
    if (ed) { openUserModal(ed.dataset.userEdit); return; }
    const dl = e.target.closest('[data-user-del]');
    if (dl && !dl.disabled) openUserModal(dl.dataset.userDel);
  });
  on('usSave', 'click', saveUser);
  on('usDelete', 'click', e => {
    const btn = e.currentTarget;
    if (btn.dataset.armed !== '1') {
      btn.dataset.armed = '1';
      btn.innerHTML = `<i class="bi bi-exclamation-triangle"></i> ${t('modal.user.confirm_delete')}`;
      setTimeout(() => {
        btn.dataset.armed = '';
        btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.user.delete')}`;
      }, 5000);
      return;
    }
    btn.dataset.armed = '';
    btn.innerHTML = `<i class="bi bi-trash3"></i> ${t('modal.user.delete')}`;
    deleteUser(userEditing);
  });
  on('impBar', 'click', e => { if (e.target.closest('#impExit')) stopImpersonation(); });

  /* ---------- conti di tutti gli utenti (admin) ---------- */
  on('adminAcctBody', 'click', e => {
    const ed = e.target.closest('[data-admin-edit]');
    if (ed) { openAcctModal(ed.dataset.adminEdit); return; }
    const dl = e.target.closest('[data-admin-del]');
    if (dl) {
      armedClick(dl, 'modal.acct.confirm_delete', () => deleteAdminAccount(dl.dataset.adminDel));
    }
  });

  /* ---------- pagina conti ---------- */
  on('page-conti', 'click', async e => {
    const rl = e.target.closest('[data-rules-acct]');
    if (rl) { openRulesModal(rl.dataset.rulesAcct); return; }
    const ed = e.target.closest('[data-edit-acct]');
    if (ed) { openAcctModal(ed.dataset.editAcct); return; }
    const cc = e.target.closest('[data-card-charge]');
    if (cc) { cardCharge(cc.dataset.cardCharge); return; }
    const dp = e.target.closest('[data-deposit]');
    if (dp) { depositConsolidate(dp.dataset.deposit); return; }
    const op = e.target.closest('[data-open-acct]');
    if (op) { await setAccount(op.dataset.openAcct); goPage('registro'); return; }
    const im = e.target.closest('[data-import-acct]');
    if (im) { openImport(im.dataset.importAcct); return; }
    const ex = e.target.closest('[data-export-acct]');
    if (ex) { exportCSV(ex.dataset.exportAcct); return; }
  });
  on('newAcctBtn', 'click', () => openAcctModal());

  /* ---------- maschera conto ---------- */
  on('acTipo', 'click', e => {
    const b = e.target.closest('[data-tipo]');
    if (!b || b.disabled) return;
    acForm.tipo = b.dataset.tipo;
    const def = {corrente: 'bi-bank', deposito: 'bi-piggy-bank', carta: 'bi-credit-card', virtuale: 'bi-bookmark-star'};
    if (!acctEditing) acForm.icon = def[acForm.tipo];
    syncAcctForm();
  });
  on('acIcons', 'click', e => {
    const b = e.target.closest('[data-icon]');
    if (!b) return;
    acForm.icon = b.dataset.icon;
    syncAcctForm();
  });
  on('acShareAdd', 'click', addShareByEmail);
  on('acShareEmail', 'keydown', e => { if (e.key === 'Enter') { e.preventDefault(); addShareByEmail(); } });
  on('acShareList', 'click', e => {
    const del = e.target.closest('[data-share-del]');
    if (del) { acShares = acShares.filter(sh => sh.user_id !== del.dataset.shareDel); renderShareList(); }
  });
  on('acShareList', 'change', e => {
    const sel = e.target.closest('[data-share-level]');
    if (!sel) return;
    const sh = acShares.find(x => x.user_id === sel.dataset.shareLevel);
    if (sh) sh.level = sel.value;
  });
  on('acSave', 'click', saveAccount);
  on('acDelete', 'click', deleteAccount);
  on('acOpenRules', 'click', () => {
    if (!acctEditing) { toast(t('acct.rules_after_save')); return; }
    const id = acctEditing;
    closeModal('acctModal');
    setTimeout(() => openRulesModal(id), 200);
  });

  /* ---------- regole ---------- */
  on('ruAdd', 'click', () => openRuleEditor(null));
  on('ruCancel', 'click', showRulesList);
  on('ruSave', 'click', saveRule);
  on('ruClose', 'click', () => closeModal('rulesModal'));
  on('ruDelete', 'click', e => armedClick(e.currentTarget, 'rule.confirm_delete', () => deleteRule(ruleEditing)));
  on('ruListPanel', 'click', e => {
    const pr = e.target.closest('[data-preset]');
    if (pr) {
      const p = PRESETS[pr.dataset.preset];
      openRuleEditor({...p, category_id: S.categories.find(c => c.kind === 'out')?.id});
      return;
    }
    const ed = e.target.closest('[data-rule-edit]');
    if (ed) { openRuleEditor(acRules.find(r => r.id === ed.dataset.ruleEdit)); return; }
    const tg = e.target.closest('[data-rule-toggle]');
    if (tg) { toggleRule(tg.dataset.ruleToggle); return; }
    const gn = e.target.closest('[data-rule-gen]');
    if (gn) { generateRule(gn.dataset.ruleGen); return; }
    const dl = e.target.closest('[data-rule-del]');
    if (dl) armedClick(dl, 'rule.confirm_delete', () => deleteRule(dl.dataset.ruleDel));
  });
  ['ruTipo', 'ruCadenza', 'ruBase', 'ruGiorno', 'ruMese', 'ruAnnuale', 'ruProrata', 'ruOp', 'ruCat'].forEach(id => on(id, 'change', syncRuleForm));
  ['ruValore', 'ruSoglia', 'ruCap'].forEach(id => on(id, 'input', updateRulePreview));

  /* ---------- import ---------- */
  on('impDrop', 'click', () => document.getElementById('impFile')?.click());
  on('impDrop', 'dragover', e => { e.preventDefault(); e.currentTarget.classList.add('over'); });
  on('impDrop', 'dragleave', e => e.currentTarget.classList.remove('over'));
  on('impDrop', 'drop', e => {
    e.preventDefault();
    e.currentTarget.classList.remove('over');
    const f = e.dataTransfer?.files?.[0];
    if (f) readCsvFile(f);
  });
  on('impFile', 'change', e => { const f = e.target.files?.[0]; if (f) readCsvFile(f); });

  on('impBack', 'click', () => {
    const show = (i, on2) => { const el = document.getElementById(i); if (el) el.style.display = on2 ? '' : 'none'; };
    show('impStep1', true);
    show('impStep2', false);
    show('impConfirm', false);
    show('impBack', false);
    const f = document.getElementById('impFile');
    if (f) f.value = '';
  });
  ['impColDate', 'impColDesc', 'impColAmount', 'impDateFmt', 'impDec'].forEach(id => on(id, 'change', buildImportPreview));
  on('impConfirm', 'click', runImport);

  /* ---------- ricorrenti ---------- */
  on('newRecBtn', 'click', () => openRecModal());
  on('page-ricorrenti', 'click', e => {
    const ed = e.target.closest('[data-recedit]');
    if (ed) { openRecModal(ed.dataset.recedit); return; }
    const dl = e.target.closest('[data-recdel]');
    if (dl) {
      armedClick(dl, 'rec.confirm_delete', () => deleteRecurrence(dl.dataset.recdel));
    }
  });
  on('rTipo', 'click', e => {
    const b = e.target.closest('button');
    if (!b) return;
    document.querySelectorAll('#rTipo button').forEach(x => x.classList.toggle('is-on', x === b));
    syncRecCategories();
  });
  on('rMode', 'click', e => {
    const b = e.target.closest('[data-mode]');
    if (!b) return;
    recMode = b.dataset.mode;
    syncRecMode();
  });
  on('rAvgN', 'input', syncRecMode);
  on('rEndType', 'change', syncRecEnd);
  on('rIsTransfer', 'change', syncRecTransfer);
  on('rAcct', 'change', syncRecTransfer);
  ['rFirst', 'rPattern', 'rWeekend', 'rSkip'].forEach(id => on(id, 'change', updateRecPreview));
  ['rFirst', 'rPattern'].forEach(id => on(id, 'change', syncPatternHint));
  on('saveRec', 'click', saveRecurrence);
  document.querySelector('th[data-recsort]')?.closest('thead')?.addEventListener('click', e => {
    const th = e.target.closest('th[data-recsort]');
    if (!th) return;
    const k = th.dataset.recsort;
    if (REC_VIEW.key === k) REC_VIEW.dir = -REC_VIEW.dir; else { REC_VIEW.key = k; REC_VIEW.dir = 1; }
    renderRecurrences();
  });
  on('recGroup', 'change', e => {
    REC_VIEW.group = e.target.checked;
    try { localStorage.setItem('pfm.recGroup', REC_VIEW.group ? '1' : '0'); } catch { /* storage non disponibile */ }
    renderRecurrences();
  });
  on('rApplyExisting', 'change', syncRecApply);

  /* ---------- trasferimenti ---------- */
  on('transferForm', 'submit', e => { e.preventDefault(); submitTransfer(); });
  on('trCancel', 'click', resetTransferForm);
  on('trFrom', 'change', updateTransferAvail);
  on('trSwap', 'click', () => {
    const a = document.getElementById('trFrom'), b = document.getElementById('trTo');
    if (!a || !b) return;
    const av = a.value;
    a.value = b.value;
    b.value = av;
    updateTransferAvail();
  });
  on('trPeriod', 'change', e => { S.trPeriod = e.target.value; renderTransfers(); });
  on('trHistory', 'click', e => {
    const rc = e.target.closest('[data-trrec]');
    if (rc) { openRecModal(rc.dataset.trrec); return; }
    const ed = e.target.closest('[data-tredit]');
    if (ed) { editTransfer(ed.dataset.tredit); return; }
    const dl = e.target.closest('[data-trdel]');
    if (dl) {
      armedClick(dl, 'tr.confirm_delete', () => deleteTransfer(dl.dataset.trdel));
    }
  });

  /* ---------- statistiche ---------- */
  const stPage = document.getElementById('page-statistiche');
  const closeStatsMulti = except => document.querySelectorAll('#page-statistiche .ms-wrap').forEach(w => {
    if (w === except) return;
    const p = w.querySelector('.ms-pop');
    if (p) p.hidden = true;
    w.querySelector('.ms-btn')?.setAttribute('aria-expanded', 'false');
  });
  on('statsAccount', 'change', e => { S.stats.acct = e.target.value; renderStats(); });
  const shiftStats = n => setStatsMonths(addMonths(S.stats.k1, n), addMonths(S.stats.k2, n));
  on('stPrev', 'click', () => shiftStats(-1));
  on('stNext', 'click', () => shiftStats(1));
  on('stToday', 'click', () => { const c = TODAY.slice(0, 7); setStatsMonths(c, c); });
  on('stMonthLabel', 'click', e => { e.stopPropagation(); closeStatsMulti(); openStatsPop(!SPP.open); });
  const spop = document.getElementById('stPeriodPop');
  if (spop) {
    spop.addEventListener('click', e => {
      e.stopPropagation();
      const m = e.target.closest('.pp-m'), y = e.target.closest('[data-syear]'), pr = e.target.closest('[data-spreset]');
      if (m) {
        const k = m.dataset.k;
        if (e.shiftKey) { const a = S.stats.k1, b = S.stats.k2; SPP.pick = null; setStatsMonths(k < a ? k : a, k > b ? k : b); }
        else if (SPP.pick) { const first = SPP.pick; SPP.pick = null; openStatsPop(false); setStatsMonths(first, k); }
        else { SPP.pick = k; setStatsMonths(k, k); }
      } else if (y) { SPP.year += Number(y.dataset.syear); renderStatsPop(); }
      else if (pr) {
        const cur = TODAY.slice(0, 7), a = pr.dataset.spreset;
        openStatsPop(false);
        if (a === 'year') setStatsMonths(`${TODAY.slice(0, 4)}-01`, `${TODAY.slice(0, 4)}-12`);
        else setStatsMonths(addMonths(cur, Number(a)), addMonths(cur, Number(pr.dataset.b)));
      }
    });
    spop.addEventListener('mouseover', e => {
      const m = e.target.closest('.pp-m');
      if (m && SPP.pick) paintStatsPop(m.dataset.k);
    });
  }
  document.addEventListener('click', e => {
    if (SPP.open && !e.target.closest('#page-statistiche .mp-wrap')) openStatsPop(false);
    if (!e.target.closest('.ms-wrap')) closeStatsMulti();
  });
  document.addEventListener('keydown', e => { if (e.key === 'Escape') { if (SPP.open) openStatsPop(false); closeStatsMulti(); } });

  /* menu a scelta multipla (categorie, tag) */
  stPage?.addEventListener('click', e => {
    const btn = e.target.closest('.ms-btn');
    if (btn) {
      const wrap = btn.closest('.ms-wrap'), pop = wrap.querySelector('.ms-pop');
      closeStatsMulti(wrap);
      if (SPP.open) openStatsPop(false);
      pop.hidden = !pop.hidden;
      btn.setAttribute('aria-expanded', String(!pop.hidden));
      return;
    }
    const clr = e.target.closest('[data-msclr]');
    if (clr) { (clr.closest('.ms-wrap').id === 'stCatMs' ? S.stats.cats : S.stats.tags).clear(); paintStats(); return; }
    const seg = e.target.closest('.st-seg button');
    if (seg) { S.stats[seg.closest('.st-seg').dataset.seg] = seg.dataset.v; paintStats(); return; }
    const lg = e.target.closest('[data-pie]');
    if (lg) {
      const set = S.stats.pieBy === 'cat' ? S.stats.cats : S.stats.tags;
      if (set.has(lg.dataset.pie)) set.delete(lg.dataset.pie); else set.add(lg.dataset.pie);
      paintStats();
      return;
    }
    const chip = e.target.closest('[data-chip]');
    if (chip) removeStatsChip(chip.dataset.chip, chip.dataset.v);
  });
  stPage?.addEventListener('change', e => {
    const cb = e.target.closest('.ms-pop input[type="checkbox"]');
    if (!cb) return;
    const set = cb.closest('.ms-wrap').id === 'stCatMs' ? S.stats.cats : S.stats.tags;
    if (cb.checked) set.add(cb.value); else set.delete(cb.value);
    paintStats();
  });

  on('stFrom', 'change', e => { S.stats.from = e.target.value; renderStats(); });
  on('stTo', 'change', e => { S.stats.to = e.target.value; renderStats(); });
  let stTimer;
  on('stSearch', 'input', e => {
    const v = e.target.value;
    document.getElementById('stSearchWrap')?.classList.toggle('has-val', !!v);
    clearTimeout(stTimer);
    stTimer = setTimeout(() => { S.stats.search = v; paintStats(); }, 200);
  });
  on('stSearchClear', 'click', () => {
    const el = document.getElementById('stSearch');
    if (el) el.value = '';
    S.stats.search = '';
    paintStats();
  });
  on('stFilterBtn', 'click', () => document.getElementById('stFilters')?.classList.toggle('is-on'));
  on('stGran', 'change', e => { S.stats.gran = e.target.value; paintStats(); });
  on('stForecastBtn', 'click', () => { S.stats.forecast = !S.stats.forecast; paintStats(); });
  on('stTransfersBtn', 'click', () => { S.stats.transfers = !S.stats.transfers; paintStats(); });
  ['stTipo', 'stMin', 'stMax'].forEach(id => on(id, 'change', () => {
    S.stats.tipo = document.getElementById('stTipo').value;
    S.stats.min = document.getElementById('stMin').value;
    S.stats.max = document.getElementById('stMax').value;
    paintStats();
  }));
  on('stReset', 'click', () => { resetStatsFilters(); renderStats(); });

  /* ---------- impostazioni ---------- */
  on('setTabs', 'click', e => { const b = e.target.closest('[data-tab]'); if (b) showSetTab(b.dataset.tab); });
  /* Il calendario decide i giorni lavorativi di ricorrenze e addebiti carta: cambia le date
     previste, quindi ricarico il registro. Le righe già registrate non vengono toccate. */
  on('setHolidays', 'change', async e => {
    S.holidayCalendar = e.target.value;
    await savePrefs();
    await refreshAll();
    toast(t('set.holidays_saved'));
  });
  /* Anteprima viva mentre si trascina il selettore, salvataggio al rilascio. */
  [['setColorPos', 'colorPos'], ['setColorNeg', 'colorNeg']].forEach(([id, key]) => {
    on(id, 'input', e => { S[key] = e.target.value; applyAmountColors(); });
    on(id, 'change', async e => { S[key] = e.target.value; applyAmountColors(); await savePrefs(); });
  });
  [['setColorPosReset', 'colorPos'], ['setColorNegReset', 'colorNeg']].forEach(([id, key]) => {
    on(id, 'click', async () => { S[key] = ''; applyAmountColors(); await savePrefs(); });
  });
  on('setTheme', 'change', async e => { S.theme = e.target.value; applyTheme(); await savePrefs(); });
  on('setEmptyDays', 'change', async e => {
    S.showEmpty = e.target.checked;
    document.getElementById('emptyDaysBtn')?.classList.toggle('is-on', S.showEmpty);
    await savePrefs();
    await refreshRegister();
  });
  on('setFuture', 'change', async e => { S.showFuture = e.target.checked; await savePrefs(); await refreshRegister(); });
  on('setSign', 'change', async e => { S.showSign = e.target.checked; await savePrefs(); paintRegister(); });
  on('setPrivacy', 'change', e => setPrivacy(e.target.checked));
  on('setMSaldo', 'change', async e => {
    S.showMSaldo = e.target.checked;
    document.body.classList.toggle('show-msaldo', S.showMSaldo);
    await savePrefs();
  });
  ['setAbove', 'setBelow', 'setStep'].forEach(id => on(id, 'change', async () => {
    S.rows = {
      above: Math.max(0, Number(document.getElementById('setAbove').value) || 0),
      below: Math.max(1, Number(document.getElementById('setBelow').value) || 1),
      step: Math.max(5, Number(document.getElementById('setStep').value) || 15),
    };
    await savePrefs();
    await refreshRegister();
  }));

  /* ---------- chiusura modali ----------
     Quando Bootstrap è presente `data-bs-dismiss` è già suo: intervengo solo
     nel ripiego manuale, per non chiudere due volte lo stesso modale. */
  document.addEventListener('click', e => {
    if (hasBootstrap()) return;
    const el = e.target.closest('[data-bs-dismiss="modal"]');
    if (!el) return;
    const modal = el.closest('.modal');
    if (modal) closeModal(modal.id);
  });

  /* ---------- scorciatoie ---------- */
  document.addEventListener('keydown', e => {
    if (!S.logged) return;
    const typing = /^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName || '');
    if (e.key === 'Escape') {
      if (document.getElementById('entrySheet')?.classList.contains('is-on')) closeEntry();
      else if (document.querySelector('.modal.show')) closeAllModals();
      else if (S.selMode) {
        S.selMode = false;
        S.selected.clear();
        document.getElementById('selModeBtn')?.classList.remove('is-on');
        paintRegister();
      }
      return;
    }
    if (e.ctrlKey && e.key === 'Enter' && document.getElementById('entrySheet')?.classList.contains('is-on')) {
      saveEntry(true);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && /^[zy]$/i.test(e.key)
        && !document.getElementById('entrySheet')?.classList.contains('is-on') && !document.querySelector('.modal.show')) {
      const el = document.activeElement;
      const textual = el && (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && el.value));
      const redo = e.key.toLowerCase() === 'y' || e.shiftKey;
      const stack = redo ? REDO_STACK : UNDO_STACK;
      if (!textual && stack.length) { e.preventDefault(); (redo ? redoLastEdit : undoLastEdit)(); return; }
    }
    if (typing) return;
    if (e.key === 'n' || e.key === 'N') { e.preventDefault(); openEntry(); }
    if (e.key === '/') { e.preventDefault(); document.getElementById('searchInput')?.focus(); }
    if (e.key === 't' || e.key === 'T') applyPeriod('thisMonth');
  });

  initRegisterFit();

  /* ---------- ridimensionamento ---------- */
  let rtimer;
  window.addEventListener('resize', () => {
    clearTimeout(rtimer);
    rtimer = setTimeout(() => {
      if (window.innerWidth > 991) closeSidebar();
      if (!S.logged) return;
      const lbl = document.getElementById('monthLabel');
      if (lbl) lbl.textContent = monthLabelText();
      paintRegister();
    }, 160);
  });

  /* ---------- coda offline ----------
     `offline-queue.js` si risincronizza da sé al ritorno della rete e annuncia
     l'esito con `pfm:offline-synced`: qui basta ricaricare ciò che si vede. */
  window.addEventListener('pfm:offline-synced', async e => {
    if (!S.logged) return;
    await refreshAll();
    const n = e.detail?.count || 0;
    if (n) toast(t('toast.offline_synced', {n}));
  });
  window.addEventListener('offline', () => toast(t('toast.offline')));
}

function readCsvFile(file) {
  const rd = new FileReader();
  rd.onload = () => loadCSV(String(rd.result || ''));
  rd.onerror = () => toast(t('modal.imp.read_failed'));
  rd.readAsText(file, 'utf-8');
}

/* ============================================================
   CAMPI DATA — formato dipendente dalla lingua del Profilo
   Ogni input[type=date] viene "upgradato": il nativo resta nel DOM
   (nascosto) e continua a tenere il valore ISO; accanto compare un campo
   testo gg/mm/aaaa o mm/dd/yyyy (LANG.date) + pulsante calendario.
   Il codice esistente legge/scrive .value sul nativo come prima.
   ============================================================ */
const DATE_NATIVE_SET = typeof HTMLInputElement !== 'undefined'
  ? Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set : null;

function dateIsoToText(v) { return /^\d{4}-\d{2}-\d{2}$/.test(v || '') ? fmtDate(v) : ''; }

/* "12/03/2026" (secondo LANG.date) → "2026-03-12"; null se incompleta/inesistente. */
function dateTextToIso(txt) {
  const dg = String(txt).replace(/\D/g, '');
  if (dg.length !== 8) return null;
  const a = Number(dg.slice(0, 2)), b = Number(dg.slice(2, 4)), y = Number(dg.slice(4));
  const d = LANG.date === 'mdy' ? b : a, m = LANG.date === 'mdy' ? a : b;
  const dt = new Date(y, m - 1, d);
  if (y < 1000 || dt.getFullYear() !== y || dt.getMonth() !== m - 1 || dt.getDate() !== d) return null;
  return iso(dt);
}

/* Solo cifre, max 8, con '/' inseriti dopo la 2ª e la 4ª cifra. */
function dateMask(raw) {
  const dg = String(raw).replace(/\D/g, '').slice(0, 8);
  return dg.length > 4 ? `${dg.slice(0, 2)}/${dg.slice(2, 4)}/${dg.slice(4)}`
    : dg.length > 2 ? `${dg.slice(0, 2)}/${dg.slice(2)}` : dg;
}

function upgradeDateInput(nat) {
  if (nat.dataset.dateup || nat.id === 'jumpDate' || !nat.parentNode) return;
  nat.dataset.dateup = '1';
  const id = nat.id;
  const wrap = document.createElement('span');
  wrap.className = 'dateup';
  if (nat.style.width) { wrap.classList.add('is-fixed'); wrap.style.width = `calc(${nat.style.width} + 1.6rem)`; }
  const txt = document.createElement('input');
  txt.type = 'text';
  txt.className = nat.className + ' dateup-txt';
  if (nat.style.cssText) txt.style.cssText = nat.style.cssText;
  if (nat.style.width) txt.style.width = '100%';
  if (id) txt.id = id + '__txt';
  txt.setAttribute('inputmode', 'numeric');
  txt.setAttribute('autocomplete', 'off');
  txt.setAttribute('maxlength', '10');
  const al = nat.getAttribute('aria-label');
  if (al) txt.setAttribute('aria-label', al);
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'dateup-btn';
  btn.tabIndex = -1;
  btn.innerHTML = '<i class="bi bi-calendar3"></i>';
  nat.parentNode.insertBefore(wrap, nat);
  wrap.append(txt, btn, nat);
  if (id) document.querySelectorAll(`label[for="${id}"]`).forEach(l => l.setAttribute('for', id + '__txt'));

  let busy = false;
  const natGet = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').get;
  const refresh = () => {
    txt.value = dateIsoToText(natGet.call(nat));
    txt.placeholder = t('date.placeholder');
    btn.title = t('date.pick');
    btn.setAttribute('aria-label', t('date.pick'));
    txt.classList.remove('is-invalid');
  };
  nat._dateRefresh = refresh;
  /* value/disabled/required: il codice esistente scrive sul nativo, il testo segue. */
  Object.defineProperty(nat, 'value', {
    configurable: true,
    get() { return natGet.call(nat); },
    set(v) { DATE_NATIVE_SET.call(nat, v); refresh(); },
  });
  for (const prop of ['disabled', 'required']) {
    const pd = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, prop);
    Object.defineProperty(nat, prop, {
      configurable: true,
      get() { return pd.get.call(nat); },
      set(v) { pd.set.call(nat, v); txt[prop] = !!v; if (prop === 'disabled') btn.disabled = !!v; },
    });
  }
  txt.disabled = nat.disabled; btn.disabled = nat.disabled; txt.required = nat.required;

  const push = v => {
    busy = true;
    DATE_NATIVE_SET.call(nat, v);
    nat.dispatchEvent(new Event('input', {bubbles: true}));
    nat.dispatchEvent(new Event('change', {bubbles: true}));
    busy = false;
  };
  txt.addEventListener('input', () => {
    const pos = txt.selectionStart ?? txt.value.length;
    const digitsBefore = txt.value.slice(0, pos).replace(/\D/g, '').length;
    const masked = dateMask(txt.value);
    if (masked !== txt.value) {
      txt.value = masked;
      let n = 0, p = 0;
      while (p < masked.length && n < digitsBefore) { if (/\d/.test(masked[p])) n++; p++; }
      if (masked[p] === '/' && digitsBefore === masked.replace(/\D/g, '').length) p++;
      try { txt.setSelectionRange(p, p); } catch { /* ignora */ }
    }
    if (!masked) { txt.classList.remove('is-invalid'); if (natGet.call(nat) !== '') push(''); return; }
    const v = dateTextToIso(masked);
    if (v) { txt.classList.remove('is-invalid'); if (v !== natGet.call(nat)) push(v); }
    else if (masked.replace(/\D/g, '').length === 8) txt.classList.add('is-invalid');
    else txt.classList.remove('is-invalid');
  });
  txt.addEventListener('blur', () => {
    if (txt.value && !dateTextToIso(txt.value)) txt.classList.add('is-invalid');
  });
  /* Invio con testo non valido: non propagare (la riga rapida salverebbe la data precedente). */
  txt.addEventListener('keydown', e => {
    if (e.key === 'Enter' && txt.value && !dateTextToIso(txt.value)) {
      e.preventDefault(); e.stopPropagation(); txt.classList.add('is-invalid');
    }
  });
  nat.addEventListener('change', () => { if (!busy) refresh(); });
  btn.addEventListener('click', () => {
    if (nat.disabled) return;
    try { nat.showPicker(); } catch { nat.focus(); }
  });
  refresh();
}

function upgradeDateFields(root) {
  if (typeof HTMLInputElement === 'undefined' || !root || !root.querySelectorAll) return;
  if (root.matches && root.matches('input[type="date"]')) upgradeDateInput(root);
  root.querySelectorAll('input[type="date"]:not([data-dateup])').forEach(upgradeDateInput);
}

/* Al cambio lingua: placeholder e testo nel nuovo formato. */
function refreshDateFields() {
  if (typeof document === 'undefined') return;
  document.querySelectorAll('input[data-dateup]').forEach(n => n._dateRefresh && n._dateRefresh());
}

function initDateFields() {
  upgradeDateFields(document.body);
  if (typeof MutationObserver === 'undefined') return;
  new MutationObserver(muts => {
    for (const m of muts) m.addedNodes.forEach(n => { if (n.nodeType === 1) upgradeDateFields(n); });
  }).observe(document.body, {childList: true, subtree: true});
}

/* ============================================================
   AVVIO
   ============================================================ */
async function startSession() {
  S.logged = true;
  /* La coda offline spedisce solo i movimenti accodati da questo utente. */
  if (window.pfmOfflineQueue) window.pfmOfflineQueue.userId = (S.realUser || S.user)?.id || null;
  document.getElementById('loginScreen').style.display = 'none';
  document.getElementById('app').classList.add('is-on');
  await loadI18n(S.user?.locale);
  applyStaticI18n();
  renderProfile();
  await loadPrefs();
  syncSettingsControls();
  applyTheme();
  await loadAll();
  renderSideUser();
  renderImpBar();
  renderSidebarAccounts();
  renderRegTitle();
  refreshCategoryUI();
  renderSettings();
  await applyPeriod('thisMonth', {silent: true});
  loadInterestIfDeposit();
  setTimeout(() => scrollToAnchor(), 100);
}

async function boot() {
  await loadI18n('it');
  bindEvents();
  initDateFields();
  applyTheme();
  try {
    const me = await api('/auth/me');
    if (me?.user) {
      S.user = me.user;
      await startSession();
    }
  } catch {
    // Nessuna sessione attiva: resta la schermata di accesso.
    document.getElementById('loginScreen').style.display = 'flex';
  }
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js').catch(err => console.warn('service worker non registrato', err));
  }
}

boot();
