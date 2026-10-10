#!/usr/bin/env python3
# redis-fsync-check.py v1.1.0
# Read-only check of Redis persistence and the disk-sync wait (WAITAOF). Since
# kite-node 2.21.1 no order waits for it (only startup, the daily token save and
# a rate-limit cooldown do), so "everysec" is expected. Uses one scratch key.
# Usage: ./redis-utility/redis-fsync-check.py [samples]   (default 20)
import socket, statistics, subprocess, sys, time

SAMPLES = int(sys.argv[1]) if len(sys.argv) > 1 else 20
SCRATCH = "scratch:redis-fsync-check"


def cli(*args):
    return subprocess.check_output(["redis-cli", *args], text=True).strip()


def main():
    info = dict(
        line.split(":", 1)
        for line in cli("INFO", "persistence").splitlines()
        if ":" in line
    )
    version = next(l.split(":", 1)[1] for l in cli("INFO", "server").splitlines() if l.startswith("redis_version:"))
    fsync = cli("CONFIG", "GET", "appendfsync").split()[-1]
    print(f"Redis {version}")
    print(f"  appendonly (AOF)      {'yes' if info.get('aof_enabled') == '1' else 'NO'}")
    print(f"  appendfsync           {fsync}")
    print(f"  last AOF write        {info.get('aof_last_write_status', '?')}")
    print(f"  delayed fsyncs        {info.get('aof_delayed_fsync', '?')}")

    s = socket.create_connection(("127.0.0.1", 6379))

    def cmd(*a):
        s.sendall(("*%d\r\n" % len(a) + "".join(f"${len(str(x).encode())}\r\n{x}\r\n" for x in a)).encode())
        return s.recv(4096)

    waits = []
    for i in range(SAMPLES):
        cmd("SET", SCRATCH, i)
        t = time.perf_counter()
        reply = cmd("WAITAOF", 1, 0, 2000)
        waits.append((time.perf_counter() - t) * 1000)
        if not reply.startswith(b"*2\r\n:1"):
            print(f"  WAITAOF not confirmed: {reply!r}")
        time.sleep(0.05)
    cmd("DEL", SCRATCH)
    med = statistics.median(waits)
    print(f"  WAITAOF after a write median {med:.1f} ms   max {max(waits):.1f} ms   ({SAMPLES} samples)")
    if info.get("aof_enabled") != "1":
        print("  PROBLEM: AOF is off; kite-node refuses to place orders without it")
    elif fsync == "everysec":
        print(f"  OK: everysec; orders do not wait for the disk (startup/token/cooldown wait ~{med:.0f} ms once)")
    else:
        print(f"  NOTE: appendfsync {fsync}; every Redis write waits for the disk. Run ./redis-utility/redis-fsync-everysec.sh")


if __name__ == "__main__":
    main()
