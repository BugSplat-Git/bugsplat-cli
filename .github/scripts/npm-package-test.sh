#!/usr/bin/env bash
# Installs the npm package dist generated into a throwaway project and runs `bugsplat` through
# it. The package downloads its binary from a GitHub Release, which doesn't exist before the
# tag, so the download URL is pointed at the archives in the same folder.
# Usage: npm-package-test.sh <folder holding dist's artifacts>
set -euo pipefail

distrib=$(cd "$1" && pwd)
port=${PORT:-8765}

work=$(mktemp -d)
server=
cleanup() {
  if [[ -n $server ]]; then
    kill "$server" 2> /dev/null || true
    wait "$server" 2> /dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$distrib" > /dev/null 2>&1 &
server=$!
for _ in $(seq 1 50); do
  curl -fs -o /dev/null "http://127.0.0.1:$port/" && break
  sleep 0.1
done

tar -xzf "$distrib/bugsplat-cli-npm-package.tar.gz" -C "$work"
node -e '
  const fs = require("fs");
  const [file, url] = process.argv.slice(1);
  const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
  manifest.artifactDownloadUrls = [url];
  fs.writeFileSync(file, JSON.stringify(manifest, null, 2));
' "$work/package/package.json" "http://127.0.0.1:$port"
tar -czf "$work/package.tgz" -C "$work" package

mkdir "$work/project"
cd "$work/project"
npm init -y > /dev/null
npm install --no-audit --no-fund "$work/package.tgz"

# Credentials in the caller's environment would outrank the empty session store and could
# send `whoami` to the real API.
unset BUGSPLAT_ACCESS_TOKEN BUGSPLAT_CLIENT_ID BUGSPLAT_CLIENT_SECRET BUGSPLAT_DATABASE \
  BUGSPLAT_API_URL BUGSPLAT_HOST SYMBOL_UPLOAD_CLIENT_ID SYMBOL_UPLOAD_CLIENT_SECRET
export BUGSPLAT_TOKEN_STORE=file
export BUGSPLAT_CONFIG_DIR="$work/config"
version=$(npx --no-install bugsplat --version)
echo "$version"
[[ $version == "bugsplat "* ]] || { echo "FAIL: unexpected --version output" >&2; exit 1; }

set +e
npx --no-install bugsplat --api-url http://127.0.0.1:9 whoami
code=$?
set -e
[[ $code -eq 3 ]] || { echo "FAIL: exit code $code did not pass through npm's wrapper, expected 3" >&2; exit 1; }

echo "npm package test passed"
