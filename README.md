# PFM — Personal Finance Manager

Registro finanziario personale, self-hosted, scritto in Rust. Un singolo
binario serve sia le API sia il frontend (SPA/PWA), con database SQLite o
MariaDB/MySQL.

L'interfaccia è disponibile in **italiano** e **inglese**.

## Funzionalità

- **Conti e categorie**: conti multipli, categorie e tag per utente, ordinamento personalizzato.
- **Movimenti e giroconti** tra conti, con registro e saldi calcolati.
- **Ricorrenze** (regole RRULE) con gestione del fine settimana e dei giorni festivi
  tramite calendari delle festività configurabili.
- **Carte di credito**: chiusura contabile e addebito automatico del ciclo sul conto di appoggio.
- **Conti deposito**: interessi giornalieri con ritenuta del 26 %.
- **Regole di spesa sui conti**: canoni, imposta di bollo e commissioni, con condizioni e tetti annui.
- **Budget** con avanzamento per periodo.
- **Import CSV** con profili salvabili e deduplicazione; **export CSV**.
- **Condivisione dei conti** tra utenti con livelli di permesso.
- **Multiutente con ruoli** (`admin` / `user`), impersonazione e audit log.
- **PWA** con coda offline per l'inserimento dei movimenti.
- **Scheduler** interno che riallinea regole e carte all'avvio e poi a intervalli regolari.

## Sicurezza

Hashing delle password con Argon2id, sessioni firmate, protezione CSRF
(double-submit), rate limiting sul login, header di sicurezza e CSP restrittiva,
gestione dei reverse proxy fidati. Il dettaglio è in
[docs/security-checklist.md](docs/security-checklist.md).

## Architettura

Workspace Cargo composto da crate a livelli:

| Crate | Ruolo |
|-------|-------|
| `pfm-domain` | Tipi e motori di calcolo puri (ricorrenze, carte, interessi, canoni, budget, permessi), senza I/O |
| `pfm-config` | Caricamento della configurazione (file TOML + variabili d'ambiente) |
| `pfm-i18n` | Cataloghi di traduzione lato server (`en`, `it`) |
| `pfm-repo` | Accesso al database (sqlx, SQLite e MariaDB) |
| `pfm-service` | Logica applicativa |
| `pfm-http` | Router axum, middleware (sessione, CSRF, rate limit, header) e route API |
| `pfm-app` | Eseguibile: avvio, migrazioni, wizard di setup, spegnimento ordinato |

Il frontend vive in [`web/`](web/) (JavaScript vanilla, Bootstrap e Chart.js
inclusi in `web/vendor/`). Le migrazioni SQL sono in [`migrations/`](migrations/)
e vengono eseguite all'avvio.

## Requisiti

- Toolchain Rust stabile (definita in `rust-toolchain.toml`, MSRV 1.75)
- Opzionali per `build.sh`: `node` (controlli e test del frontend) e `python`
  (controlli su id del DOM e parità dei cataloghi i18n)

## Avvio rapido

```bash
git clone <url-del-repository> pfm
cd pfm
cargo run --release -p pfm-app
```

Poi apri <http://127.0.0.1:9999>.

Al primo avvio, con database vuoto, l'applicazione mostra una pagina
«Configurazione richiesta». Avvia il wizard di setup:

```bash
cargo run --release -p pfm-app -- --setup
```

Il wizard è protetto da un **token monouso** stampato nel terminale: apri
`http://127.0.0.1:9999/#t=<TOKEN>` (il frammento dopo `#` non viaggia mai sulla
rete) e crea l'utente amministratore.

## Build

```bash
./build.sh        # Linux / macOS
./build.ps1       # Windows (PowerShell)
```

`build.sh` esegue i controlli sugli asset statici, `cargo fmt --check`,
`cargo clippy -- -D warnings` e la build release; l'eseguibile risultante è
`target/release/pfm-app`.

## Test

```bash
cargo test --workspace
node scripts/frontend_test.js
```

## Opzioni da riga di comando

| Opzione | Descrizione |
|---------|-------------|
| `--config <PATH>` | Usa un file di configurazione specifico |
| `--host <ADDR>` | Sovrascrive l'indirizzo di ascolto |
| `--port <PORT>` | Sovrascrive la porta |
| `--log-level <LEVEL>` | Sovrascrive il filtro di log (es. `debug`, `warn`) |
| `--migrate` | Esegue le migrazioni ed esce |
| `--setup` | Avvia il wizard di prima configurazione |
| `--help`, `--version` | Aiuto e versione |

## Configurazione

Copia [`config.example.toml`](config.example.toml) (commentato voce per voce)
in `config.toml`, che è escluso da git perché contiene il segreto di sessione:

```bash
cp config.example.toml config.toml
```

Precedenza (dalla più alta): **variabili d'ambiente** › file › valori predefiniti.
Un file diverso si indica con `PFM_CONFIG`.

Le variabili d'ambiente usano il prefisso `PFM_` e `__` come separatore:

```bash
PFM_SERVER__PORT=9000
PFM_DATABASE__ENGINE=mariadb
PFM_DATABASE__MARIADB__URL="mysql://user:pass@localhost:3306/pfm"
```

Sezioni principali: `[server]`, `[database]` (SQLite o MariaDB), `[i18n]`,
`[static]` (file da disco o incorporati nel binario), `[security]`, `[logging]`,
`[scheduler]`.

> **Importante:** imposta un `session_secret` casuale e *tuo* e non committarlo.
> Puoi generarlo con `openssl rand -hex 32` e inserirlo in `config.toml` oppure
> passarlo come `PFM_SECURITY__SESSION_SECRET`. Se lo lasci vuoto viene generata
> una chiave effimera e le sessioni non sopravvivono al riavvio.

## Dietro un reverse proxy

Con nginx o simili (vedi [`deploy/alpine/nginx-pfm.conf`](deploy/alpine/nginx-pfm.conf)):

```toml
[security]
trust_proxy     = true
trusted_proxies = ["<ip del proxy>"]   # non lasciarla vuota in produzione
secure_cookies  = true                  # cookie Secure + HSTS
```

Blocca con il firewall l'accesso diretto alla porta dell'applicazione: il
backend parla HTTP in chiaro.

## Deploy su Alpine Linux (LXC)

In [`deploy/alpine/`](deploy/alpine/) trovi uno script di build statica musl, il
packaging, l'installer, il servizio OpenRC, il backup automatico su Google
Drive tramite rclone e le istruzioni di ripristino. Leggi
[`deploy/alpine/README.txt`](deploy/alpine/README.txt) prima di installare o
aggiornare: alcune migrazioni sono irreversibili e conviene fare un backup.

## Dati

Con la configurazione predefinita il database SQLite è `data/pfm.db` (modalità
WAL: per copiarlo usa `sqlite3 data/pfm.db ".backup '/percorso/copia.db'"`, non
copiare il solo file `.db`). La cartella `data/` è esclusa da git.

## Licenza

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).

Se esponi una versione modificata del programma agli utenti in rete, la
sezione 13 della AGPL richiede di mettere a loro disposizione il codice
sorgente corrispondente.
