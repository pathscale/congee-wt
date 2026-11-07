#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

todo='Publishing is currently disabled.'

case "${1:-}" in
  preflight)
    # Deliberately fail before reading metadata or making an HTTP request.
    fail "$todo"
    ;;
  registry-status)
    [ "$#" -eq 2 ] || fail 'usage: publish-crate.sh registry-status VERSION'
    version=$2
    case "$version" in
      ''|*[!0-9A-Za-z.+-]*) fail 'VERSION contains unsupported characters' ;;
    esac
    # Cargo versions may contain '+', which must be escaped in this path segment.
    encoded_version=${version//+/%2B}
    status=$(curl --disable --silent --show-error \
      --user-agent 'manifest-cargo-publisher (https://github.com/pathscale/congee-wt)' \
      --connect-timeout 10 --max-time 30 --retry 0 --proto '=https' \
      --output /dev/null --write-out '%{http_code}' \
      "https://crates.io/api/v1/crates/congee-wt/$encoded_version") \
      || fail 'crates.io registry request failed'
    case "$status" in
      200) printf '%s\n' 'already-published' ;;
      404) printf '%s\n' 'absent' ;;
      *) fail "crates.io returned HTTP $status; refusing to treat it as an absent version" ;;
    esac
    ;;
  *)
    fail 'usage: publish-crate.sh {preflight|registry-status VERSION}'
    ;;
esac
