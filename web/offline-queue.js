/**
 * PFM Offline Queue
 *
 * Stores pending API mutations in localStorage when the network is unavailable.
 * Each queued item carries a client-generated UUID (client_id) that the server
 * uses for idempotency: if the same client_id is sent twice, the server returns
 * the existing row instead of creating a duplicate.
 *
 * Usage:
 *   const queue = new OfflineQueue();
 *   const result = await queue.post('/api/transactions', body);
 *   // If offline, the mutation is queued and will sync on reconnect.
 *   // If online, it is sent immediately.
 */

class OfflineQueue {
  constructor(storageKey = 'pfm_offline_queue', apiBase = '/api') {
    this.storageKey = storageKey;
    // Every API route is mounted under `/api`. Callers pass logical paths
    // ('/transactions'), so the prefix is applied here — including for items
    // already sitting in localStorage from a previous version, which were
    // stored without it.
    this.apiBase = apiBase;
    this.listeners = [];
    // Id of the signed-in user. Every queued item is stamped with it and only
    // that user's items are replayed: the queue lives in localStorage and
    // survives logout, so without this a movement queued by one account would
    // be sent under whoever signs in next on the same browser.
    this.userId = null;
  }

  // ── Public API ──────────────────────────────────────────────────────────────

  /**
   * Generate a new client UUID for idempotent sync.
   * Uses crypto.randomUUID() when available, falls back to a manual v4.
   */
  static generateClientId() {
    if (typeof crypto !== 'undefined' && crypto.randomUUID) {
      return crypto.randomUUID();
    }
    // Fallback: manual v4 UUID
    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
      const r = (Math.random() * 16) | 0;
      const v = c === 'x' ? r : (r & 0x3) | 0x8;
      return v.toString(16);
    });
  }

  /**
   * Attempt to send a POST request. If the network is unavailable, queue
   * the mutation for later sync and return a queued result.
   *
   * @param {string} url - The API endpoint path (e.g. '/api/transactions').
   * @param {object} body - The JSON request body.
   * @param {string} [client_id] - Pre-generated client id (auto-generated if omitted).
   * @returns {Promise<{status: number, body: object, queued: boolean}>}
   */
  async post(url, body, client_id) {
    const cid = client_id || OfflineQueue.generateClientId();

    // Inject client_id into the body for idempotency.
    const payload = { ...body, client_id: cid };

    // Try sending immediately.
    const result = await this._fetch(url, payload);

    if (result.success) {
      return { status: result.status, body: result.body, queued: false };
    }

    // Network failure: queue for later.
    this._enqueue(url, payload, cid);
    this._notifyListeners('queued', { url, client_id: cid });
    return { status: 0, body: { queued: true, client_id: cid }, queued: true };
  }

  /**
   * Check if the network is currently available.
   * @returns {boolean}
   */
  get online() {
    return typeof navigator !== 'undefined' ? navigator.onLine : true;
  }

  /**
   * Get all pending items in the queue.
   * @returns {Array<{url: string, body: object, client_id: string, queued_at: string}>}
   */
  get pending() {
    return this._loadQueue().filter((item) => this._isMine(item));
  }

  /**
   * Replay all queued mutations to the server. Called on reconnect.
   * Successfully sent items are removed from the queue.
   * @returns {Promise<{sent: number, failed: number}>}
   */
  async sync() {
    // Nobody signed in: nothing may be replayed.
    if (!this.userId) return { sent: 0, failed: 0 };
    const queue = this._loadQueue();
    let sent = 0;
    let failed = 0;
    const remaining = [];

    for (const item of queue) {
      // Items of other users stay in storage, untouched, for when they return.
      if (item.user_id && item.user_id !== this.userId) {
        remaining.push(item);
        continue;
      }
      const result = await this._fetch(item.url, item.body);
      if (result.success) {
        sent++;
        this._notifyListeners('synced', { url: item.url, client_id: item.client_id });
      } else {
        failed++;
        remaining.push(item);
      }
    }

    this._saveQueue(remaining);

    if (sent > 0) {
      this._notifyListeners('sync-complete', { sent, failed });
    }
    return { sent, failed };
  }

  /**
   * Register a callback for queue events ('queued', 'synced', 'sync-complete').
   * @param {string} event
   * @param {function} callback
   */
  on(event, callback) {
    this.listeners.push({ event, callback });
  }

  // ── Internals ───────────────────────────────────────────────────────────────

  /** Whether a queued item belongs to the signed-in user. Items stored by an
   *  older version carry no owner and are claimed by whoever syncs first. */
  _isMine(item) {
    return !!this.userId && (!item.user_id || item.user_id === this.userId);
  }

  /** Prefix a logical path with the API mount point, idempotently. */
  _resolve(url) {
    if (/^https?:\/\//i.test(url)) return url;
    if (url.startsWith(this.apiBase + '/') || url === this.apiBase) return url;
    return this.apiBase + url;
  }

  async _fetch(url, body) {
    try {
      const headers = { 'Content-Type': 'application/json' };

      // Read CSRF token from the cookie (double-submit pattern).
      const csrf = this._getCookie('pfm_csrf');
      if (csrf) {
        headers['X-CSRF-Token'] = csrf;
      }

      const response = await fetch(this._resolve(url), {
        method: 'POST',
        headers,
        credentials: 'same-origin',
        body: JSON.stringify(body),
      });

      // A body that is not JSON must never count as a delivered movement.
      // Previously a parse failure was swallowed into `{}` and `response.ok`
      // alone decided the outcome, so the SPA fallback's `index.html` — served
      // with status 200 — was recorded as a successful sync and the queued
      // item was discarded. Every movement saved offline was lost silently.
      let data;
      let parsed = true;
      try {
        data = await response.json();
      } catch (err) {
        parsed = false;
        data = {};
      }

      if (response.ok && parsed) {
        return { success: true, status: response.status, body: data };
      }

      if (response.ok && !parsed) {
        return {
          success: false,
          status: response.status,
          body: { error: 'non-JSON response from ' + this._resolve(url) },
        };
      }

      // 503 from service worker means offline
      if (response.status === 503) {
        return { success: false, status: 503, body: data };
      }

      // Server returned an error (validation, auth, etc.) - don't queue.
      return { success: false, status: response.status, body: data };
    } catch (err) {
      // Network error (TypeError from fetch)
      return { success: false, status: 0, body: { error: String(err) } };
    }
  }

  _getCookie(name) {
    if (typeof document === 'undefined') return null;
    const match = document.cookie
      .split(';')
      .map((c) => c.trim())
      .find((c) => c.startsWith(name + '='));
    return match ? match.substring(name.length + 1) : null;
  }

  _loadQueue() {
    try {
      const raw = localStorage.getItem(this.storageKey);
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  }

  _saveQueue(items) {
    try {
      localStorage.setItem(this.storageKey, JSON.stringify(items));
    } catch {
      // Storage full or unavailable - ignore.
    }
  }

  _enqueue(url, body, client_id) {
    const queue = this._loadQueue();
    // Avoid duplicates: if same client_id already queued, skip.
    if (queue.some((item) => item.client_id === client_id)) {
      return;
    }
    queue.push({ url, body, client_id, user_id: this.userId, queued_at: new Date().toISOString() });
    this._saveQueue(queue);
  }

  _notifyListeners(event, data) {
    for (const { event: e, callback } of this.listeners) {
      if (e === event) {
        try {
          callback(data);
        } catch {
          // Listener error should not break the queue.
        }
      }
    }
  }
}

// ── Auto-sync on reconnect ──────────────────────────────────────────────────
// When the browser comes back online, replay the queue automatically.
if (typeof window !== 'undefined') {
  const queue = new OfflineQueue();
  window.pfmOfflineQueue = queue;

  window.addEventListener('online', () => {
    const pending = queue.pending;
    if (pending.length > 0) {
      queue.sync().then(() => {
        // Notify the app that data may have changed.
        window.dispatchEvent(new CustomEvent('pfm:offline-synced', { detail: { count: pending.length } }));
      });
    }
  });
}

// Export for module systems
if (typeof module !== 'undefined' && module.exports) {
  module.exports = OfflineQueue;
}