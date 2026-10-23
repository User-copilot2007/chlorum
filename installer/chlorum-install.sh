FLAKE=/etc/chlorum

# Needs root. Restart through sudo when started as a normal user.
if [ "$(id -u)" -ne 0 ]; then
  exec sudo "$0" "$@"
fi

# Leftovers from an earlier, interrupted run
rm -f /tmp/chlorum-disk-password /tmp/chlorum-root.hash

# The disk this live system runs from. Never offered, never touched.
boot_disk=""
if iso_src=$(findmnt -no SOURCE /iso 2>/dev/null); then
  boot_disk=$(lsblk -no PKNAME "$iso_src" | head -n 1)
fi

echo
echo "  CHLORUM INSTALLER"
echo
echo "Disks you can install on:"
lsblk -d -e 7,11 -o NAME,SIZE,MODEL,TRAN | grep -v "^${boot_disk:-NONE} "
echo
read -rp "Disk to install on (for example sdb or nvme1n1): " name
disk="/dev/$name"
if [ ! -b "$disk" ]; then
  echo "No such disk: $disk"
  exit 1
fi
if [ -n "$boot_disk" ] && [ "$name" = "$boot_disk" ]; then
  echo "$disk is the USB stick Chlorum is running from. Refusing."
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
echo "You will need both keys and their PINs, and paper for the recovery code."
echo
echo "Admin password (temporary console login until profiles exist)"
while true; do
  read -rsp "Admin password: " a1; echo
  read -rsp "Repeat: " a2; echo
  if [ -n "$a1" ] && [ "$a1" = "$a2" ]; then break; fi
  echo "Passwords do not match or are empty."
done
printf '%s' "$a1" | mkpasswd -m yescrypt -s > /tmp/chlorum-root.hash

# One-time random passphrase: only used to set the disk up, then wiped.
head -c 48 /dev/urandom | base64 -w0 > /tmp/chlorum-disk-password

echo
echo "Cleaning $disk"
lsblk -lnpo NAME "$disk" | while read -r dev; do
  umount -q "$dev" 2>/dev/null || true
  swapoff "$dev" 2>/dev/null || true
done
lsblk -lnpo NAME "$disk" | tac | xargs -r wipefs -af
sgdisk --zap-all "$disk"
blockdev --rereadpt "$disk" || true
udevadm settle

echo
echo "Installing. This downloads the system and takes a while."
if ! disko-install \
  --flake "$FLAKE#chlorum" \
  --disk main "$disk" \
  --extra-files /tmp/chlorum-root.hash /persist/etc/chlorum/root.hash; then
  echo
  echo "INSTALL FAILED. Diagnostics:"
  lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS "$disk" || true
  dmsetup ls || true
  findmnt -R /mnt || true
  exit 1
fi

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
echo "Remove the second key."

echo
echo "RECOVERY CODE. Write it on paper now and keep it away from this PC."
echo "Never photograph it or send it to anyone."
systemd-cryptenroll "$part" \
  --unlock-key-file=/tmp/chlorum-disk-password \
  --recovery-key

echo
read -rp "Press Enter after you have written the code down. " _

echo
echo "Removing the one-time setup passphrase."
systemd-cryptenroll "$part" \
  --unlock-key-file=/tmp/chlorum-disk-password \
  --wipe-slot=password

shred -u /tmp/chlorum-disk-password /tmp/chlorum-root.hash
echo
echo "Done. The disk now opens with either key or the recovery code."
echo "Remove the USB stick and reboot."
