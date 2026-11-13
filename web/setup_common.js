// Shared by the setup wizard and the reset page.
//
// Every /api/setup/* call needs the one-time token the server printed on its
// terminal. It normally arrives in the URL fragment (…/#t=TOKEN): a fragment is
// never sent to the server nor written to access logs. It is kept in
// sessionStorage so the reset page → wizard hop keeps working, and stripped
// from the address bar. If it is missing the operator is asked for it.

let SETUP_TOKEN = '';

(function initSetupToken() {
  const m = /[#&]t=([A-Za-z0-9_-]+)/.exec(window.location.hash);
  if (m) {
    SETUP_TOKEN = m[1];
    try { sessionStorage.setItem('pfm_setup_token', SETUP_TOKEN); } catch { /* storage unavailable */ }
    history.replaceState(null, '', window.location.pathname + window.location.search);
    return;
  }
  try { SETUP_TOKEN = sessionStorage.getItem('pfm_setup_token') || ''; } catch { /* storage unavailable */ }
})();

async function setupFetch(url, options = {}) {
  if (!SETUP_TOKEN) {
    SETUP_TOKEN = (window.prompt('Token di setup (mostrato nel terminale dove gira pfm-app --setup):') || '').trim();
    try { sessionStorage.setItem('pfm_setup_token', SETUP_TOKEN); } catch { /* storage unavailable */ }
  }
  const headers = Object.assign({}, options.headers, { 'X-Setup-Token': SETUP_TOKEN });
  const resp = await fetch(url, Object.assign({}, options, { headers }));
  if (resp.status === 401) {
    // Wrong / stale token: forget it so the next attempt asks again.
    SETUP_TOKEN = '';
    try { sessionStorage.removeItem('pfm_setup_token'); } catch { /* storage unavailable */ }
  }
  return resp;
}
