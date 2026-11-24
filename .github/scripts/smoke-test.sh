#!/usr/bin/env bash
# Runs a release archive's `bugsplat` on the machine it was built for.
# Usage: smoke-test.sh <archive (.tar.gz, .tar.xz or .zip)> <target triple>
set -euo pipefail

archive=$1
target=$2
fixtures=$(cd "$(dirname "$0")/../../crates/bugsplat-debugid/tests/fixtures" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# A Windows program can't open the /tmp path Git Bash reports.
if command -v cygpath > /dev/null; then
  work=$(cygpath -m "$work")
  fixtures=$(cygpath -m "$fixtures")
fi

case $archive in
  *.zip) unzip -q "$archive" -d "$work/archive" ;;
  *) mkdir "$work/archive" && tar -xf "$archive" -C "$work/archive" ;;
esac
bin=$(find "$work/archive" -type f \( -name bugsplat -o -name bugsplat.exe \) | head -n 1)
[[ -n $bin ]] || { echo "no bugsplat binary in $archive" >&2; exit 1; }

unset BUGSPLAT_ACCESS_TOKEN BUGSPLAT_CLIENT_ID BUGSPLAT_CLIENT_SECRET BUGSPLAT_DATABASE \
  BUGSPLAT_API_URL BUGSPLAT_HOST SYMBOL_UPLOAD_CLIENT_ID SYMBOL_UPLOAD_CLIENT_SECRET
# Never the keychain of whoever runs this.
export BUGSPLAT_TOKEN_STORE=file
export BUGSPLAT_CONFIG_DIR="$work/config"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Runs bugsplat with the given arguments; leaves its output in $out and exit code in $code.
run() {
  set +e
  out=$("$bin" "$@" 2>&1)
  code=$?
  set -e
  echo "\$ bugsplat $* (exit $code)"
  echo "$out"
}

expect() {
  local want_code=$1 want_text=$2
  shift 2
  run "$@"
  [[ $code -eq $want_code ]] || fail "bugsplat $*: exit $code, expected $want_code"
  [[ $out == *"$want_text"* ]] || fail "bugsplat $*: output lacks \"$want_text\""
}

expect 0 "($target)" --version

expect 0 E546B55B6D214E86871B40AC35CD0D461 symbols info "$fixtures/pdb-guid/bugsplat.pdb"
expect 0 64FB82ED7A000 symbols info "$fixtures/pdb-guid/bugsplat.dll"
expect 0 85fe216fc7dd441f04c237310a56081fbf23c082 symbols info "$fixtures/elfy/hello.elf"
expect 0 ac15902fba2e31c18b29da51e889b321 symbols info "$fixtures/macho-uuid/bugsplat.app.dSYM"

# Nothing listens on port 9, and a stored session that hasn't expired needs no request.
api=http://127.0.0.1:9
session="$BUGSPLAT_CONFIG_DIR/sessions/http___127.0.0.1_9.json"
seed_session() {
  mkdir -p "$(dirname "$session")"
  cat > "$session" << JSON
{"access_token":"smoke-test","refresh_token":null,"expires_at":4000000000,"client_id":"smoke-test","token_endpoint":"$api/oauth/token","email":"smoke@example.com"}
JSON
}

expect 3 "Not signed in" --api-url "$api" whoami
seed_session
expect 0 smoke@example.com --api-url "$api" whoami
expect 0 "Signed out" --api-url "$api" logout
[[ ! -e $session ]] || fail "logout left $session behind"
expect 3 "Not signed in" --api-url "$api" whoami

# A CI runner has no Secret Service, which is what a container or a server looks like. Only
# there: on a desktop this would reach the real keyring.
if [[ ${GITHUB_ACTIONS:-} == true && $(uname -s) == Linux ]]; then
  export BUGSPLAT_TOKEN_STORE=auto
  expect 3 "Not signed in" --api-url "$api" whoami
  seed_session
  expect 0 smoke@example.com --api-url "$api" whoami
  expect 0 "Signed out" --api-url "$api" logout
  [[ ! -e $session ]] || fail "logout left $session behind"

  export BUGSPLAT_TOKEN_STORE=keychain
  expect 1 Keychain --api-url "$api" whoami
fi

echo "smoke test passed: $target"
