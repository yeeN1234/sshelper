#!/bin/sh
# install-key.sh - executed on a Linux/Unix target by sshelper, then deletes itself.
#
#   sh install-key.sh <uploaded .pub file>
#
# * ~/.ssh -> 700, ~/.ssh/authorized_keys -> 600
# * CR characters (CRLF from Windows) are stripped with tr -d '\r', a UTF-8 BOM
#   is removed, and the key is appended only if it is not already present.
# * The uploaded temp files (this script and the .pub) are always removed.
#
# POSIX sh only (dash / busybox / bash). Output is kept ASCII because it is
# printed by the caller's console, whatever its code page.

PUB=${1:-}
SELF=$0

cleanup() { rm -f -- "$PUB" "$SELF" 2>/dev/null; }
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

say()  { printf '[remote] %s\n' "$*"; }
fail() { printf '[remote] ERROR: %s\n' "$*" >&2; exit 1; }

[ -n "$PUB" ] && [ -f "$PUB" ] || fail "uploaded public key not found: $PUB"

umask 077
SSH_DIR=$HOME/.ssh
AUTH=$SSH_DIR/authorized_keys

mkdir -p "$SSH_DIR"            || fail "cannot create $SSH_DIR"
chmod 700 "$SSH_DIR"           || fail "cannot chmod 700 $SSH_DIR"
[ -e "$AUTH" ] || : > "$AUTH"  || fail "cannot create $AUTH"
chmod 600 "$AUTH"              || fail "cannot chmod 600 $AUTH"

# Remove CR (CRLF / stray \r from Windows), drop a UTF-8 BOM, keep the first
# non-empty line.
BOM=$(printf '\357\273\277')
KEY=$(tr -d '\r' < "$PUB" | awk 'NF { print; exit }')
KEY=${KEY#"$BOM"}

case $KEY in
  ssh-*|ecdsa-*|sk-*) ;;
  *) fail "uploaded file does not look like an OpenSSH public key" ;;
esac

BLOB=$(printf '%s\n' "$KEY" | awk '{ print $2 }')
[ -n "$BLOB" ] || fail "public key has no key data"

if grep -qF -- "$BLOB" "$AUTH"; then
  say "key already present in ~/.ssh/authorized_keys (nothing appended)"
else
  # Without a trailing newline the appended key would be glued to the last one.
  if [ -s "$AUTH" ] && [ -n "$(tail -c 1 "$AUTH")" ]; then
    printf '\n' >> "$AUTH" || fail "cannot write $AUTH"
  fi
  printf '%s\n' "$KEY" >> "$AUTH" || fail "cannot write $AUTH"
  say "key appended to ~/.ssh/authorized_keys"
fi

# SELinux (RHEL / Fedora): a freshly created ~/.ssh needs the right context.
if command -v restorecon >/dev/null 2>&1; then
  restorecon -R "$SSH_DIR" >/dev/null 2>&1 || :
fi

# sshd StrictModes ignores the key when the home directory is group/world writable.
if [ -n "$(find "$HOME" -maxdepth 0 \( -perm -0020 -o -perm -0002 \) 2>/dev/null)" ]; then
  say "WARNING: $HOME is group/world writable, sshd may ignore the key. Fix: chmod go-w \"$HOME\""
fi

say "permissions: ~/.ssh 700, ~/.ssh/authorized_keys 600"
say "done"
