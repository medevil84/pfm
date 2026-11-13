PFM su Alpine Linux (LXC) - pacchetto musl statico + OpenRC
===========================================================

AGGIORNAMENTO DA UNA VERSIONE PRECEDENTE (leggi prima)
  1. Fai un backup del DB: la migrazione 0020 (categorie/tag per utente) e'
     irreversibile. Es.:  pfm-backup
  2. Ricostruisci il pacchetto (build-musl.sh + package.sh): il tarball in dist/
     e' quello vecchio.
  3. Modifica a mano /etc/pfm/config.toml (l'installer non lo sovrascrive):
     trust_proxy, trusted_proxies, secure_cookies (vedi GESTIONE). Senza
     trust_proxy dietro nginx tutti i client condividono il limite di login
     (5 tentativi/minuto in totale) e un amministratore puo' restare fuori.
  4. Dopo l'aggiornamento ricarica la pagina con Ctrl+F5: i file statici hanno
     max-age di un giorno e il vecchio app.js / le vecchie pagine di setup
     non conoscono il token.

INSTALLAZIONE (nell'LXC, come root)
  scp pfm-<ver>-x86_64-musl.tar.gz root@<lxc>:/root/
  tar xzf pfm-*.tar.gz && cd pfm-*-musl
  sh ./install.sh                         # installazione nuova
  PFM_IMPORT_DB=/root/pfm.db sh ./install.sh   # importa il DB esistente (vedi sotto)

  L'installer: installa sqlite/rclone/ca-certificates (apk), crea l'utente di
  sistema "pfm", copia il binario in /usr/local/bin, genera /etc/pfm/config.toml
  con un session_secret casuale, installa il servizio OpenRC "pfm", lo abilita
  all'avvio (rc-update add pfm default) e lo avvia. E' rieseguibile per gli
  aggiornamenti: config, conf.d e backup.conf esistenti NON vengono sovrascritti.

PRIMO AVVIO
  Con DB vuoto il servizio mostra solo una pagina "Configurazione richiesta".
  Lancia   pfm-setup   e completa il wizard dal browser: il comando ferma il
  servizio, esegue "pfm-app --setup" e lo riavvia. Il wizard e' protetto da un
  TOKEN monouso che compare nel terminale ("SETUP TOKEN"): apri
  http://<ip>:9999/#t=<TOKEN> (il frammento #t=... non viaggia mai sulla rete).
  In modalita' setup l'API normale non e' attiva e /api/setup/* rifiuta ogni
  richiesta senza il token. Se hai importato un DB gia' configurato il wizard
  NON serve.

DOVE VENGONO LETTI/SCRITTI I DATI
  Percorsi relativi (data/pfm.db, web, config.toml) sono risolti rispetto alla
  directory di lavoro del processo ($PWD). Nell'LXC il servizio parte da
  /var/lib/pfm e il config usa percorsi assoluti:
     database SQLite : /var/lib/pfm/pfm.db   (+ pfm.db-wal / pfm.db-shm, modalita' WAL)
     configurazione  : /etc/pfm/config.toml  (root:pfm 0640, contiene il session_secret)
     override env    : /etc/conf.d/pfm       (variabili PFM_*)
     backup locali   : /var/backups/pfm
     frontend (web/) : incorporato nel binario (static.source = "embedded")
     migrazioni      : incorporate nel binario (eseguite all'avvio)

  Copiare il DB dalla macchina attuale (NON copiare il solo pfm.db: c'e' il WAL):
     sqlite3 data/pfm.db ".backup '/tmp/pfm.db'"
     scp /tmp/pfm.db root@<lxc>:/root/pfm.db
     # poi nell'LXC:  PFM_IMPORT_DB=/root/pfm.db sh ./install.sh

BACKUP AUTOMATICO SU GOOGLE DRIVE
  Cosa fa /usr/local/sbin/pfm-backup (lanciato ogni giorno da crond tramite
  /etc/periodic/daily/pfm-backup, ~02:00):
    sqlite3 .backup (copia consistente a servizio attivo) -> integrity_check
    -> gzip in /var/backups/pfm (ultimi 14) -> rclone copy su Drive
    -> cancella da Drive i file piu' vecchi di 60 giorni.
  Impostazioni: /etc/pfm/backup.conf (cartella remota, retention, on/off).

  Configurazione di Drive (una volta sola; l'LXC non ha browser):
   1. Su un PC con browser e rclone:
        rclone authorize "drive" --drive-scope drive.file
      (scope drive.file = rclone vede solo i file che crea lui). Copia la riga
      JSON {"access_token":...} che stampa.
   2. Nell'LXC:   pfm-rclone-setup     e incolla il token.
   3. Prova:      pfm-backup           poi controlla la cartella "pfm-backups" su Drive.
  Log: logread | grep pfm-backup
  Frequenza diversa: sposta il link da /etc/periodic/daily a /etc/periodic/hourly
  (o 15min), oppure usa "crontab -e".
  Suggerimento: sono dati finanziari; per cifrarli su Drive crea un remote rclone
  di tipo "crypt" sopra "gdrive:" e metti RCLONE_REMOTE="nomecrypt:" in backup.conf.

  RIPRISTINO:
     rc-service pfm stop
     gunzip -c /var/backups/pfm/pfm-AAAAMMGG-HHMMSS.db.gz > /var/lib/pfm/pfm.db
     rm -f /var/lib/pfm/pfm.db-wal /var/lib/pfm/pfm.db-shm
     chown pfm:pfm /var/lib/pfm/pfm.db && rc-service pfm start

GESTIONE
  rc-service pfm start|stop|restart|status     logread | grep pfm
  Accesso da browser: http://<ip-lxc>:9999/ (HTTP in chiaro). Dietro un reverse
  proxy TLS (vedi nginx-pfm.conf) metti in config.toml:
     trust_proxy     = true
     trusted_proxies = ["<ip del proxy>"]     # OBBLIGATORIO: lista vuota = si fida
                                              # di X-Forwarded-For da chiunque
     secure_cookies  = true                   # cookie Secure + HSTS
  e BLOCCA con il firewall la porta 9999 a tutti tranne il proxy: raggiungibile
  direttamente, il backend e' HTTP in chiaro e aggira nginx. Se il proxy e' sulla
  stessa macchina usa host="127.0.0.1". NON attivare secure_cookies finche'
  apri ancora http://<ip>:9999/ in chiaro: il browser scarterebbe i cookie.
  All'avvio il log segnala le configurazioni rischiose (logread | grep pfm).

  CATEGORIE E TAG: da questa versione appartengono all'utente che le crea; quelle
  gia' presenti diventano "condivise" e le modifica solo un amministratore.

DISINSTALLAZIONE
  sh ./uninstall.sh            # tiene dati, config e utente
  sh ./uninstall.sh --purge    # cancella anche /var/lib/pfm, /etc/pfm, backup, utente
