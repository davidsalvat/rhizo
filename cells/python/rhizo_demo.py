# An agent-style request handled by MicroPython in a cell (spec `workloads`):
# warm up (imports, a prime table), say "warm" and pause briefly (a host may
# freeze the cell there as a template), then answer one JSON request.
# Deterministic output, identical on Linux and in a cell.
import json
import time


def primes_below(n):
    sieve = bytearray([1]) * n
    sieve[0] = sieve[1] = 0
    i = 2
    while i * i < n:
        if sieve[i]:
            for j in range(i * i, n, i):
                sieve[j] = 0
        i += 1
    return [k for k in range(n) if sieve[k]]


def words(count, seed):
    vocab = ["cell", "fork", "agent", "kernel", "snapshot", "resume", "page", "file"]
    out = []
    for _ in range(count):
        seed = (seed * 1103515245 + 12345) % 2147483648
        out.append(vocab[(seed >> 16) % len(vocab)])
    return " ".join(out)


def handle(request, table):
    data = json.loads(request)
    freq = {}
    for w in data["text"].split():
        freq[w] = freq.get(w, 0) + 1
    top = sorted(freq.items(), key=lambda kv: (-kv[1], kv[0]))[:3]
    below = [p for p in table if p < data["n"]]
    return "primes below %d: %d, last %d; top words: %s" % (
        data["n"],
        len(below),
        below[-1],
        ", ".join("%s=%d" % kv for kv in top),
    )


def main():
    table = primes_below(20000)
    print("warm")
    time.sleep(0.001)
    request = json.dumps({"n": 10000, "text": words(500, 42)})
    print(handle(request, table))
    print("python ok")
