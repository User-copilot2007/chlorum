# Chlorum first-run setup. Runs once on tty1 before the login prompt.
# Asks only what it cannot decide itself: the keyboard layout, Wi-Fi when
# there is no cable, then one screen with a generated profile name and
# password. The disk's recovery code was already shown by the installer.

STATE=/var/lib/chlorum
Y=$'\033[1;33m'
N=$'\033[0m'

adjectives=(
  brave calm clever cosmic crisp curious daring eager fluffy frosty
  gentle giant golden happy hasty humble jolly keen lazy lively
  lucky mellow mighty misty nimble noble odd plucky polite proud
  quick quiet rapid rusty shiny silent sleepy sly snowy speedy
  spicy steady sunny swift tidy tiny vivid wild witty zesty
)
animals=(
  badger beaver bison camel cobra cougar coyote crane dingo falcon
  ferret gecko gopher heron hippo husky ibis iguana jackal koala
  lemur llama lynx magpie marmot mole moose newt ocelot otter
  owl panda parrot pelican puffin quokka rabbit raven salmon seal
  sloth tapir toucan turtle walrus weasel wombat yak zebra lobster
)

# One random word from the arguments
pick() {
  local r
  r=$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')
  shift $(( r % $# ))
  printf '%s' "$1"
}

new_name() {
  printf '%s-%s' "$(pick "${adjectives[@]}")" "$(pick "${animals[@]}")"
}

title() {
  printf '\033c'
  printf '\n  %sCHLORUM SETUP%s\n\n' "$Y" "$N"
}

# Profiles are created through the system message bus (D-Bus).
# Make sure it runs; start it again once if it failed during boot.
wait_for_bus() {
  local i
  for i in $(seq 1 30); do
    busctl --system status >/dev/null 2>&1 && return 0
    if [ "$i" -eq 5 ]; then
      systemctl reset-failed dbus.socket dbus.service >/dev/null 2>&1 || true
      systemctl start dbus.socket dbus.service >/dev/null 2>&1 || true
    fi
    sleep 1
  done
  echo "The system message bus (D-Bus) is not running."
  return 1
}

# Shown on every failure, so a photo of the screen is enough to find the cause.
diagnostics() {
  echo
  echo "Diagnostics:"
  systemctl --failed --no-pager --plain --no-legend 2>&1 | head -n 10 || true
  journalctl -b -p err --no-pager -o cat 2>&1 | tail -n 12 || true
}

setup() {
  mkdir -p "$STATE"

  # 0. Keyboard layout, so what is typed matches what is shown
  title
  echo "Keyboard layout. Press Enter for US English, or type a layout"
  echo "such as fr, de, uk, ua, es, it, pl, be, ch and press Enter."
  while true; do
    read -rp "Layout [us]: " km
    km=${km:-us}
    if [[ $km =~ ^[a-z0-9_-]{2,20}$ ]] && loadkeys -q "$km" 2>/dev/null; then
      printf '%s\n' "$km" > "$STATE/keymap"
      break
    fi
    echo "Unknown layout \"$km\". Try again, for example fr or de."
  done

  # 1. Internet: cable just works, Wi-Fi only when there is no cable
  title
  echo "Checking the internet connection..."
  if ! nm-online -q -t 20; then
    echo "No internet over cable. Choose a Wi-Fi network."
    sleep 2
    nmtui-connect || true
    title
    if ! nm-online -q -t 20; then
      echo "Still offline. Setup continues; connect later with: nmtui"
      sleep 3
    fi
  fi

  # 2. Computer name, generated and kept across restarts
  if [ ! -s "$STATE/hostname" ]; then
    new_name > "$STATE/hostname"
  fi
  cat "$STATE/hostname" > /proc/sys/kernel/hostname

  # 3. A profile left by an earlier run that never reached the last screen.
  #    Its password was never shown, and it holds nothing yet.
  while read -r old _; do
    [ -n "$old" ] && homectl remove "$old"
  done < <(homectl list --no-legend 2>/dev/null)

  # 4. The owner profile: its own encrypted volume, opened by its password
  user=$(new_name)
  pass=$(xkcdpass -w eff-long -n 5 -d -)
  title
  echo "Chlorum made a profile name and a password for you:"
  echo
  printf '  Profile name:  %s%s%s\n' "$Y" "$user" "$N"
  printf '  Password:      %s%s%s\n' "$Y" "$pass" "$N"
  echo
  echo "Press Enter to use them, or type c and Enter to choose your own."
  read -r choice
  tries=0
  while true; do
    if [ "$choice" = c ]; then
      while true; do
        read -rp "Profile name (lowercase letters, digits and -): " user
        [[ $user =~ ^[a-z][a-z0-9-]{1,30}$ ]] && break
        echo "Use 2 to 31 lowercase letters, digits or -, starting with a letter."
      done
      while true; do
        read -rsp "Password (at least 12 characters): " p1; echo
        read -rsp "Repeat: " p2; echo
        if [ "${#p1}" -ge 12 ] && [ "$p1" = "$p2" ]; then
          pass=$p1
          break
        fi
        echo "The passwords must match and be at least 12 characters."
      done
    fi
    echo
    echo "Creating your encrypted profile. This takes a minute."
    wait_for_bus
    if NEWPASSWORD="$pass" homectl create "$user" \
         --real-name="$user" \
         --member-of=chlorum-owner \
         --storage=luks \
         --fs-type=btrfs \
         --enforce-password-policy=no \
         --luks-discard=yes; then
      break
    fi
    tries=$((tries + 1))
    if [ "$tries" -ge 3 ]; then
      return 1
    fi
    echo
    echo "That did not work (see the message above)."
    echo "Press Enter to try again, or type c and Enter to choose your own."
    read -r choice
  done

  # 5. The only screen to remember
  title
  echo "WRITE THESE DOWN ON PAPER. They are shown only once."
  echo "Never photograph them or send them to anyone."
  echo
  printf '  Profile name:   %s%s%s\n' "$Y" "$user" "$N"
  printf '  Password:       %s%s%s\n' "$Y" "$pass" "$N"
  echo
  read -rp "Press Enter after you have written everything down. " _

  touch "$STATE/setup-done"
  title
  echo "Setup finished. Log in with your profile name and password."
  sleep 3
}

# Ctrl-C must not end setup and leave a login prompt with no profile.
trap '' INT QUIT

while true; do
  set +e
  (set -e; setup)
  rc=$?
  set -e
  [ "$rc" -eq 0 ] && exit 0

  diagnostics
  echo
  echo "Setup failed (error $rc)."
  echo "  r  try again"
  echo "  s  repair shell (only possible until setup has finished)"
  echo "  p  power off"
  read -rp "Choice: " c
  case "$c" in
    s) bash -l || true ;;
    p) systemctl poweroff ;;
    *) ;;
  esac
done
