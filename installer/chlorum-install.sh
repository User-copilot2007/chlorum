FLAKE=/etc/chlorum

# Needs root. Restart through sudo when started as a normal user.
if [ "$(id -u)" -ne 0 ]; then
  exec sudo "$0" "$@"
fi

ROOT=/mnt/disko-install-root

# Unmount and close whatever an earlier, interrupted run left behind.
cleanup_target() {
  umount -R "$ROOT" 2>/dev/null || true
  cryptsetup close chlorum 2>/dev/null || true
  rmdir "$ROOT" 2>/dev/null || true
}
cleanup_target
rm -f /tmp/chlorum-disk-password /tmp/chlorum-root.hash

# The drive this live system runs from. Never offered, never touched.
# The ISO file system can sit on a partition or on the whole drive.
boot_disks=()
add_boot_disk() {
  local dev parent
  dev=$(readlink -f "$1") || return 0
  [ -b "$dev" ] || return 0
  parent=$(lsblk -dno PKNAME "$dev" | head -n 1)
  if [ -z "$parent" ]; then
    parent=$(lsblk -dno NAME "$dev" | head -n 1)
  fi
  boot_disks+=("$parent")
}
if iso_src=$(findmnt -no SOURCE /iso 2>/dev/null); then
  add_boot_disk "$iso_src"
fi
if iso_label=$(blkid -L CHLORUM 2>/dev/null); then
  add_boot_disk "$iso_label"
fi
if [ "${#boot_disks[@]}" -eq 0 ] && mountpoint -q /iso; then
  echo "Cannot tell which drive Chlorum is running from. Refusing to install."
  exit 1
fi

is_boot_disk() {
  local b
  for b in "${boot_disks[@]}"; do
    [ "$b" = "$1" ] && return 0
  done
  return 1
}

# Whole disks only, without the boot drive, CD drives and RAM disks.
offered=()
while read -r n; do
  is_boot_disk "$n" || offered+=("$n")
done < <(lsblk -dn -e 7,11 -o NAME)

echo
echo "  CHLORUM INSTALLER"
echo
if [ "${#offered[@]}" -eq 0 ]; then
  echo "No disk to install on. Plug in the target drive and run chlorum-install again."
  exit 1
fi
echo "Disks you can install on:"
for n in "${offered[@]}"; do
  lsblk -dno NAME,SIZE,MODEL,TRAN "/dev/$n"
done
echo
read -rp "Disk to install on (for example sdb or nvme1n1): " name
ok=no
for n in "${offered[@]}"; do
  [ "$n" = "$name" ] && ok=yes
done
if [ "$ok" != yes ]; then
  echo "\"$name\" is not in the list above. Nothing was changed."
  exit 1
fi
disk="/dev/$name"

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
rc=0
disko-install \
  --flake "$FLAKE#chlorum" \
  --disk main "$disk" \
  --extra-files /tmp/chlorum-root.hash /persist/etc/chlorum/root.hash || rc=$?

# disko-install can report an error while cleaning up after a finished
# install. Trust what is on the disk, not only its exit code.
if [ "$rc" -ne 0 ]; then
  if [ -e "$ROOT/nix/var/nix/profiles/system" ] && [ -f "$ROOT/boot/loader/loader.conf" ]; then
    echo
    echo "The system is installed; only the installer's cleanup complained. Continuing."
  else
    echo
    echo "INSTALL FAILED. Diagnostics:"
    lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS "$disk" || true
    dmsetup ls || true
    findmnt -R /mnt || true
    cleanup_target
    shred -u /tmp/chlorum-disk-password /tmp/chlorum-root.hash 2>/dev/null || true
    exit 1
  fi
fi
cleanup_target
udevadm settle

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
