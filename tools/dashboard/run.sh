#!/usr/bin/env bash
# run.sh v1.0.0
# Starts the Streamlit live dashboard on 127.0.0.1:8501 (localhost only: the page has no
# login, so reach it through an SSH tunnel, e.g. from your PC:
#   ssh -L 8501:127.0.0.1:8501 ubuntu@<box>   then open http://localhost:8501 ).
# First run creates tools/dashboard/.venv with streamlit + redis.
#   DASH_PREFIX=kite-demo tools/dashboard/run.sh   # open on the demo data
set -euo pipefail
cd "$(dirname "$0")/../.."
VENV=tools/dashboard/.venv
if [[ ! -x "$VENV/bin/streamlit" ]]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install -q --upgrade pip
  "$VENV/bin/pip" install -q -r tools/dashboard/requirements.txt
fi
[[ -f config/dashboard.json ]] || { echo 'config/dashboard.json missing (see config/dashboard.example.json)' >&2; exit 1; }
exec "$VENV/bin/streamlit" run tools/dashboard/app.py \
  --server.address 127.0.0.1 --server.port "${DASH_PORT:-8501}" --server.headless true \
  --browser.gatherUsageStats false --theme.base dark
