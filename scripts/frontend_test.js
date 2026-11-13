/**
 * Test di non-regressione della logica pura del frontend.
 *
 * Non sostituisce una prova nel browser: costruisce un DOM minimo solo per
 * poter valutare web/app.js e chiamare le funzioni che fanno i conti. Serve a
 * intercettare, senza aprire una pagina, la classe di difetti che la Fase 1 e
 * la Fase 2 hanno pagato caro: il segno degli importi, il saldo progressivo e
 * la lettura dei campi del DTO.
 *
 * Nessuna dipendenza: node scripts/frontend_test.js dalla radice del progetto.
 */
/* DOM minimo, nessuna rete: serve solo a far valutare il file. */
const els = new Map();
function makeEl(id) {
  const el = {
    id, value: '', textContent: '', innerHTML: '', style: {}, dataset: {},
    disabled: false, checked: false, options: [], files: [],
    classList: {_s:new Set(), add(...c){c.forEach(x=>this._s.add(x));},
      remove(...c){c.forEach(x=>this._s.delete(x));},
      toggle(c,on){ on===undefined? (this._s.has(c)?this._s.delete(c):this._s.add(c)) : (on?this._s.add(c):this._s.delete(c)); },
      contains(c){return this._s.has(c);}},
    addEventListener(){}, removeEventListener(){}, setAttribute(){}, removeAttribute(){},
    getAttribute(){return null;}, appendChild(){}, prepend(){}, remove(){}, focus(){},
    closest(){return null;}, querySelector(){return makeEl('x');}, querySelectorAll(){return [];},
    toggleAttribute(){}, scrollIntoView(){}, getBoundingClientRect(){return {top:0};},
    getContext(){return {};}, scrollTop:0, scrollHeight:0, clientHeight:0, offsetTop:0,
  };
  return el;
}
const doc = {
  body: makeEl('body'),
  scrollingElement: makeEl('scroller'),
  cookie: '',
  documentElement: makeEl('html'),
  getElementById(id){ if(!els.has(id)) els.set(id, makeEl(id)); return els.get(id); },
  querySelector(){ return null; },
  querySelectorAll(){ return []; },
  createElement(){ return makeEl('new'); },
  addEventListener(){},
  get activeElement(){ return {tagName:'BODY'}; },
};
global.document = doc;
global.window = {
  innerWidth: 1280,
  addEventListener(){}, removeEventListener(){},
  matchMedia(){ return {matches:false, addEventListener(){}}; },
  scrollTo(){}, scrollY:0,
  location:{href:'http://localhost/'},
};
global.navigator = {onLine:true, serviceWorker:undefined};
global.localStorage = {getItem(){return null;}, setItem(){}, removeItem(){}};
global.getComputedStyle = () => ({getPropertyValue: () => '#000'});
global.requestAnimationFrame = fn => fn();
global.fetch = async () => ({ok:false, status:401, headers:{get(){return null;}}, json:async()=>({}), text:async()=>''});
global.Chart = undefined;
global.crypto = require('crypto').webcrypto;

const path = require('path');
const appPath = path.join(__dirname, '..', 'web', 'app.js');
const src = require('fs').readFileSync(appPath, 'utf8');
// Esporto lo scope per poter esercitare le funzioni pure.
const fn = new Function('module', src + '\nmodule.exports={parseAmount,parsePct,buildLed,periodRange,statsRange,prevRange,computeWindow,visibleDays,S,LED_OPENING,money,fmtDate,fmtDayLong,patternFromForm,patternToForm,ruleDescription,splitCSV,looksLikeDate,addDays,iso,fromISO,rowToEntry,t,addMonths,periodKeys,monthLabelText,setLed:d=>{LED=d;}};');
const m = {exports:{}};
fn(m);
const A = m.exports;
console.log('web/app.js valutato senza eccezioni al livello superiore');

let pass = 0, fail = 0;
function eq(got, want, label) {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  ok ? pass++ : fail++;
  console.log(`${ok ? 'ok  ' : 'FAIL'}  ${label}` + (ok ? '' : `\n        atteso ${JSON.stringify(want)}, ottenuto ${JSON.stringify(got)}`));
}

/* --- B1: il segno non viene mai deciso da parseAmount --- */
eq(A.parseAmount('42,18'), 4218, 'parseAmount virgola decimale');
eq(A.parseAmount('1.234,56'), 123456, 'parseAmount separatore migliaia');
eq(A.parseAmount('-42,18'), 4218, 'parseAmount ignora il segno digitato');
eq(A.parseAmount('€ 88,40'), 8840, 'parseAmount ignora il simbolo di valuta');
eq(A.parseAmount(''), 0, 'parseAmount stringa vuota');
eq(A.parseAmount('abc'), 0, 'parseAmount testo non numerico');
eq(A.parsePct('0,20'), 0.2, 'parsePct virgola');

/* --- saldo progressivo su righe unite, reali e previste --- */
const e = (date, cents, extra={}) => ({date, cents, desc:'x', cat:'', acct:'a', id:'i'+date+cents, ...extra});
const days = A.buildLed('2026-09-01', '2026-09-04', 10000, [
  e('2026-09-02', -2500),
  e('2026-09-02', 1000),
  e('2026-09-04', -500),
]);
eq(days.length, 4, 'buildLed copre ogni giorno dell intervallo');
eq(days.map(d => d.saldoFine), [10000, 8500, 8500, 8000], 'buildLed saldo di fine giornata');
eq(days[1].entries.map(x => x.saldo), [7500, 8500], 'buildLed saldo progressivo per riga (uscite prima)');
eq(days.map(d => d.empty), [true, false, true, false], 'buildLed marca i giorni vuoti');
eq(A.buildLed('2026-09-01','2026-09-02',0,[e('2026-08-31',-100)]).every(d=>d.empty), true,
   'buildLed scarta le righe fuori intervallo');

/* --- aggregazione: apertura sommata su piu conti --- */
const multi = A.buildLed('2026-09-01', '2026-09-02', 5000 + 3000, [
  {...e('2026-09-01', -1000), acct: 'a'},
  {...e('2026-09-01', 2000), acct: 'b'},
]);
eq(multi[0].saldoFine, 9000, 'aggregazione tutti i conti: apertura sommata');

/* --- periodi --- */
const r = A.periodRange('year');
eq([r.from.slice(4), r.to.slice(4)], ['-01-01', '-12-31'], 'periodRange anno corrente');
eq(A.prevRange({from:'2026-09-01', to:'2026-09-30'}), {from:'2026-08-02', to:'2026-08-31'},
   'prevRange stessa lunghezza, immediatamente prima');

/* --- date --- */
eq(A.addDays('2026-02-28', 1), '2026-03-01', 'addDays fine febbraio');
eq(A.addDays('2026-01-01', -1), '2025-12-31', 'addDays a cavallo d anno');
eq(A.fmtDate('2026-09-08'), '08/09/2026', 'fmtDate formato italiano');

/* --- ricorrenze: RRULE andata e ritorno --- */
eq(A.patternToForm('FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=5'), {kind:'monthly-day', skip:0}, 'patternToForm mensile');
eq(A.patternToForm('FREQ=WEEKLY;INTERVAL=3'), {kind:'weekly', skip:2}, 'patternToForm salti -> intervallo');
eq(A.patternToForm('FREQ=MONTHLY;INTERVAL=1;BYDAY=2TU'), {kind:'monthly-weekday', skip:0}, 'patternToForm giorno della settimana');
eq(A.patternToForm(''), {kind:'monthly-day', skip:0}, 'patternToForm valore assente');

/* --- CSV --- */
const csv = A.splitCSV('Data;Descrizione;Importo\n19/08/2026;"POS CONAD; SPESA";-42,18');
eq(csv.sep, ';', 'splitCSV rileva il separatore');
eq(csv.rows[1], ['19/08/2026', 'POS CONAD; SPESA', '-42,18'], 'splitCSV rispetta le virgolette');
eq(A.looksLikeDate('19/08/2026'), true, 'looksLikeDate gg/mm/aaaa');
eq(A.looksLikeDate('POS CONAD'), false, 'looksLikeDate testo');

/* --- riga del registro dal DTO --- */
const row = A.rowToEntry({id:'t1', account_id:'a2', date:'2099-01-01', cents:-100,
  description:null, category_id:'c1', pair_id:'p1', rec_id:null, running_balance_cents:0}, 'fallback');
eq([row.acct, row.pair, row.previsto], ['a2', 'p1', true], 'rowToEntry usa i campi nuovi del DTO');
eq(A.rowToEntry({id:'t2', date:'2020-01-01', cents:1, running_balance_cents:0}, 'fallback').acct, 'fallback',
   'rowToEntry ripiega sul conto interrogato');

/* --- selettore dei mesi: intervalli personalizzati --- */
eq(A.addMonths('2026-10', -2), '2026-08', 'addMonths indietro');
eq(A.addMonths('2026-11', 3), '2027-02', 'addMonths a cavallo d anno');
eq(A.addMonths('2026-01', -1), '2025-12', 'addMonths gennaio meno uno');
A.S.period = 'custom'; A.S.range = {k1: '2026-08', k2: '2026-10'};
eq(A.periodKeys('custom'), ['2026-08', '2026-10'], 'periodKeys custom');
eq(A.periodRange('custom'), {from: '2026-08-01', to: '2026-10-31'}, 'periodRange custom copre i mesi interi');
eq(A.monthLabelText().includes('–'), true, 'etichetta intervallo con trattino');
A.S.range = {k1: '2026-10', k2: '2026-10'};
eq(A.monthLabelText().includes('–'), false, 'etichetta mese singolo');

console.log(`\n${pass} superati, ${fail} falliti`);
process.exit(fail ? 1 : 0);
