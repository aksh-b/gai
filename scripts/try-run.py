"""Run an agent on the local gai server and print what it did, step by step.

    python scripts/try-run.py <agent|-> "task"     (- for the default assistant)
    python scripts/try-run.py --show <run_id>      (an existing run)
"""

import json
import os
import sys
import time
import urllib.request

# Model output has arrows and quotes the Windows console code page lacks.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
# GAI_KEY overrides; an empty GAI_KEY sends none (for an open test server).
KEY = os.environ.get("GAI_KEY")
if KEY is None:
    KEY = open(os.path.join(HERE, "..", ".admin-key")).read().strip()
BASE = os.environ.get("GAI_URL", "http://127.0.0.1:8484")


def call(method, path, body=None, timeout=1800):
    req = urllib.request.Request(
        BASE + path,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"content-type": "application/json", **({"authorization": f"Bearer {KEY}"} if KEY else {})},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return {"error": f"HTTP {e.code}: {e.read().decode()[:500]}"}


def text_of(blocks):
    return " ".join(b.get("text", "") for b in blocks or [] if isinstance(b, dict)).replace("\n", " ").strip()


def show(run_id):
    run = call("GET", f"/v1/runs/{run_id}")
    for s in call("GET", f"/v1/runs/{run_id}/steps"):
        out = s.get("output") or {}
        kind, name = s["kind"], s.get("name")
        if isinstance(out, str):
            print(f"  {kind:<10} {name}: {out[:200]}")
        elif kind == "model":
            model = (out.get("routed") or {}).get("model", name)
            calls = [b["name"] for b in out.get("content", []) if b.get("type") == "tool_use"]
            said = text_of(out.get("content"))
            print(f"  model      {model}: {said[:150]!r}" + (f" -> calls {calls}" if calls else ""))
        else:
            inp = json.dumps((s.get("input") or {}).get("input"))[:100]
            err = " (error)" if out.get("is_error") else ""
            print(f"  {kind:<10} {name} {inp} -> {text_of(out.get('content'))[:140]!r}{err}")
    answer = run.get("output")
    if not isinstance(answer, str):
        answer = json.dumps(answer)
    print(f"{run.get('status')} in {run.get('steps')} steps, ${run.get('cost', 0):.4f}, run {run_id}")
    print("ANSWER:", (answer or run.get("error") or "")[:800])


if __name__ == "__main__":
    if sys.argv[1] == "--show":
        show(sys.argv[2])
        sys.exit(0)
    agent, task = sys.argv[1], sys.argv[2]
    body = {"input": task}
    if agent != "-":
        body["agent"] = agent
    t0 = time.time()
    out = call("POST", "/v1/runs", body)
    print(f"took {time.time() - t0:.0f}s")
    if "run_id" not in (out or {}):
        print(out)
        sys.exit(1)
    show(out["run_id"])
