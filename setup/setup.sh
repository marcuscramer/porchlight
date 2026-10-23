#!/bin/bash
# Sets up a Meta Portal for Porchlight over USB: installs the latest release and runs every one-time adb step
# from the README's "Convenience" section. Double-click Setup-Portal.command (macOS) instead of running this by
# hand; Restore-Portal.command runs it with --restore, which undoes the system changes (not the app itself).
#
# Safe to run again (after a factory reset, or to update). Every change to a setting someone else may also own
# (Meta's install verifier, Meta's system updates, the dialog overlays) records the value it found on the Portal
# itself, in /data/local/tmp/porchlight-setup.state, so --restore puts back exactly that and nothing more — a Portal
# that also runs Immortal keeps Immortal's settings.
#
# Options: --restore   undo the system changes
#          --yes       answer every question with its default (no prompts)
# Environment: ANDROID_SERIAL picks the device when more than one is connected (standard adb behaviour);
#              PORCHLIGHT_ANY_DEVICE=1 skips the "is this a Portal?" question (for testing on an emulator).

set -u
cd "$(dirname "$0")" || exit 1
SCRIPT_DIR="$(pwd)"

PKG=dev.porchlight.app
ACTIVITY="$PKG/.MainActivity"
A11Y_SERVICE="$PKG/$PKG.CallWakeUpAccessibilityService"
APK_URL="https://github.com/marcuscramer/porchlight/releases/latest/download/app-release.apk"
STATE=/data/local/tmp/porchlight-setup.state
VERIFIER_PKG=com.facebook.appverifier
OTA_PKGS="com.facebook.aloha.otaui com.facebook.aloha.alohaotasetup"
OVERLAY_PKGS="com.facebook.aloha.rro.niu.android com.facebook.aloha.rro.niu.settings"

MODE=setup
ASSUME_YES=false
for arg in "$@"; do
  case "$arg" in
    --restore) MODE=restore ;;
    --yes) ASSUME_YES=true ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

# ----- output -----------------------------------------------------------------
if [ -t 1 ]; then B=$'\033[1m'; G=$'\033[32m'; Y=$'\033[33m'; R=$'\033[31m'; N=$'\033[0m'; else B= G= Y= R= N=; fi
step() { printf "\n%s==>%s %s\n" "$B" "$N" "$1"; }
ok()   { printf "  %s✓%s %s\n" "$G" "$N" "$1"; }
skip() { printf "  %s–%s %s\n" "$Y" "$N" "$1"; }
warn() { printf "  %s!%s %s\n" "$Y" "$N" "$1"; }
die()  { printf "\n%sStopped:%s %s\n" "$R" "$N" "$1" >&2; exit 1; }
# ask "question" default(y|n) -> returns 0 for yes
ask() {
  local def="$2" hint answer
  [ "$def" = y ] && hint="Y/n" || hint="y/N"
  if $ASSUME_YES || [ ! -t 0 ]; then [ "$def" = y ]; return; fi
  printf "  %s [%s] " "$1" "$hint"
  read -r answer
  answer="$(printf %s "$answer" | tr '[:upper:]' '[:lower:]')"
  [ -z "$answer" ] && answer="$def"
  [ "$answer" = y ] || [ "$answer" = yes ] || [ "$answer" = j ] || [ "$answer" = ja ]
}

# ----- adb: already here, on PATH, or downloaded from Google -----------------
resolve_adb() {
  if [ -x "$SCRIPT_DIR/platform-tools/adb" ]; then ADB="$SCRIPT_DIR/platform-tools/adb"; return; fi
  if command -v adb >/dev/null 2>&1; then ADB="$(command -v adb)"; return; fi
  step "Getting adb (Google's Android platform-tools) — into this folder only, nothing is installed system-wide"
  local os
  case "$(uname -s)" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) die "This computer isn't supported for the automatic download. Install Google's platform-tools and run again." ;;
  esac
  curl -fL --progress-bar -o "$SCRIPT_DIR/platform-tools.zip" \
    "https://dl.google.com/android/repository/platform-tools-latest-$os.zip" \
    || die "Couldn't download platform-tools. Check the internet connection and run again."
  unzip -oq "$SCRIPT_DIR/platform-tools.zip" -d "$SCRIPT_DIR" || die "Couldn't unpack platform-tools."
  rm -f "$SCRIPT_DIR/platform-tools.zip"
  [ -x "$SCRIPT_DIR/platform-tools/adb" ] || die "adb is missing after the download."
  ADB="$SCRIPT_DIR/platform-tools/adb"
  ok "adb ready"
}

a()  { "$ADB" "$@"; }
sh_() { "$ADB" shell "$@" 2>/dev/null | tr -d '\r'; }   # run on the Portal, output without CRs

# ----- wait for exactly one authorised device ---------------------------------
wait_for_device() {
  step "Looking for the Portal"
  a start-server >/dev/null 2>&1
  local said_none=false said_unauth=false lines count
  while true; do
    lines="$(a devices | tr -d '\r' | sed '1d' | grep -v '^$' || true)"
    if [ -n "${ANDROID_SERIAL:-}" ]; then lines="$(printf '%s\n' "$lines" | grep "^$ANDROID_SERIAL[[:space:]]" || true)"; fi
    count="$(printf '%s\n' "$lines" | grep -c . || true)"
    if [ "$count" -gt 1 ]; then
      printf '%s\n' "$lines"
      die "More than one device is connected. Unplug the others, or run again with ANDROID_SERIAL=<serial from the list above>."
    fi
    case "$lines" in
      *$'\t'device)
        ok "Connected: $(printf %s "$lines" | cut -f1)"; return ;;
      *$'\t'unauthorized)
        $said_unauth || { warn "On the Portal's screen: allow USB debugging (tick \"Always allow\", then Allow)."; said_unauth=true; } ;;
      *)
        $said_none || {
          warn "No Portal found yet. Check that:"
          echo "      - it's connected to this computer with a USB-C cable (one that carries data, not only power)"
          echo "      - Settings > Debug > ADB Enabled is switched on on the Portal"
          said_none=true
        } ;;
    esac
    sleep 2
  done
}

describe_device() {
  SDK="$(sh_ getprop ro.build.version.sdk)"
  DEVICE="$(sh_ getprop ro.product.device)"
  case "$DEVICE" in
    ripley) MODEL="Portal TV" ;;
    aloha) MODEL="Portal+ (2018)" ;;
    ohana) MODEL="Portal (2018)" ;;
    omni) MODEL="Portal Mini / Portal (2019)" ;;
    atlas) MODEL="Portal (2019)" ;;
    cipher) MODEL="Portal+ (2021)" ;;
    terry) MODEL="Portal Go" ;;
    *) MODEL="" ;;
  esac
  if [ -n "$MODEL" ]; then
    ok "$MODEL (Android API $SDK)"
  else
    warn "This doesn't look like a Meta Portal (device \"$DEVICE\", Android API $SDK)."
    [ "${PORCHLIGHT_ANY_DEVICE:-}" = 1 ] || ask "Continue anyway?" n || die "Nothing was changed."
  fi
}

# `settings get` prints "null" for a setting that doesn't exist, so an empty answer is a failed read (seen once on
# an emulator). Retry, and stop rather than act on it: writing back a list read as empty would drop everyone
# else's entries (e.g. Immortal's accessibility service).
setting_get() {
  local v i
  for i in 1 2 3 4 5; do
    v="$(sh_ settings get "$1" "$2")"
    [ -n "$v" ] && { printf %s "$v"; return; }
    sleep 1
  done
  die "Couldn't read the Portal's setting $1 $2. Nothing more was changed; run the setup again."
}
has_pkg() { sh_ pm list packages "$1" | grep -qx "package:$1"; }
pkg_enabled() { ! sh_ pm list packages -d "$1" | grep -qx "package:$1"; }
# The value a setting had before this script first changed it; recorded once, so running setup again never
# overwrites the original with this script's own value.
record_once() { sh_ "grep -q '^$1=' $STATE 2>/dev/null || echo '$1=$2' >> $STATE" >/dev/null; }
recorded() { sh_ "grep '^$1=' $STATE 2>/dev/null | head -1 | cut -d= -f2-"; }
forget() { sh_ "[ -f $STATE ] && grep -v '^$1=' $STATE > $STATE.tmp; mv $STATE.tmp $STATE 2>/dev/null" >/dev/null; }

# ----- setup steps -------------------------------------------------------------
turn_off_install_verifier() {
  step "Allowing Porchlight to update itself"
  # Meta's verifier silently rejects every sideloaded install, Porchlight's own updates included.
  if has_pkg "$VERIFIER_PKG"; then
    if pkg_enabled "$VERIFIER_PKG"; then
      record_once verifier_app enabled
      sh_ pm disable-user --user 0 "$VERIFIER_PKG" >/dev/null && ok "Meta's install verifier switched off"
    else
      ok "Meta's install verifier is already off"
    fi
  else
    skip "No Meta install verifier on this device"
  fi
  local current
  current="$(setting_get global package_verifier_enable)"
  if [ "$current" != 0 ]; then
    record_once package_verifier_enable "$current"
    sh_ settings put global package_verifier_enable 0 >/dev/null && ok "Android's install verification switched off"
  else
    ok "Android's install verification is already off"
  fi
}

install_app() {
  step "Downloading the latest Porchlight"
  local apk="$SCRIPT_DIR/porchlight-latest.apk"
  curl -fL --progress-bar -o "$apk" "$APK_URL" || die "Couldn't download Porchlight. Check the internet connection and run again."
  [ "$(head -c 2 "$apk")" = PK ] || { rm -f "$apk"; die "The download isn't an app package. Try again later."; }
  ok "Downloaded"
  step "Installing Porchlight on the Portal"
  local out
  out="$(a install -r -g --no-streaming "$apk" 2>&1 | tr -d '\r')"
  rm -f "$apk"
  case "$out" in
    *Success*) ok "Installed (contacts and settings of an earlier install are kept)" ;;
    *INSTALL_FAILED_VERSION_DOWNGRADE*) ok "A newer Porchlight is already installed — keeping it" ;;
    *INSTALL_FAILED_UPDATE_INCOMPATIBLE*)
      die "The Porchlight on this Portal was signed differently (a test build?). Uninstalling it deletes its
contacts; if that's fine, run:  adb uninstall $PKG  and then this setup again." ;;
    *) printf '%s\n' "$out" | tail -3; die "The install didn't succeed (see above)." ;;
  esac
}

grant_permissions() {
  step "Camera and microphone"
  sh_ pm grant "$PKG" android.permission.CAMERA >/dev/null
  sh_ pm grant "$PKG" android.permission.RECORD_AUDIO >/dev/null
  if [ "${SDK:-0}" -ge 33 ]; then sh_ pm grant "$PKG" android.permission.POST_NOTIFICATIONS >/dev/null; fi
  ok "Allowed, so the first call doesn't stop at a permission question"
  sh_ appops set "$PKG" REQUEST_INSTALL_PACKAGES allow >/dev/null && ok "Allowed to install its own updates"
}

enable_call_wake_up() {
  step "Call wake-up (ends the screensaver and turns the TV on for a call)"
  local list
  list="$(setting_get secure enabled_accessibility_services)"
  case ":$list:" in
    *":$A11Y_SERVICE:"*) ok "Already on" ;;
    *)
      if [ -z "$list" ] || [ "$list" = null ]; then list="$A11Y_SERVICE"; else list="$list:$A11Y_SERVICE"; fi
      sh_ settings put secure enabled_accessibility_services "$list" >/dev/null
      ok "Switched on (it can be turned off any time in Porchlight's Settings)"
      ;;
  esac
  sh_ settings put secure accessibility_enabled 1 >/dev/null
  sh_ appops set "$PKG" SYSTEM_ALERT_WINDOW allow >/dev/null && ok "Allowed to cover the screen briefly while it does that"
}

fix_dialog_overlays() {
  # Android 9 Portals re-theme system dialogs (e.g. the update confirmation) white-on-white. Newer ones don't.
  [ "${SDK:-0}" -lt 29 ] || return 0
  step "Making system dialogs readable"
  local overlays o
  overlays="$(sh_ cmd overlay list)"
  for o in $OVERLAY_PKGS; do
    if printf '%s\n' "$overlays" | grep -q "\[x\] $o"; then
      record_once "overlay_$o" enabled
      sh_ cmd overlay disable --user 0 "$o" >/dev/null && ok "$o switched off"
    elif printf '%s\n' "$overlays" | grep -q " $o"; then
      ok "$o is already off"
    else
      skip "$o isn't on this device"
    fi
  done
}

maybe_stop_meta_updates() {
  local present="" p
  for p in $OTA_PKGS; do has_pkg "$p" && present="$present $p"; done
  [ -n "$present" ] || return 0
  step "Meta's system updates"
  echo "    Meta no longer develops the Portal; an update from Meta could still change how the Portal behaves."
  echo "    Switching its updater off keeps the Portal as it is now. Undo with Restore-Portal."
  if ask "Switch Meta's system updates off?" n; then
    for p in $present; do
      if pkg_enabled "$p"; then
        record_once "ota_$p" enabled
        sh_ pm disable-user --user 0 "$p" >/dev/null
      fi
    done
    ok "Meta's system updates switched off"
  else
    skip "Left as they are"
  fi
}

launch() {
  step "Starting Porchlight"
  sh_ am start -n "$ACTIVITY" >/dev/null && ok "Porchlight is open on the Portal"
}

# ----- restore -----------------------------------------------------------------
restore() {
  step "Undoing Porchlight's system changes (the app itself stays installed)"
  local list new v o p
  list="$(setting_get secure enabled_accessibility_services)"
  new="$(printf %s "$list" | tr ':' '\n' | grep -vx "$A11Y_SERVICE" | grep -v '^$' | paste -sd: -)"
  if [ "$new" != "$list" ]; then
    if [ -n "$new" ]; then sh_ settings put secure enabled_accessibility_services "$new" >/dev/null
    else sh_ settings delete secure enabled_accessibility_services >/dev/null; fi
    ok "Call wake-up switched off"
  fi
  if has_pkg "$PKG"; then
    sh_ appops set "$PKG" SYSTEM_ALERT_WINDOW default >/dev/null
    sh_ appops set "$PKG" REQUEST_INSTALL_PACKAGES default >/dev/null
    ok "Screen cover and self-update permissions taken back"
  fi
  if [ "$(recorded verifier_app)" = enabled ]; then
    sh_ pm enable "$VERIFIER_PKG" >/dev/null && ok "Meta's install verifier switched back on"; forget verifier_app
  fi
  v="$(recorded package_verifier_enable)"
  if [ -n "$v" ]; then
    if [ "$v" = null ]; then sh_ settings delete global package_verifier_enable >/dev/null
    else sh_ settings put global package_verifier_enable "$v" >/dev/null; fi
    ok "Android's install verification put back"; forget package_verifier_enable
  fi
  for o in $OVERLAY_PKGS; do
    if [ "$(recorded "overlay_$o")" = enabled ]; then
      sh_ cmd overlay enable --user 0 "$o" >/dev/null && ok "$o switched back on"; forget "overlay_$o"
    fi
  done
  for p in $OTA_PKGS; do
    if [ "$(recorded "ota_$p")" = enabled ]; then
      sh_ pm enable "$p" >/dev/null && ok "$p switched back on"; forget "ota_$p"
    fi
  done
  if has_pkg "$PKG" && ask "Also uninstall Porchlight? This deletes its contacts." n; then
    a uninstall "$PKG" >/dev/null 2>&1 && ok "Porchlight uninstalled"
  fi
}

# ----- main --------------------------------------------------------------------
echo "${B}Porchlight setup for Meta Portal${N}"
resolve_adb
wait_for_device
describe_device
if [ "$MODE" = restore ]; then
  restore
  step "Done."
  exit 0
fi
turn_off_install_verifier
install_app
grant_permissions
enable_call_wake_up
fix_dialog_overlays
maybe_stop_meta_updates
launch
step "Done."
echo "    Next, on the Portal: pick a name for it, then pair with someone (Add contact)."
echo "    If this Portal is only for calls (e.g. for a relative), switch on Kiosk mode in Porchlight's Settings:"
echo "    Porchlight then comes back by itself after a restart and after the screensaver."
