# An agent-like request handled by CPython in a cell (spec `workloads`):
# import the standard library and warm up, say "warm" and pause briefly (a
# host may freeze the cell there as a template), then answer one tool call:
# parse its JSON, run regular expressions and string operations over its
# text, compute, and reply in JSON. Deterministic: the same output on Linux
# and in a cell. A copy of the frozen cell reseeds `random` first (design
# D22 of `add-cell-hierarchy`); with the argument `random`, it then prints
# `random.random()` and 16 bytes of `os.urandom`, which every copy draws
# anew.
import binascii
import datetime
import json
import math
import os
import random
import re
import struct
import sys
import time

WORDS = re.compile(r"[a-z]+")
EMAIL = re.compile(r"\b([a-z]+)@([a-z]+\d)\.example\b")
DATE = re.compile(r"\b(\d{4})-(\d{2})-(\d{2})\b")
VOCAB = ["cell", "fork", "agent", "kernel", "snapshot", "resume", "page", "file"]


def tool_call(seed):
    """The request an agent would send: a JSON tool call with a document."""
    rng = random.Random(seed)
    lines = []
    for i in range(40):
        words = " ".join(rng.choice(VOCAB) for _ in range(12))
        day = datetime.date(2026, 1, 1) + datetime.timedelta(days=rng.randrange(365))
        lines.append("%s, ask %s@team%d.example by %s" % (words, VOCAB[i % 8], i % 3, day))
    return json.dumps(
        {
            "tool": "summarize",
            "arguments": {"document": "\n".join(lines), "top": 3, "since": "2026-06-01"},
        }
    )


def warm():
    """Imports are done; touch every path once (what a template keeps)."""
    request = tool_call(1)
    answer(request, quiet=True)
    return tool_call(42)


def answer(request, quiet=False):
    call = json.loads(request)
    args = call["arguments"]
    text = args["document"]
    freq = {}
    for word in WORDS.findall(text):
        if word in VOCAB:
            freq[word] = freq.get(word, 0) + 1
    top = sorted(freq.items(), key=lambda kv: (-kv[1], kv[0]))[: args["top"]]
    contacts = sorted({"%s@%s" % m for m in EMAIL.findall(text)})
    since = datetime.date.fromisoformat(args["since"])
    dates = [datetime.date(int(y), int(m), int(d)) for y, m, d in DATE.findall(text)]
    later = [d for d in dates if d >= since]
    gaps = [(b - a).days for a, b in zip(sorted(later), sorted(later)[1:])]
    mean = math.fsum(gaps) / len(gaps)
    spread = math.sqrt(math.fsum((g - mean) ** 2 for g in gaps) / len(gaps))
    digest = struct.pack("<II", binascii.crc32(text.encode()), len(text))
    reply = {
        "tool": call["tool"],
        "top_words": [w.upper() for w, _ in top],
        "counts": [n for _, n in top],
        "contacts": len(contacts),
        "first_contact": contacts[0].title(),
        "dates_since": len(later),
        "last_date": max(later).strftime("%d %b %Y"),
        "mean_gap_days": round(mean, 4),
        "gap_spread": round(spread, 4),
        "digest": binascii.hexlify(digest).decode(),
    }
    if not quiet:
        print(json.dumps(reply, sort_keys=True))


def generation():
    """The cell's copy generation, which changes in every copy of it (a
    fork, a cell from a template, a resume); None outside Rhizo."""
    try:
        with open("/run/rhizo/generation", "rb") as f:
            return f.read()
    except OSError:
        return None


def main():
    request = warm()
    seen = generation()
    print("warm", flush=True)
    time.sleep(0.001)
    if generation() != seen:
        random.seed()  # a copy: from its own `getrandom`
    answer(request)
    print("cpython ok")
    if "random" in sys.argv[1:]:
        print(random.random(), os.urandom(16).hex())


if __name__ == "__main__":
    main()
