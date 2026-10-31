#!/usr/bin/env bash
# Create a Codex heartbeat automation that keeps re-prompting this workspace's
# most recently updated thread, so unfinished work continues while you are away.
#
# Usage:
#   scripts/codex-overnight.sh [interval-minutes] [count]
#   scripts/codex-overnight.sh --stop
#
# Each automation run still has its own tool budget, so progress happens in
# steps. This writes only to ~/.codex/automations/<id>/automation.toml.

set -euo pipefail

AUTOMATION_ID="alpha-harness-overnight"
AUTOMATION_DIR="${HOME}/.codex/automations/${AUTOMATION_ID}"
STATE_DB="${HOME}/.codex/state_5.sqlite"
WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--stop" ]]; then
    rm -rf "${AUTOMATION_DIR}"
    echo "removed ${AUTOMATION_DIR}"
    exit 0
fi

INTERVAL="${1:-15}"
COUNT="${2:-32}"
PROMPT="继续推进组合解耦计划：先读 docs/composition-decoupling.md，按其中顺序实现剩余项；每完成一项就提交一次；始终保证 cargo check -j 1 --workspace --all-targets 通过；不要停在不可编译的状态。"

THREAD_ID="$(python3 - "${STATE_DB}" "${WORKSPACE_ROOT}" <<'PY'
import sqlite3, sys

db, root = sys.argv[1], sys.argv[2]
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
row = con.execute(
    "select id from threads where cwd = ? order by updated_at desc limit 1",
    (root,),
).fetchone()
print(row[0] if row else "")
PY
)"

if [[ -z "${THREAD_ID}" ]]; then
    echo "no Codex thread found for ${WORKSPACE_ROOT}" >&2
    exit 1
fi

mkdir -p "${AUTOMATION_DIR}"
now_ms="$(($(date +%s) * 1000))"
cat > "${AUTOMATION_DIR}/automation.toml" <<TOML
version = 1
id = "${AUTOMATION_ID}"
kind = "heartbeat"
name = "alpha-harness composition decoupling"
prompt = "${PROMPT}"
status = "ACTIVE"
rrule = "FREQ=MINUTELY;INTERVAL=${INTERVAL};COUNT=${COUNT}"
notification_policy = "failed_runs_only"
target_thread_id = "${THREAD_ID}"
created_at = ${now_ms}
updated_at = ${now_ms}
TOML

echo "created ${AUTOMATION_DIR}/automation.toml"
echo "  thread:   ${THREAD_ID}"
echo "  schedule: every ${INTERVAL} min, ${COUNT} times"
echo "  stop:     scripts/codex-overnight.sh --stop"
