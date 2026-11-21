#!/usr/bin/env sh
set -eu

REPO="muninn-hq/aether-sdk"
BIN_NAME="aether-mcp"
INSTALL_DIR="${AETHER_INSTALL_DIR:-${HOME}/.local/bin}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Error: required command '$1' not found." >&2
    exit 1
  fi
}

need_cmd curl
need_cmd tar
need_cmd uname

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)
    case "${ARCH}" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)
        echo "Error: unsupported Linux architecture '${ARCH}'. Supported: x86_64, aarch64." >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "${ARCH}" in
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *)
        echo "Error: unsupported macOS architecture '${ARCH}'." >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Error: unsupported OS '${OS}'. Use install.ps1 on Windows." >&2
    exit 1
    ;;
esac

if [ -n "${AETHER_VERSION:-}" ]; then
  TAG="${AETHER_VERSION}"
else
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  if [ -z "${TAG}" ]; then
    echo "Error: could not resolve latest release tag from GitHub." >&2
    exit 1
  fi
fi

ARCHIVE="${BIN_NAME}-${TAG}-${TARGET}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "Downloading ${BIN_NAME} ${TAG} (${TARGET})..."
if ! curl -fsSL "${BASE_URL}/${ARCHIVE}" -o "${TMP_DIR}/${ARCHIVE}" \
  || ! curl -fsSL "${BASE_URL}/${ARCHIVE}.sha256" -o "${TMP_DIR}/${ARCHIVE}.sha256"; then
  echo "Error: could not download ${ARCHIVE}." >&2
  echo "Release ${TAG} may not include a ${TARGET} build. See https://github.com/${REPO}/releases" >&2
  exit 1
fi

(
  cd "${TMP_DIR}"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${ARCHIVE}.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${ARCHIVE}.sha256"
  else
    echo "Error: neither sha256sum nor shasum found for checksum verification." >&2
    exit 1
  fi
)

tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${ARCHIVE}"
mkdir -p "${INSTALL_DIR}"
install -m 755 "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"

echo "Installed ${BIN_NAME} ${TAG} to ${INSTALL_DIR}/${BIN_NAME}"
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo "Add ${INSTALL_DIR} to your PATH:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac

# Releases from before `setup` existed do not list it in --help (stdin is closed so an old
# build that ignores --help exits instead of waiting for input).
HAS_SETUP=""
if "${INSTALL_DIR}/${BIN_NAME}" --help </dev/null 2>&1 | grep -q '^  setup'; then
  HAS_SETUP=1
fi

# Offer the guided setup when a terminal is available. `curl | sh` has no stdin, so ask and run
# setup on /dev/tty. Set AETHER_NO_SETUP=1 to skip.
if [ -n "${HAS_SETUP}" ] && [ -z "${AETHER_NO_SETUP:-}" ] && ( : </dev/tty ) 2>/dev/null; then
  printf "\nRun the guided setup now? [Y/n] "
  read -r ANSWER </dev/tty || ANSWER="n"
  case "${ANSWER}" in
    n|N|no|NO) ;;
    *)
      "${INSTALL_DIR}/${BIN_NAME}" setup </dev/tty
      exit $?
      ;;
  esac
fi

echo ""
echo "Next steps:"
if [ -n "${HAS_SETUP}" ]; then
  echo "  ${INSTALL_DIR}/${BIN_NAME} setup"
  echo "  # or install the service yourself:"
fi
echo "  ${INSTALL_DIR}/${BIN_NAME} service install --vault /path/to/your/vault"
echo "  # or run in the foreground:"
echo "  ${INSTALL_DIR}/${BIN_NAME} --vault /path/to/your/vault --http-addr 127.0.0.1:14771 --http-only"
