"""Call a gai server's MCP endpoint the way an MCP client does: initialize,
list tools, and call one.

    GAI_URL=http://127.0.0.1:8485 GAI_KEY= python scripts/try-mcp.py agent_companion '{"task": "hi"}'
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
URL = os.environ.get("GAI_URL", "http://127.0.0.1:8484") + "/mcp"
session = None


def rpc(method, params=None, id_=None):
    global session
    body = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        body["params"] = params
    if id_ is not None:
        body["id"] = id_
    headers = {"content-type": "application/json", "accept": "application/json, text/event-stream"}
    if KEY:
        headers["authorization"] = f"Bearer {KEY}"
    if session:
        headers["mcp-session-id"] = session
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=1800) as r:
        session = r.headers.get("mcp-session-id") or session
        raw = r.read().decode()
    if id_ is None:
        return None
    # Replies come as JSON or as server-sent events.
    for line in raw.splitlines():
        if line.startswith("data:") and line[5:].strip().startswith("{"):
            msg = json.loads(line[5:])
            if msg.get("id") == id_:
                return msg
    return json.loads(raw) if raw.strip().startswith("{") else raw


init = rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "try-mcp", "version": "0"}}, 1)
print("server:", init["result"]["serverInfo"])
rpc("notifications/initialized")
tools = rpc("tools/list", {}, 2)["result"]["tools"]
print("tools:", [t["name"] for t in tools])
if len(sys.argv) > 2:
    t0 = time.time()
    res = rpc("tools/call", {"name": sys.argv[1], "arguments": json.loads(sys.argv[2])}, 3)
    r = res.get("result") or res
    print(f"took {time.time() - t0:.0f}s, error={r.get('isError')}")
    print("text:", " ".join(c.get("text", "") for c in r.get("content", [])))
    print("structured:", json.dumps(r.get("structuredContent")))
