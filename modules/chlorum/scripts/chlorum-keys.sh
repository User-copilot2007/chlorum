# chlorum-keys: optional extra protection. Stand-in for the
# "Keys & security" page of the System Center until it exists.

PART=/dev/disk/by-partlabel/disk-main-system

usage() {
  cat <<'EOF'
chlorum-keys list          show what opens the disk
chlorum-keys disk-key      add a security key (YubiKey or any FIDO2 key) to the disk
chlorum-keys profile-key   add a security key as another way into your profile
chlorum-keys tpm           link the disk to this PC's security chip again
                           (after a firmware update or turning the TPM on)
EOF
}

as_root() {
  if [ "$(id -u)" -ne 0 ]; then
    exec run0 "$0" "$@"
  fi
}

# Read the whole dump first: grep -q in a pipe can cut it off early.
disk_has() {
  local dump
  dump=$(cryptsetup luksDump "$PART" 2>/dev/null) || return 1
  grep -q -- "$1" <<<"$dump"
}

# How to prove we may change the disk: the security chip when it is
# linked, otherwise systemd-cryptenroll asks for the recovery code.
unlock_args=()
set_unlock() {
  if disk_has systemd-tpm2 && systemd-analyze has-tpm2 >/dev/null 2>&1; then
    unlock_args=(--unlock-tpm2-device=auto)
  fi
}

case "${1:-}" in
  list)
    as_root "$@"
    systemd-cryptenroll "$PART"
    ;;
  disk-key)
    as_root "$@"
    set_unlock
    echo "Insert the key. Then enter its PIN and touch it."
    systemd-cryptenroll "$PART" "${unlock_args[@]}" \
      --fido2-device=auto --fido2-with-client-pin=yes
    echo "Done. The disk now also opens with this key."
    ;;
  profile-key)
    if [ "$(id -u)" -eq 0 ]; then
      echo "Run this as yourself, not as root."
      exit 1
    fi
    echo "Insert the key. You will be asked for your profile password,"
    echo "then the key's PIN and a touch."
    homectl update "$(id -un)" --fido2-device=auto --fido2-with-client-pin=yes
    echo "Done. Your profile now also opens with this key."
    ;;
  tpm)
    as_root "$@"
    if ! systemd-analyze has-tpm2 >/dev/null 2>&1; then
      echo "No usable security chip (TPM). Turn it on in firmware settings first."
      exit 1
    fi
    echo "Enter your recovery code when asked."
    systemd-cryptenroll "$PART" --wipe-slot=tpm2 \
      --tpm2-device=auto --tpm2-pcrs=7
    echo "Done. The disk opens by itself again on this PC."
    ;;
  *)
    usage
    ;;
esac
