let currentStep = 1;
const TOTAL_STEPS = 3;

const pwInput = document.getElementById('wzPass');
const pwConfirm = document.getElementById('wzPass2');
const pwBar = document.getElementById('pwBar');
const pwLabel = document.getElementById('pwLabel');
const pwMatchHint = document.getElementById('pwMatchHint');

function pwScore(v){
  let s = 0;
  if(v.length >= 8) s++;
  if(v.length >= 12) s++;
  if(/[a-z]/.test(v) && /[A-Z]/.test(v)) s++;
  if(/\d/.test(v)) s++;
  if(/[^a-zA-Z0-9]/.test(v)) s++;
  return Math.min(s, 4);
}
const pwNames = ['','Debole','Media','Buona','Eccellente'];

pwInput.addEventListener('input', () => {
  const score = pwScore(pwInput.value);
  pwBar.className = 'bar' + (score ? ' w'+score : '');
  pwLabel.className = 'pw-label' + (score ? ' w'+score : '');
  pwLabel.textContent = score ? pwNames[score] : '';
  checkMatch();
});

function checkMatch(){
  if(pwConfirm.value && pwConfirm.value !== pwInput.value){
    pwMatchHint.style.display = 'block';
  } else {
    pwMatchHint.style.display = 'none';
  }
}
pwConfirm.addEventListener('input', checkMatch);

function validateStep(step){
  if(step === 1){
    const user = document.getElementById('wzUser').value.trim();
    const pass = pwInput.value;
    const pass2 = pwConfirm.value;
    const email = document.getElementById('wzEmail').value.trim();
    if(!user) return 'Inserisci il nome utente.';
    if(!pass) return 'Inserisci la password.';
    if(pass.length < 8) return 'La password deve avere almeno 8 caratteri.';
    if(pass !== pass2) return 'Le password non coincidono.';
    if(!email) return 'Inserisci l\'email dell\'amministratore.';
    if(!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) return 'Email non valida.';
  }
  return null;
}

function showError(msg){
  const el = document.createElement('div');
  el.className = 'wz-err';
  el.innerHTML = '<i class="bi bi-exclamation-triangle"></i> ' + msg;
  const panel = document.getElementById('panel' + currentStep);
  const existing = panel.querySelector('.wz-err');
  if(existing) existing.remove();
  panel.prepend(el);
  setTimeout(() => { if(el.parentNode) el.remove(); }, 5000);
}

function goNext(){
  const err = validateStep(currentStep);
  if(err){ showError(err); return; }

  if(currentStep < TOTAL_STEPS){
    document.getElementById('panel' + currentStep).classList.remove('is-on');
    currentStep++;
    const nextPanel = document.getElementById('panel' + currentStep);
    nextPanel.classList.remove('is-on');
    void nextPanel.offsetWidth;
    nextPanel.classList.add('is-on');
    if(currentStep === 3) populateReview();
    updateIndicator();
    updateButtons();
  } else {
    showEndDialog();
  }
}

function goBack(){
  if(currentStep <= 1) return;
  document.getElementById('panel' + currentStep).classList.remove('is-on');
  currentStep--;
  const prevPanel = document.getElementById('panel' + currentStep);
  prevPanel.classList.remove('is-on');
  void prevPanel.offsetWidth;
  prevPanel.classList.add('is-on');
  updateIndicator();
  updateButtons();
}

function updateIndicator(){
  document.querySelectorAll('.wz-step').forEach(el => {
    const s = parseInt(el.dataset.step);
    el.classList.remove('is-active','is-done');
    if(s < currentStep){
      el.classList.add('is-done');
      el.querySelector('.dot').innerHTML = '<i class="bi bi-check"></i>';
    } else if(s === currentStep){
      el.classList.add('is-active');
    } else {
      el.querySelector('.dot').innerHTML = '<span>' + s + '</span>';
    }
  });
  document.querySelectorAll('.wz-connector').forEach(el => {
    const c = parseInt(el.dataset.conn);
    el.classList.toggle('is-done', c < currentStep);
  });
}

function updateButtons(){
  const btnBack = document.getElementById('btnBack');
  const btnNext = document.getElementById('btnNext');
  btnBack.style.display = currentStep > 1 ? '' : 'none';
  if(currentStep === TOTAL_STEPS){
    btnNext.innerHTML = '<i class="bi bi-check-lg"></i> Termina setup';
  } else {
    btnNext.innerHTML = 'Continua <i class="bi bi-arrow-right"></i>';
  }
}

function populateReview(){
  document.getElementById('rvUser').textContent = document.getElementById('wzUser').value.trim();
  const pass = pwInput.value;
  document.getElementById('rvPass').textContent = '\u2022'.repeat(Math.min(pass.length, 12));
  document.getElementById('rvEmail').textContent = document.getElementById('wzEmail').value.trim();
  document.getElementById('rvAppName').textContent = document.getElementById('wzAppName').value.trim() || 'PFM';
  const langSel = document.getElementById('wzLang');
  document.getElementById('rvLang').textContent = langSel.options[langSel.selectedIndex].text;
  const curSel = document.getElementById('wzCurrency');
  document.getElementById('rvCurrency').textContent = curSel.options[curSel.selectedIndex].text;
}

function showEndDialog(){
  document.getElementById('endDialog').classList.add('is-on');
}
function closeEndDialog(){
  document.getElementById('endDialog').classList.remove('is-on');
}

async function finishSetup(){
  closeEndDialog();
  const btnNext = document.getElementById('btnNext');
  btnNext.disabled = true;
  btnNext.innerHTML = '<span class="spinner-border spinner-border-sm"></span> Invio…';

  try {
    const payload = {
      username: document.getElementById('wzUser').value.trim(),
      password: pwInput.value,
      email: document.getElementById('wzEmail').value.trim(),
      app_name: document.getElementById('wzAppName').value.trim() || 'PFM',
      locale: document.getElementById('wzLang').value,
      currency: document.getElementById('wzCurrency').value
    };

    const resp = await setupFetch('/api/setup/complete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });

    if(!resp.ok){
      // The body can only be read once: take the text, then try to parse it.
      const raw = await resp.text().catch(() => '');
      let errBody = {};
      try { errBody = JSON.parse(raw); } catch { /* plain-text error */ }
      throw new Error(errBody.detail || errBody.message ||
        (resp.status === 401 ? 'Token di setup mancante o errato' : raw) || 'Errore imprevisto');
    }

    // Show the "app shutting down" message.
    document.body.innerHTML = '<div style="min-height:100vh;display:flex;align-items:center;justify-content:center;background:var(--sidebar-bg);color:#fff;font-family:Inter,system-ui,sans-serif;font-size:1rem;opacity:.7">Setup completato. App in chiusura…</div>';
  } catch(e) {
    btnNext.disabled = false;
    btnNext.innerHTML = '<i class="bi bi-check-lg"></i> Termina setup';
    showError('Errore: ' + e.message);
  }
}

updateIndicator();
updateButtons();


// ─── Event wiring ───────────────────────────────────────────────────────────
// These used to be `onclick=` attributes in the HTML. Inline handlers are
// blocked by the Content-Security-Policy (`script-src 'self'`), which is now
// applied to HTML pages as well as to the API, so they are bound here instead.
document.addEventListener('DOMContentLoaded', function () {
  const bind = function (selector, handler) {
    const el = document.querySelector(selector);
    if (el) { el.addEventListener('click', handler); }
  };
  bind('#btnBack', goBack);
  bind('#btnNext', goNext);
  bind('#dgCancel', closeEndDialog);
  bind('#dgFinish', finishSetup);
});
