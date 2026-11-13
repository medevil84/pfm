#!/bin/sh
# uninstall.sh - Remove pfm service from Alpine Linux
# Run as root
# Usage: sudo ./uninstall.sh [--purge]

set -eu

# Check root
if [ "$(id -u)" -ne 0 ]; then
  echo "Errore: questo script deve essere eseguito come root" >&2
  exit 1
fi

PURGE=0
if [ "${1:-}" = "--purge" ]; then
  PURGE=1
fi

echo "=== Rimozione di PFM Service ==="
echo ""

# Stop service
echo "Arresto servizio..."
if rc-service pfm status > /dev/null 2>&1; then
  rc-service pfm stop || true
fi

# Remove from default runlevel
echo "Disabilitazione servizio..."
rc-update del pfm default || true

# Remove service files
echo "Rimozione file di servizio..."
rm -f /etc/init.d/pfm /usr/local/bin/pfm-app
rm -f /usr/local/sbin/pfm-setup /usr/local/sbin/pfm-backup /usr/local/sbin/pfm-rclone-setup
rm -f /etc/periodic/daily/pfm-backup

if [ "$PURGE" -eq 0 ]; then
  echo ""
  echo "=== Rimozione Completata ==="
  echo ""
  echo "Dati e configurazione mantenuti:"
  echo "  - Database: /var/lib/pfm/"
  echo "  - Backup: /var/backups/pfm/"
  echo "  - Configurazione: /etc/pfm/"
  echo "  - Variabili ambiente: /etc/conf.d/pfm"
  echo "  - Utente/Gruppo pfm: mantenuti"
  echo ""
  echo "Per rimuovere tutto (dati inclusi), eseguire:"
  echo "  sudo ./uninstall.sh --purge"
else
  # Ask for confirmation
  echo ""
  echo "Attenzione: --purge rimuoverà TUTTI i dati, backup e configurazione!"
  echo ""
  printf "Sei sicuro? Scrivi 'si' per confermare: "
  read -r CONFIRM || CONFIRM=""

  if [ "$CONFIRM" = "si" ]; then
    echo "Rimozione dati e configurazione..."
    rm -rf /var/lib/pfm /var/backups/pfm /etc/pfm /etc/conf.d/pfm

    echo "Rimozione utente e gruppo..."
    deluser pfm 2>/dev/null || true
    delgroup pfm 2>/dev/null || true

    echo ""
    echo "=== Purge Completato ==="
    echo ""
    echo "Tutti i dati, backup, configurazione e utente pfm sono stati rimossi."
  else
    echo "Operazione annullata - nessun dato rimosso."
    exit 1
  fi
fi
