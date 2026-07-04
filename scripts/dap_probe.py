#!/usr/bin/env python3
"""Ad-hoc DAP client to smoke-test live variable inspection of the Dream debugger."""
import json, subprocess, sys, threading, queue

BIN = sys.argv[1]
SRC = sys.argv[2]
BP_LINE = int(sys.argv[3])

proc = subprocess.Popen([BIN, "debug-adapter", SRC],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

events = queue.Queue()
responses = {}
seq = [0]

def reader():
    buf = b""
    while True:
        chunk = proc.stdout.read(1)
        if not chunk:
            break
        buf += chunk
        if b"\r\n\r\n" in buf:
            header, rest = buf.split(b"\r\n\r\n", 1)
            length = int(dict(l.split(": ") for l in header.decode().split("\r\n"))["Content-Length"])
            while len(rest) < length:
                rest += proc.stdout.read(length - len(rest))
            msg = json.loads(rest[:length].decode())
            buf = rest[length:]
            if msg.get("type") == "event":
                events.put(msg)
            elif msg.get("type") == "response":
                responses[msg["request_seq"]] = msg

threading.Thread(target=reader, daemon=True).start()

def send(cmd, args=None):
    seq[0] += 1
    s = seq[0]
    body = {"seq": s, "type": "request", "command": cmd}
    if args is not None:
        body["arguments"] = args
    data = json.dumps(body).encode()
    proc.stdin.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
    proc.stdin.flush()
    return s

def wait_response(s, timeout=5):
    import time
    t = time.time()
    while time.time() - t < timeout:
        if s in responses:
            return responses[s]
    raise TimeoutError(f"no response for seq {s}")

def wait_event(name, timeout=5):
    import time
    t = time.time()
    while time.time() - t < timeout:
        try:
            e = events.get(timeout=0.1)
        except queue.Empty:
            continue
        if e.get("event") == name:
            return e
    raise TimeoutError(f"no event {name}")

s = send("initialize", {"adapterID": "dream"}); wait_response(s)
wait_event("initialized")
s = send("launch", {"program": SRC}); wait_response(s)
s = send("setBreakpoints", {"source": {"path": SRC},
                             "breakpoints": [{"line": BP_LINE}]}); print("bp:", wait_response(s)["body"])
s = send("configurationDone"); wait_response(s)

wait_event("stopped")
s = send("stackTrace", {"threadId": 1}); frames = wait_response(s)["body"]["stackFrames"]
print("stack:", [(f["name"], f["line"]) for f in frames])
s = send("scopes", {"frameId": 0}); ref = wait_response(s)["body"]["scopes"][0]["variablesReference"]
s = send("variables", {"variablesReference": ref}); vars = wait_response(s)["body"]["variables"]
for v in vars:
    print(f"  {v['name']}: {v['value']}   [{v['type']}] ref={v['variablesReference']}")
    if v["variablesReference"]:
        s = send("variables", {"variablesReference": v["variablesReference"]})
        for c in wait_response(s)["body"]["variables"]:
            print(f"      .{c['name']}: {c['value']}   [{c['type']}] ref={c['variablesReference']}")

send("continue")
proc.terminate()
