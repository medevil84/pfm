// Reset confirmation page.
//
// Extracted from an inline <script> block: the Content-Security-Policy
// (`script-src 'self'`) now covers HTML pages as well as the API, so inline
// scripts and `onclick=` attributes no longer execute.
//
// CONTEXT.md requires an *explicit* double confirmation before a destructive
// reset. The previous implementation was a double click on the same button
// with a 4-second timeout, which is one gesture repeated, not two deliberate
// decisions — and a mistimed second click silently reverted to the initial
// state. It is now an acknowledgement checkbox that arms the button, plus the
// destructive click itself.

const ack = document.getElementById('ackReset');
const btnConfirm = document.getElementById('btnConfirm');
const btnCancel = document.getElementById('btnCancel');

function syncArmedState() {
  btnConfirm.disabled = !ack.checked;
}

async function confirmReset() {
  if (!ack.checked) {
    return;
  }

  btnConfirm.disabled = true;
  ack.disabled = true;
  btnCancel.disabled = true;
  btnConfirm.innerHTML =
    '<span class="spinner-border spinner-border-sm"></span> Reset in corso…';

  try {
    const resp = await setupFetch('/api/setup/reset', { method: 'POST' });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.detail || err.message ||
        (resp.status === 401 ? 'Token di setup mancante o errato' : 'Errore durante il reset'));
    }
    // The backend has emptied the application tables and removed the
    // `initialized` marker; the wizard can now run from a clean state.
    window.location.href = '/setup_wizard.html';
  } catch (e) {
    ack.disabled = false;
    btnCancel.disabled = false;
    ack.checked = false;
    syncArmedState();
    btnConfirm.innerHTML = '<i class="bi bi-trash"></i> Conferma reset';
    alert('Errore: ' + e.message);
  }
}

async function cancelSetup() {
  // Cancelling has to terminate the process. The previous version only
  // repainted the page and asked the user to close the window, on the stated
  // assumption that the server would exit when the client disconnected — it
  // does not. The process stayed alive in setup mode with /api/setup/reset
  // reachable without authentication.
  btnCancel.disabled = true;
  btnConfirm.disabled = true;

  try {
    await setupFetch('/api/setup/cancel', { method: 'POST' });
  } catch (e) {
    // The process may close the connection while shutting down, which is the
    // expected outcome rather than an error worth reporting.
  }

  document.body.innerHTML =
    '<div style="min-height:100vh;display:flex;align-items:center;' +
    'justify-content:center;background:var(--sidebar-bg);color:#fff;' +
    'font-family:Inter,system-ui,sans-serif;font-size:1rem;opacity:.7">' +
    'Setup annullato. L\'applicazione è in chiusura.</div>';
}

ack.addEventListener('change', syncArmedState);
btnConfirm.addEventListener('click', confirmReset);
btnCancel.addEventListener('click', cancelSetup);
syncArmedState();
