#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "${REPO_ROOT}" ]]; then
  echo "error: run inside a git repo" >&2
  exit 1
fi

MSG_DIR="$REPO_ROOT/ops/relay/messages"
STATE_DIR="$REPO_ROOT/.git/ops-relay-state"
mkdir -p "$MSG_DIR" "$STATE_DIR"

usage() {
  cat <<USAGE
Usage:
  relay.sh send --from ROLE --to ROLE|all --kind KIND --text "MESSAGE"
  relay.sh inbox --for ROLE [--pull]
  relay.sh watch --for ROLE [--interval SEC] [--pull] [--say]
  relay.sh heartbeat --from ROLE [--to ROLE|all]
  relay.sh roundtrip-start --from ROLE --to ROLE [--timeout SEC]

Examples:
  relay.sh send --from laptop --to studio --kind ping --text "ready?"
  relay.sh watch --for laptop --interval 5 --pull --say
  relay.sh heartbeat --from laptop --to studio
USAGE
}

pull_latest() {
  git -C "$REPO_ROOT" pull --ff-only --quiet >/dev/null 2>&1 || true
}

push_changes() {
  local msg="$1"
  if git -C "$REPO_ROOT" diff --cached --quiet && git -C "$REPO_ROOT" diff --quiet; then
    return 0
  fi
  git -C "$REPO_ROOT" add ops/relay/messages >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" commit -m "$msg" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" push --quiet >/dev/null 2>&1 || true
}

field() {
  local key="$1"
  local file="$2"
  awk -F': ' -v k="$key" '$1==k {print substr($0, index($0, ": ")+2); exit}' "$file"
}

mark_seen() {
  local role="$1"
  local id="$2"
  local seen="$STATE_DIR/seen_${role}.txt"
  touch "$seen"
  if ! grep -Fxq "$id" "$seen"; then
    echo "$id" >> "$seen"
  fi
}

is_seen() {
  local role="$1"
  local id="$2"
  local seen="$STATE_DIR/seen_${role}.txt"
  [[ -f "$seen" ]] && grep -Fxq "$id" "$seen"
}

emit_alert() {
  local say_enabled="$1"
  printf '\a'
  if [[ "$say_enabled" == "1" ]] && command -v say >/dev/null 2>&1; then
    say "Ops relay message" >/dev/null 2>&1 || true
  fi
}

show_new_for_role() {
  local role="$1"
  local say_enabled="${2:-0}"
  local found=0
  shopt -s nullglob
  for f in "$MSG_DIR"/*.md; do
    local to from kind id ts
    to="$(field to "$f")"
    if [[ "$to" != "$role" && "$to" != "all" ]]; then
      continue
    fi
    id="$(field id "$f")"
    if is_seen "$role" "$id"; then
      continue
    fi
    from="$(field from "$f")"
    kind="$(field kind "$f")"
    ts="$(field timestamp_utc "$f")"
    echo "--- NEW MESSAGE [$id] ---"
    echo "time: $ts"
    echo "from: $from"
    echo "to:   $to"
    echo "kind: $kind"
    echo "body:"
    awk 'f{print} /^---$/{c++; if(c==2){f=1; next}}' "$f"
    echo
    mark_seen "$role" "$id"
    found=1
    emit_alert "$say_enabled"
  done
  shopt -u nullglob
  return $found
}

make_message() {
  local from="$1"
  local to="$2"
  local kind="$3"
  local text="$4"
  local now id host file
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  id="$(date -u +%Y%m%dT%H%M%SZ)_${from}_to_${to}_${kind}_$RANDOM"
  host="$(hostname -s 2>/dev/null || echo unknown-host)"
  file="$MSG_DIR/${id}.md"
  cat > "$file" <<MSG
---
id: $id
timestamp_utc: $now
from: $from
to: $to
kind: $kind
host: $host
---
$text
MSG
  echo "$file"
}

cmd_send() {
  local from="" to="" kind="" text=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --from) from="$2"; shift 2;;
      --to) to="$2"; shift 2;;
      --kind) kind="$2"; shift 2;;
      --text) text="$2"; shift 2;;
      *) echo "error: unknown arg $1" >&2; exit 2;;
    esac
  done
  [[ -n "$from" && -n "$to" && -n "$kind" && -n "$text" ]] || { echo "error: missing required args" >&2; exit 2; }
  local file
  file="$(make_message "$from" "$to" "$kind" "$text")"
  git -C "$REPO_ROOT" add "$file"
  push_changes "ops(relay): $kind $from->$to"
  echo "sent: $file"
}

cmd_inbox() {
  local role="" do_pull=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --for) role="$2"; shift 2;;
      --pull) do_pull=1; shift;;
      *) echo "error: unknown arg $1" >&2; exit 2;;
    esac
  done
  [[ -n "$role" ]] || { echo "error: --for ROLE required" >&2; exit 2; }
  if [[ "$do_pull" == "1" ]]; then
    pull_latest
  fi
  show_new_for_role "$role" 0 || true
}

cmd_watch() {
  local role="" interval=5 do_pull=0 say_enabled=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --for) role="$2"; shift 2;;
      --interval) interval="$2"; shift 2;;
      --pull) do_pull=1; shift;;
      --say) say_enabled=1; shift;;
      *) echo "error: unknown arg $1" >&2; exit 2;;
    esac
  done
  [[ -n "$role" ]] || { echo "error: --for ROLE required" >&2; exit 2; }
  echo "watching relay inbox for role=$role interval=${interval}s pull=${do_pull} say=${say_enabled}"
  while true; do
    if [[ "$do_pull" == "1" ]]; then
      pull_latest
    fi
    show_new_for_role "$role" "$say_enabled" || true
    sleep "$interval"
  done
}

cmd_heartbeat() {
  local from="" to="all"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --from) from="$2"; shift 2;;
      --to) to="$2"; shift 2;;
      *) echo "error: unknown arg $1" >&2; exit 2;;
    esac
  done
  [[ -n "$from" ]] || { echo "error: --from ROLE required" >&2; exit 2; }
  local branch head now
  branch="$(git -C "$REPO_ROOT" branch --show-current)"
  head="$(git -C "$REPO_ROOT" rev-parse --short HEAD)"
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cmd_send --from "$from" --to "$to" --kind heartbeat --text "heartbeat ok at $now (branch=$branch head=$head)"
}

cmd_roundtrip_start() {
  local from="" to="" timeout=120
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --from) from="$2"; shift 2;;
      --to) to="$2"; shift 2;;
      --timeout) timeout="$2"; shift 2;;
      *) echo "error: unknown arg $1" >&2; exit 2;;
    esac
  done
  [[ -n "$from" && -n "$to" ]] || { echo "error: --from ROLE --to ROLE required" >&2; exit 2; }

  local token
  token="rt-$(date -u +%Y%m%dT%H%M%SZ)-$RANDOM"
  cmd_send --from "$from" --to "$to" --kind ping --text "roundtrip token=$token; reply with kind=pong and same token"

  echo ""
  echo "Send this on the other machine (or other Codex):"
  echo "  cd $REPO_ROOT && ./ops/tools/relay.sh send --from $to --to $from --kind pong --text 'roundtrip token=$token acknowledged'"
  echo ""
  echo "Waiting up to ${timeout}s for pong token=$token ..."

  local start now elapsed
  start="$(date +%s)"
  while true; do
    pull_latest
    shopt -s nullglob
    for f in "$MSG_DIR"/*.md; do
      local f_kind f_to body
      f_kind="$(field kind "$f")"
      f_to="$(field to "$f")"
      body="$(awk 'f{print} /^---$/{c++; if(c==2){f=1; next}}' "$f")"
      if [[ "$f_kind" == "pong" && "$f_to" == "$from" && "$body" == *"roundtrip token=$token"* ]]; then
        echo "roundtrip: PASS (token=$token)"
        shopt -u nullglob
        return 0
      fi
    done
    shopt -u nullglob
    now="$(date +%s)"
    elapsed=$((now - start))
    if (( elapsed >= timeout )); then
      echo "roundtrip: TIMEOUT (token=$token)"
      return 1
    fi
    sleep 2
  done
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

cmd="$1"
shift
case "$cmd" in
  send) cmd_send "$@" ;;
  inbox) cmd_inbox "$@" ;;
  watch) cmd_watch "$@" ;;
  heartbeat) cmd_heartbeat "$@" ;;
  roundtrip-start) cmd_roundtrip_start "$@" ;;
  -h|--help|help) usage ;;
  *) echo "error: unknown command $cmd" >&2; usage; exit 2 ;;
esac
