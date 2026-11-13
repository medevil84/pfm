#!/bin/sh
# install.sh - Install pfm service on Alpine Linux (OpenRC)
# Run as root from extracted tarball directory
# Usage: ./install.sh              (oppure: PFM_IMPORT_DB=/path/pfm.db ./install.sh)

set -eu

# Check root
if [ "$(id -u)" -ne 0 ]; then
  echo "Errore: questo script deve essere eseguito come root" >&2
  exit 1
fi

echo "=== Installazione di PFM Service ==="

# Check Alpine/OpenRC
if ! command -v apk > /dev/null 2>&1; then
  echo "Errore: apk non trovato - questo non sembra essere Alpine Linux" >&2
  exit 1
fi

if ! command -v rc-update > /dev/null 2>&1; then
  echo "Errore: rc-update non trovato - OpenRC non sembra installato" >&2
  exit 1
fi

# Check required files
if [ ! -f "./pfm-app" ]; then
  echo "Errore: ./pfm-app non trovato" >&2
  exit 1
fi

if [ ! -d "./files" ]; then
  echo "Errore: ./files directory non trovata" >&2
  exit 1
fi

echo "Controllo pre-installazione: OK"
echo ""

# Install dependencies
echo "Installazione dipendenze..."
if ! apk add --no-cache sqlite ca-certificates tzdata; then
  echo "Errore: impossibile installare sqlite o ca-certificates" >&2
  exit 1
fi

# Try to install rclone from community repo
if ! apk add --no-cache rclone; then
  echo "Avviso: rclone non installato (repository 'community' potrebbe non essere abilitato)"
  echo "         Abilitare /etc/apk/repositories per il repository 'community' per i backup su Google Drive"
fi

# Create user and group
echo "Configurazione utente e gruppo..."
if ! grep -q '^pfm:' /etc/group; then
  addgroup -S pfm
  echo "Gruppo 'pfm' creato"
fi

if ! grep -q '^pfm:' /etc/passwd; then
  adduser -S -D -H -s /sbin/nologin -h /var/lib/pfm -G pfm pfm
  echo "Utente 'pfm' creato"
fi

# Prima di tutto: e' una installazione nuova? (il servizio crea il DB al primo avvio)
FRESH=1
[ -f /var/lib/pfm/pfm.db ] && FRESH=0

# Stop service if running (upgrade scenario)
if rc-service pfm status > /dev/null 2>&1; then
  echo "Arresto del servizio pfm..."
  rc-service pfm stop || true
fi

# Create directories with proper ownership and permissions
echo "Creazione directory..."
install -d -m 0750 -o root -g pfm /etc/pfm
install -d -m 0750 -o pfm -g pfm /var/lib/pfm
install -d -m 0750 -o pfm -g pfm /var/backups/pfm

install -d -m 0755 -o root -g root /usr/local/bin /usr/local/sbin

# Install binary
echo "Installazione binario..."
install -m 0755 -o root -g root ./pfm-app /usr/local/bin/pfm-app

# Install OpenRC init script
echo "Installazione script di init..."
install -m 0755 -o root -g root ./files/etc/init.d/pfm /etc/init.d/pfm

# Install conf.d config only if not present (preserve existing)
if [ ! -f /etc/conf.d/pfm ]; then
  install -m 0644 -o root -g root ./files/etc/conf.d/pfm /etc/conf.d/pfm
  echo "Configurazione ambiente creata: /etc/conf.d/pfm"
else
  echo "Configurazione ambiente già presente: /etc/conf.d/pfm (mantenuta)"
fi

# Install sbin helpers
echo "Installazione helper script..."
install -m 0755 -o root -g root ./files/usr/local/sbin/pfm-setup /usr/local/sbin/pfm-setup
install -m 0755 -o root -g root ./files/usr/local/sbin/pfm-backup /usr/local/sbin/pfm-backup
install -m 0755 -o root -g root ./files/usr/local/sbin/pfm-rclone-setup /usr/local/sbin/pfm-rclone-setup

# Install backup config only if not present
if [ ! -f /etc/pfm/backup.conf ]; then
  install -m 0644 -o root -g root ./files/etc/pfm/backup.conf /etc/pfm/backup.conf
else
  echo "Configurazione backup già presente: /etc/pfm/backup.conf (mantenuta)"
fi

# Create config.toml from template if not present
if [ ! -f /etc/pfm/config.toml ]; then
  echo "Creazione configurazione principale..."
  SESSION_SECRET=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  sed "s|@SESSION_SECRET@|$SESSION_SECRET|g" ./files/etc/pfm/config.toml.template > /etc/pfm/config.toml
  chown root:pfm /etc/pfm/config.toml
  chmod 0640 /etc/pfm/config.toml
  echo "Configurazione creata con secret di sessione casuale: /etc/pfm/config.toml"
else
  echo "Configurazione principale già presente: /etc/pfm/config.toml (mantenuta)"
  cp ./files/etc/pfm/config.toml.template /etc/pfm/config.toml.new
  echo "Nota: template aggiornato salvato per confronto in /etc/pfm/config.toml.new"
fi

# Handle optional database import
if [ -n "${PFM_IMPORT_DB:-}" ]; then
  if [ ! -f "$PFM_IMPORT_DB" ]; then
    echo "Errore: PFM_IMPORT_DB specificato ma file non trovato: $PFM_IMPORT_DB" >&2
    exit 1
  fi
  if [ -f /var/lib/pfm/pfm.db ]; then
    echo "Nota: /var/lib/pfm/pfm.db esiste già - import saltato"
  else
    echo "Import database da $PFM_IMPORT_DB..."
    install -m 0640 -o pfm -g pfm "$PFM_IMPORT_DB" /var/lib/pfm/pfm.db
    echo "Database importato: /var/lib/pfm/pfm.db"
    FRESH=0
  fi
fi

# Install cron symlink for daily backups (run-parts skips files with dots)
echo "Installazione pianificazione backup..."
install -d -m 0755 -o root -g root /etc/periodic/daily
# (wrapper e non symlink: nessun dubbio su come run-parts tratti i link)
printf '#!/bin/sh\nexec /usr/local/sbin/pfm-backup\n' > /etc/periodic/daily/pfm-backup
chmod 755 /etc/periodic/daily/pfm-backup

# Enable and start services
echo "Abilitazione servizi..."
rc-update add pfm default || true
rc-update add crond default || true
if ! rc-service crond status > /dev/null 2>&1; then
  rc-service crond start
fi

# Start pfm service
echo "Avvio servizio pfm..."
rc-service pfm restart || rc-service pfm start

# Check service status
sleep 3
if rc-service pfm status > /dev/null 2>&1; then
  echo "Stato servizio: ATTIVO"
else
  echo "Avviso: lo stato del servizio potrebbe non essere disponibile subito"
fi

# Get local IP
LOCAL_IP=$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -n1)
if [ -z "$LOCAL_IP" ]; then
  LOCAL_IP="localhost"
fi

echo ""
echo "=== Installazione Completata ==="
echo ""
echo "URL di accesso: http://$LOCAL_IP:9999/"
echo ""
echo "Prossimi passi:"

# Installazione nuova senza DB importato: serve il wizard
if [ "$FRESH" -eq 1 ]; then
  echo "1. Eseguire la procedura di configurazione iniziale:"
  echo "   pfm-setup"
fi

echo "2. Per abilitare backup su Google Drive (verifica cron: run-parts --test /etc/periodic/daily):"
echo "   pfm-rclone-setup"
echo "   Quindi testare con: pfm-backup"
echo ""
echo "3. Configurazione: /etc/pfm/config.toml"
echo ""
echo "4. Log del servizio:"
echo "   logread | grep pfm"
echo ""
