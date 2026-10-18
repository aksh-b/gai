"""Check knowledge search quality on a local gai server: ingest a file into a
fresh collection, then ask questions worded differently from the text and
report where the passage that answers each one ranks.

    GAI_URL=http://127.0.0.1:8485 python scripts/try-search.py STATUS.md
"""

import json
import os
import sys
import time
import urllib.request

BASE = os.environ.get("GAI_URL", "http://127.0.0.1:8485")
KEY = os.environ.get("GAI_KEY", "")

# (question, words that appear only in the passage that answers it)
CASES = [
    ("how do I stop two copies of the server from running the same scheduled job twice", ["advance_trigger"]),
    ("where are pictures and sound files kept", ["Blobs live in table"]),
    ("what happens when a webhook arrives without the right secret", ["X-Gai-Token"]),
    ("can I go back to an older version of an agent", ["Rollback"]),
    ("how does gai decide which model answers a request", ["router/<name>"]),
]


def call(method, path, body=None):
    headers = {"content-type": "application/json"}
    if KEY:
        headers["authorization"] = f"Bearer {KEY}"
    req = urllib.request.Request(BASE + path, method=method, headers=headers,
                                 data=None if body is None else json.dumps(body).encode())
    with urllib.request.urlopen(req, timeout=900) as r:
        return json.loads(r.read() or b"null")


def main(path):
    name = f"search-check-{int(time.time())}"
    call("POST", "/v1/collections", {"name": name})
    t0 = time.time()
    doc = call("POST", f"/v1/collections/{name}/documents",
               {"source": os.path.basename(path), "text": open(path, encoding="utf8").read()})
    print(f"ingested {doc['document']['chunks']} chunks in {time.time() - t0:.0f}s into {name}")
    for mode in ["vector", "hybrid", "text"]:
        ranks = []
        for q, marks in CASES:
            hits = call("POST", f"/v1/collections/{name}/search", {"query": q, "mode": mode, "k": 10})
            rank = next((i + 1 for i, h in enumerate(hits) if any(m in h["text"] for m in marks)), None)
            ranks.append(rank)
        top1 = sum(1 for r in ranks if r == 1)
        top3 = sum(1 for r in ranks if r and r <= 3)
        print(f"{mode:<7} answer ranks {ranks}  -> first: {top1}/{len(CASES)}, top 3: {top3}/{len(CASES)}")
    call("DELETE", f"/v1/collections/{name}")


if __name__ == "__main__":
    main(sys.argv[1])
