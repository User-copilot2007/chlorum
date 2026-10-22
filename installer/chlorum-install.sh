FLAKE=/etc/chlorum

echo
echo "  CHLORUM INSTALLER"
echo
lsblk -d -o NAME,SIZE,MODEL,TRAN
echo
read -rp "Disk to install on (for example sdb or nvme1n1): " name
disk="/dev/$name"
if [ ! -b "$disk" ]; then
  echo "No such disk: $disk"
  exit 1
fi

echo
echo "EVERYTHING ON $disk ($(lsblk -dno SIZE,MODEL "$disk")) WILL BE ERASED."
read -rp "Type the disk name again to confirm: " again
if [ "$again" != "$name" ]; then
  echo "Cancelled."
  exit 1
fi

umask 077

echo
echo "Disk password (temporary safety net while Chlorum is in development)"
while true; do
  read -rsp "Disk password: " p1; echo
  read -rsp "Repeat: " p2; echo
  if [ -n "$p1" ] && [ "$p1" = "$p2" ]; then break; fi
  echo "Passwords do not match or are empty."
done
printf '%s' "$p1" > /tmp/chlorum-disk-password

echo
echo "Admin password (temporary console login until profiles exist)"
while true; do
  read -rsp "Admin password: " a1; echo
  read -rsp "Repeat: " a2; echo
  if [ -n "$a1" ] && [ "$a1" = "$a2" ]; then break; fi
  echo "Passwords do not match or are empty."
done
printf '%s' "$a1" | mkpasswd -m yescrypt -s > /tmp/chlorum-root.hash

echo
echo "Installing. This downloads the system and takes a while."
disko-install \
  --flake "$FLAKE#chlorum" \
  --disk main "$disk" \
  --extra-files /tmp/chlorum-root.hash /persist/etc/chlorum/root.hash

part=/dev/disk/by-partlabel/disk-main-system

enroll_key() {
  echo
  echo "Insert your $1 key and press Enter. Then enter its PIN and touch it."
  read -r _
  systemd-cryptenroll "$part" \
    --unlock-key-file=/tmp/chlorum-disk-password \
    --fido2-device=auto \
    --fido2-with-client-pin=yes
}

enroll_key "FIRST"
echo "Remove the first key."
enroll_key "SECOND"

echo
echo "RECOVERY CODE. Write it on paper now and keep it away from this PC."
systemd-cryptenroll "$part" \
  --unlock-key-file=/tmp/chlorum-disk-password \
  --recovery-key

shred -u /tmp/chlorum-disk-password /tmp/chlorum-root.hash
echo
echo "Done. Remove the USB stick and reboot."
