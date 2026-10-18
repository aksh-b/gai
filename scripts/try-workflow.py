"""Run a stored workflow on a local gai server, approve its human steps,
and print what each node did.

    GAI_URL=http://127.0.0.1:8485 GAI_KEY= python scripts/try-workflow.py research-brief topic="Rust"
"""

import json
import os
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
KEY = os.environ.get("GAI_KEY")
if KEY is None:
    KEY = open(os.path.join(HERE, "..", ".admin-key")).read().strip()
BASE = os.environ.get("GAI_URL", "http://127.0.0.1:8484")


def call(method, path, body=None):
    req = urllib.request.Request(
        BASE + path, method=method, data=None if body is None else json.dumps(body).encode(),
        headers={"content-type": "application/json", **({"authorization": f"Bearer {KEY}"} if KEY else {})})
    with urllib.request.urlopen(req, timeout=600) as r:
        return json.loads(r.read() or b"null")


def main():
    name = sys.argv[1]
    inputs = dict(a.split("=", 1) for a in sys.argv[2:])
    t0 = time.time()
    run = call("POST", f"/v1/workflows/{name}/runs", {"inputs": inputs, "mode": "background"})
    rid = run["run_id"]
    print(f"run {rid}")
    while True:
        for a in call("GET", "/v1/approvals"):
            if a["run_id"] == rid:
                prompt = (a["input"] or {}).get("prompt", a["input"])
                print(f"[{time.time() - t0:.0f}s] approving {a['call_id']}: {str(prompt)[:300]!r}")
                call("POST", f"/v1/runs/{rid}/approvals/{a['call_id']}", {"approved": True})
        r = call("GET", f"/v1/runs/{rid}")
        if r["status"] in ("completed", "failed", "cancelled", "budget_exceeded"):
            break
        time.sleep(5)
    print(f"{r['status']} after {time.time() - t0:.0f}s")
    for s in call("GET", f"/v1/runs/{rid}/steps"):
        out = s.get("output") or {}
        print(f"  node {s['name']:<10} {out.get('status')}: {str(out.get('output') or out.get('error'))[:160]!r}")
    print("OUTPUT:\n" + (r.get("output") if isinstance(r.get("output"), str) else json.dumps(r.get("output"))) if r.get("output") else f"ERROR: {r.get('error')}")


if __name__ == "__main__":
    main()
