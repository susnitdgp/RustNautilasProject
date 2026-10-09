#!/usr/bin/env bash
# redis-fsync-always.sh v1.0.0
# Sets Redis "appendfsync always" now and saves it to the Redis config file,
# so the disk-sync wait (WAITAOF) before every kite-node order returns in
# under a millisecond instead of up to ~1 s ("everysec").
# Safe to run any time; records remain written to disk before each order.
# Undo: redis-cli CONFIG SET appendfsync everysec && redis-cli CONFIG REWRITE
set -euo pipefail
DIR="$(dirname "$(readlink -f "$0")")"

echo "Before: appendfsync $(redis-cli CONFIG GET appendfsync | tail -1)"
redis-cli CONFIG SET appendfsync always >/dev/null
redis-cli CONFIG REWRITE >/dev/null
echo "After:  appendfsync $(redis-cli CONFIG GET appendfsync | tail -1) (saved to the Redis config file)"
exec "$DIR/redis-fsync-check.py" 10
