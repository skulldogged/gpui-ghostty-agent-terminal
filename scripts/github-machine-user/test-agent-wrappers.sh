#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
AGENT_GH="$SCRIPT_DIR/agent-gh"
AGENT_GIT="$SCRIPT_DIR/agent-git"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/codex-agent-wrapper-test.XXXXXX")

cleanup() {
  case "$TEST_ROOT" in
    "${TMPDIR:-/tmp}"/codex-agent-wrapper-test.*)
      rm -rf -- "$TEST_ROOT"
      ;;
  esac
}
trap cleanup EXIT

export XDG_CONFIG_HOME="$TEST_ROOT/config"
export XDG_DATA_HOME="$TEST_ROOT/data"
export CODEX_GITHUB_DISABLE_SECRET_SERVICE=1
export PATH="$TEST_ROOT/bin:$PATH"

CONFIG_ROOT="$XDG_CONFIG_HOME/codex-github-machine"
TOKEN_FILE="$XDG_DATA_HOME/codex-github-machine/fixture-agent/github-token"
AUTH_KEY="$XDG_DATA_HOME/codex-github-machine/fixture-agent/auth-key"
SIGNING_KEY="$XDG_DATA_HOME/codex-github-machine/fixture-agent/signing-key"
mkdir -p "$CONFIG_ROOT" "$(dirname "$TOKEN_FILE")" "$TEST_ROOT/bin" "$TEST_ROOT/repository"
chmod 700 "$CONFIG_ROOT" "$(dirname "$TOKEN_FILE")"
touch "$AUTH_KEY" "$SIGNING_KEY"
chmod 600 "$AUTH_KEY" "$SIGNING_KEY"

cat > "$CONFIG_ROOT/setup.env" <<CONFIG
AGENT_GITHUB_USER=fixture-agent
AGENT_GITHUB_EMAIL=fixture-agent@users.noreply.github.com
AGENT_GITHUB_AUTH_KEY=$AUTH_KEY
AGENT_GITHUB_SIGNING_KEY=$SIGNING_KEY
AGENT_GITHUB_TOKEN_FILE=$TOKEN_FILE
CONFIG
chmod 600 "$CONFIG_ROOT/setup.env"
printf '%s' 'fixture-token' > "$TOKEN_FILE"
chmod 600 "$TOKEN_FILE"
printf '%s\n' 'example/allowed' > "$CONFIG_ROOT/repositories"
chmod 600 "$CONFIG_ROOT/repositories"

cat > "$TEST_ROOT/bin/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${GH_TOKEN:-}" != "fixture-token" ]]; then
  printf 'fake gh: expected isolated machine token\n' >&2
  exit 1
fi

if [[ "${1:-}" == "api" && "${2:-}" == "user" ]]; then
  printf '%s\n' 'fixture-agent'
  exit 0
fi

if [[ "${1:-}" == "api" && "${2:-}" == repos/* ]]; then
  case "${2#repos/}" in
    example/allowed|example/second)
      printf '%s\n' 'true'
      exit 0
      ;;
    *)
      printf '%s\n' 'false'
      exit 0
      ;;
  esac
fi

printf 'fake gh invoked: %s\n' "$*"
FAKE_GH
chmod 700 "$TEST_ROOT/bin/gh"

git -C "$TEST_ROOT/repository" init --quiet
git -C "$TEST_ROOT/repository" remote add origin https://github.com/example/allowed.git

assert_contains() {
  local output=$1 expected=$2
  if [[ "$output" != *"$expected"* ]]; then
    printf 'expected output to contain %q, got:\n%s\n' "$expected" "$output" >&2
    exit 1
  fi
}

GH_IDENTITY=$(cd "$TEST_ROOT/repository" && "$AGENT_GH" identity)
assert_contains "$GH_IDENTITY" 'fixture-agent acting on example/allowed'

GIT_IDENTITY=$(cd "$TEST_ROOT/repository" && "$AGENT_GIT" identity)
assert_contains "$GIT_IDENTITY" 'fixture-agent <fixture-agent@users.noreply.github.com> acting on example/allowed'

git -C "$TEST_ROOT/repository" remote set-url origin https://github.com/example/unregistered.git
if UNREGISTERED_OUTPUT=$(cd "$TEST_ROOT/repository" && "$AGENT_GH" identity 2>&1); then
  printf 'expected unregistered repository identity to fail\n' >&2
  exit 1
fi
assert_contains "$UNREGISTERED_OUTPUT" 'grant @fixture-agent write access'
assert_contains "$UNREGISTERED_OUTPUT" 'agent-gh register example/unregistered'

if DENIED_OUTPUT=$("$AGENT_GH" register example/denied 2>&1); then
  printf 'expected registration without write access to fail\n' >&2
  exit 1
fi
assert_contains "$DENIED_OUTPUT" 'does not have write access'

REGISTER_OUTPUT=$("$AGENT_GH" register example/second)
assert_contains "$REGISTER_OUTPUT" 'Registered example/second for @fixture-agent'
grep -Fqx 'example/allowed' "$CONFIG_ROOT/repositories"
grep -Fqx 'example/second' "$CONFIG_ROOT/repositories"

git -C "$TEST_ROOT/repository" remote set-url origin git@github.com:example/second.git
SECOND_IDENTITY=$(cd "$TEST_ROOT/repository" && "$AGENT_GH" identity)
assert_contains "$SECOND_IDENTITY" 'fixture-agent acting on example/second'

if OVERRIDE_OUTPUT=$(cd "$TEST_ROOT/repository" && "$AGENT_GH" pr list -R example/allowed 2>&1); then
  printf 'expected cross-repository override to fail\n' >&2
  exit 1
fi
assert_contains "$OVERRIDE_OUTPUT" 'refusing repository override: example/allowed'

if API_OUTPUT=$(cd "$TEST_ROOT/repository" && \
  "$AGENT_GH" api repos/example/allowed/issues/1 -X PATCH -f state=closed 2>&1); then
  printf 'expected cross-repository API mutation to fail\n' >&2
  exit 1
fi
assert_contains "$API_OUTPUT" 'refusing PATCH outside repos/example/second/'

if URL_OUTPUT=$(cd "$TEST_ROOT/repository" && \
  "$AGENT_GH" pr close https://github.com/example/allowed/pull/1 2>&1); then
  printf 'expected cross-repository PR URL to fail\n' >&2
  exit 1
fi
assert_contains "$URL_OUTPUT" 'refusing repository URL outside example/second'

printf 'agent wrapper registry tests passed\n'
