#!/usr/bin/env bash
# redis-fsync-everysec.sh v1.0.0
# Sets local Redis "appendfsync everysec" now and saves it to the Redis config
# file. Since kite-node 2.21.1 nothing in the order path waits for the disk, so
# every Redis write (the order-rate budget before each order) returns in
# ~0.06 ms instead of ~2.8 ms. A Redis crash can lose at most the last second
# of writes (budget counts, dashboard data); the Kite token is still confirmed
# on disk when it is saved.
# Undo: redis-cli CONFIG SET appendfsync always && redis-cli CONFIG REWRITE
set -euo pipefail
DIR="$(dirname "$(readlink -f "$0")")"

echo "Before: appendfsync $(redis-cli CONFIG GET appendfsync | tail -1)"
redis-cli CONFIG SET appendfsync everysec >/dev/null
redis-cli CONFIG REWRITE >/dev/null
echo "After:  appendfsync $(redis-cli CONFIG GET appendfsync | tail -1) (saved to the Redis config file)"
exec "$DIR/redis-fsync-check.py" 10
