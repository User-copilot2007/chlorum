FLAKE=/etc/chlorum
ROOT=/mnt/disko-install-root

# Needs root. Restart through sudo when started as a normal user.
if [ "$(id -u)" -ne 0 ]; then
  exec sudo "$0" "$@"
fi

# Unmount and close whatever an earlier, interrupted run left behind.
cleanup_target() {
  umount -R "$ROOT" 2>/dev/null || true
  cryptsetup close chlorum 2>/dev/null || true
  rmdir "$ROOT" 2>/dev/null || true
}
cleanup_target
rm -f /tmp/chlorum-disk-password

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

if [ "${#offered[@]}" -eq 1 ]; then
  name="${offered[0]}"
else
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
fi
disk="/dev/$name"

echo
echo "Install Chlorum on $name ($(lsblk -dno SIZE,MODEL "$disk" | xargs))?"
echo "EVERYTHING ON IT WILL BE ERASED."
read -rp "Type yes to install: " answer
if [ "$answer" != yes ]; then
  echo "Cancelled. Nothing was changed."
  exit 1
fi

umask 077

# One-time random passphrase: only used to set the disk up, then wiped
# before this installer ends. It is never written to the installed disk.
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
disko-install --flake "$FLAKE#chlorum" --disk main "$disk" || rc=$?

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
    shred -u /tmp/chlorum-disk-password 2>/dev/null || true
    exit 1
  fi
fi
cleanup_target
udevadm settle

# Find the encrypted partition on the chosen disk itself, not by label:
# another drive may hold an older Chlorum with the same label.
PART=$(lsblk -lnpo NAME,PARTLABEL "$disk" | awk '$2 == "disk-main-system" { print $1 }')
if [ -z "$PART" ]; then
  echo "INSTALL FAILED: the encrypted partition was not found on $disk."
  shred -u /tmp/chlorum-disk-password 2>/dev/null || true
  exit 1
fi

# The PC's security chip opens the disk by itself, but only while the
# Secure Boot settings stay the same (PCR 7). If that fails, the disk asks
# for the recovery code instead.
tpm=no
if systemd-analyze has-tpm2 >/dev/null 2>&1; then
  echo
  echo "Linking the disk to this PC's security chip (TPM)."
  if systemd-cryptenroll "$PART" \
       --unlock-key-file=/tmp/chlorum-disk-password \
       --tpm2-device=auto \
       --tpm2-pcrs=7; then
    tpm=yes
  else
    echo "Could not use the security chip. The disk will ask for the recovery code."
  fi
else
  echo
  echo "This PC has no usable security chip (TPM)."
fi

# Always a recovery code, made and shown before the setup passphrase is
# wiped, so there is never a moment when the disk has only one way in.
echo
echo "RECOVERY CODE for this disk. Write it on paper now and keep it away from this PC."
echo "Never photograph it or send it to anyone."
if [ "$tpm" = yes ]; then
  echo "You need it only if the security chip ever refuses, for example after a firmware update."
else
  echo "The disk asks for it at every start until you turn the TPM on in"
  echo "firmware settings and run: chlorum-keys tpm"
fi
systemd-cryptenroll "$PART" \
  --unlock-key-file=/tmp/chlorum-disk-password \
  --recovery-key
echo
read -rp "Press Enter after you have written the code down. " _

echo
echo "Removing the one-time setup passphrase."
systemd-cryptenroll "$PART" \
  --unlock-key-file=/tmp/chlorum-disk-password \
  --wipe-slot=password
shred -u /tmp/chlorum-disk-password

echo
echo "Done. Remove the USB stick and reboot."
echo "On first start Chlorum sets up your profile."
