#!/usr/bin/env python3
# kite-latency.py v1.0.0
# Read-only Zerodha Kite connectivity / latency check. Never places orders and
# never prints credentials (api key + access token are read from Redis).
#
# Measures, over IPv4 (the route order calls use):
#   * DNS lookup, TCP connect and TLS handshake to api.kite.trade
#   * Kite REST round trips on one kept-alive connection for the calls the
#     pre-order safety check makes (orders, trades, positions, margins)
#   * Redis WAITAOF (disk-sync wait; since 2.21.1 only at start-up, token save and
#     cooldown, never on the order path)
# Usage: ./network-utility/kite-latency.py [rounds]   (default 5 rounds)
import http.client, json, socket, ssl, statistics, subprocess, sys, time

HOST = "api.kite.trade"
PATHS = ["/orders", "/trades", "/portfolio/positions", "/user/margins"]
ROUNDS = int(sys.argv[1]) if len(sys.argv) > 1 else 5


def redis(*args):
    return subprocess.check_output(["redis-cli", *args], text=True).strip()


def ms(t0):
    return (time.perf_counter() - t0) * 1000


def main():
    api_key, token = redis("GET", "susanta:kite_api_key"), redis("GET", "susanta:kite_access_token")
    if not api_key or not token:
        sys.exit("Kite credentials not found in Redis (run the Kite login first)")
    headers = {"X-Kite-Version": "3", "Authorization": f"token {api_key}:{token}"}

    t = time.perf_counter()
    ip = socket.getaddrinfo(HOST, 443, socket.AF_INET)[0][4][0]
    dns = ms(t)
    t = time.perf_counter()
    raw = socket.create_connection((ip, 443), timeout=10)
    tcp = ms(t)
    t = time.perf_counter()
    sock = ssl.create_default_context().wrap_socket(raw, server_hostname=HOST)
    tls = ms(t)
    print(f"Kite {HOST} via IPv4 {ip}")
    print(f"  DNS {dns:6.1f} ms   TCP connect {tcp:6.1f} ms   TLS handshake {tls:6.1f} ms")

    conn = http.client.HTTPSConnection(HOST, 443, timeout=10)
    conn.sock = sock
    results = {p: [] for p in PATHS}
    for _ in range(ROUNDS):
        for p in PATHS:
            t = time.perf_counter()
            conn.request("GET", p, headers=headers)
            resp = conn.getresponse()
            body = resp.read()
            results[p].append(ms(t))
            if resp.status != 200:
                msg = json.loads(body or b"{}").get("message", "")
                sys.exit(f"{p}: HTTP {resp.status} {msg}")
        time.sleep(0.3)  # stay well inside Kite's 10 requests/second
    print(f"  REST round trips ({ROUNDS} rounds, kept-alive connection):")
    for p, v in results.items():
        print(f"    {p:<22} median {statistics.median(v):6.1f} ms   min {min(v):6.1f}   max {max(v):6.1f}")
    snapshot = sum(statistics.median(v) for v in results.values()) + statistics.median(results["/orders"])
    print(f"  pre-order safety check (5 calls back to back) ~ {snapshot:.0f} ms")

    # Redis disk-sync wait (WAITAOF): start-up, token save and cooldown only.
    s = socket.create_connection(("127.0.0.1", 6379))

    def cmd(*a):
        s.sendall(("*%d\r\n" % len(a) + "".join(f"${len(str(x).encode())}\r\n{x}\r\n" for x in a)).encode())
        return s.recv(4096)

    waits = []
    for i in range(10):
        cmd("SET", "scratch:kite-latency-probe", i)
        t = time.perf_counter()
        cmd("WAITAOF", 1, 0, 2000)
        waits.append(ms(t))
        time.sleep(0.05)
    cmd("DEL", "scratch:kite-latency-probe")
    fsync = redis("CONFIG", "GET", "appendfsync").split()[-1]
    print(f"Redis WAITAOF (appendfsync {fsync}): median {statistics.median(waits):.1f} ms   max {max(waits):.1f} ms")
    if statistics.median(waits) > 50:
        print("  WARNING: slow disk-sync wait adds this much to every order; set appendfsync always")


if __name__ == "__main__":
    main()
