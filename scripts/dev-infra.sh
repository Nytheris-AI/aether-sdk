#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/infra/docker-compose.yml"
ENV_FILE="${REPO_ROOT}/.env"

if [[ -f "${ENV_FILE}" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "${ENV_FILE}"
  set +a
fi

SQLITE_PATH="${AETHER_SQLITE_PATH:-.aether/dev.db}"
if [[ "${SQLITE_PATH}" != /* ]]; then
  SQLITE_PATH="${REPO_ROOT}/${SQLITE_PATH}"
fi

compose() {
  docker compose -f "${COMPOSE_FILE}" "$@"
}

init_sqlite() {
  local db_dir
  db_dir="$(dirname "${SQLITE_PATH}")"
  mkdir -p "${db_dir}"

  if command -v sqlite3 >/dev/null 2>&1; then
    sqlite3 "${SQLITE_PATH}" "PRAGMA journal_mode=WAL;" >/dev/null
    echo "SQLite initialized at ${SQLITE_PATH} (WAL mode)"
  else
    echo "sqlite3 CLI not found; created ${db_dir} directory (skipping WAL initialization)"
  fi
}

usage() {
  cat <<EOF
Usage: $(basename "$0") <command>

Commands:
  up           Start all local dev services and initialize SQLite
  down         Stop all local dev services
  status       Show container and SQLite status
  reset        Stop services, remove Docker volumes, and delete local SQLite DB
  init-sqlite  Initialize local SQLite database with WAL mode
EOF
}

cmd="${1:-}"

case "${cmd}" in
  up)
    compose up -d
    init_sqlite
    ;;
  down)
    compose down
    ;;
  status)
    compose ps
    if [[ -f "${SQLITE_PATH}" ]]; then
      echo "SQLite DB: ${SQLITE_PATH} (present)"
    else
      echo "SQLite DB: ${SQLITE_PATH} (not initialized)"
    fi
    ;;
  reset)
    compose down -v
    rm -f "${SQLITE_PATH}" "${SQLITE_PATH}-wal" "${SQLITE_PATH}-shm"
    echo "Reset complete: volumes and local SQLite DB removed."
    ;;
  init-sqlite)
    init_sqlite
    ;;
  -h|--help|"")
    usage
    [[ -n "${cmd}" ]] || exit 1
    ;;
  *)
    echo "Unknown command: ${cmd}" >&2
    usage >&2
    exit 1
    ;;
esac
