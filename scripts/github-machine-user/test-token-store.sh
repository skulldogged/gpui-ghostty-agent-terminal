#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TOKEN_STORE="$SCRIPT_DIR/token-store"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/codex-token-store-test.XXXXXX")

cleanup() {
  case "$TEST_ROOT" in
    "${TMPDIR:-/tmp}"/codex-token-store-test.*)
      rm -rf -- "$TEST_ROOT"
      ;;
  esac
}
trap cleanup EXIT

export XDG_CONFIG_HOME="$TEST_ROOT/config"
export XDG_DATA_HOME="$TEST_ROOT/data"
export CODEX_GITHUB_DISABLE_SECRET_SERVICE=1

mkdir -p "$XDG_CONFIG_HOME/codex-github-machine"
cat > "$XDG_CONFIG_HOME/codex-github-machine/setup.env" <<'CONFIG'
TARGET_REPO=skulldogged/gpui-ghostty-agent-terminal
AGENT_GITHUB_USER=fixture-agent
AGENT_GITHUB_TOKEN_FILE=TOKEN_FILE_PLACEHOLDER
CONFIG
sed -i "s|TOKEN_FILE_PLACEHOLDER|$XDG_DATA_HOME/codex-github-machine/fixture-agent/github-token|" \
  "$XDG_CONFIG_HOME/codex-github-machine/setup.env"

FIXTURE_TOKEN="fixture-token-do-not-use"

if printf '%s' "$FIXTURE_TOKEN" | "$TOKEN_STORE" store-secret-service >/dev/null 2>&1; then
  printf 'expected disabled Secret Service storage to fail\n' >&2
  exit 1
fi

printf '%s' "$FIXTURE_TOKEN" | "$TOKEN_STORE" store-file
LOOKED_UP_TOKEN=$("$TOKEN_STORE" lookup)

if [[ "$LOOKED_UP_TOKEN" != "$FIXTURE_TOKEN" ]]; then
  printf 'file-backed token did not round-trip\n' >&2
  exit 1
fi

TOKEN_FILE="$XDG_DATA_HOME/codex-github-machine/fixture-agent/github-token"
if [[ ! -O "$TOKEN_FILE" ]]; then
  printf 'token file is not owned by the current user\n' >&2
  exit 1
fi

if [[ "$(stat -c '%a' "$TOKEN_FILE")" != "600" ]]; then
  printf 'token file mode is not 600\n' >&2
  exit 1
fi

printf 'token-store fallback test passed\n'
