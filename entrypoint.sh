#!/bin/bash
set -e

PLUGINS_DIR="${PLUGINS_DIR:-/app/plugins}"
PLUGINS_REPO="${PLUGINS_REPO:-https://github.com/razorpay/claude-plugins.git}"
PLUGINS_BRANCH="${PLUGINS_BRANCH:-master}"

echo "==> Cloning plugins from ${PLUGINS_REPO} (branch: ${PLUGINS_BRANCH})"

if [ -d "${PLUGINS_DIR}/.git" ]; then
    echo "    Plugins dir exists, pulling latest..."
    cd "${PLUGINS_DIR}"
    git fetch origin "${PLUGINS_BRANCH}" --depth 1
    git reset --hard "origin/${PLUGINS_BRANCH}"
    cd -
else
    echo "    Fresh clone..."
    git clone --depth 1 --branch "${PLUGINS_BRANCH}" "${PLUGINS_REPO}" "${PLUGINS_DIR}"
fi

echo "==> Plugins synced. Starting oac-slack-bot..."

# Set PLUGIN_DIRS to point at the cloned plugins
export PLUGIN_DIRS="${PLUGINS_DIR}/plugins"

exec python -m oac_slack_bot
