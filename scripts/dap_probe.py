#!/usr/bin/env python3
"""Ad-hoc DAP client to smoke-test the Dream debugger (async + workers)."""
import json, subprocess, sys, threading, queue, time

BIN = sys.argv[1]
SRC = sys.argv[2]
BP_LINES = [int(x) for x in sys.argv[3].split(",")]
STOP_TIMEOUT = float(sys.argv[4]) if len(sys.argv) > 4 else 6.0

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

def wait_response(s, timeout=8):
    t = time.time()
    while time.time() - t < timeout:
        if s in responses:
            return responses[s]
        time.sleep(0.005)
    raise TimeoutError(f"no response for seq {s}")

def wait_event(name, timeout=8):
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
                             "breakpoints": [{"line": l} for l in BP_LINES]}); print("bp:", wait_response(s)["body"])
s = send("configurationDone"); wait_response(s)

for _ in range(len(BP_LINES) + 2):
    try:
        st = wait_event("stopped", timeout=STOP_TIMEOUT)
    except TimeoutError:
        print("no more stops")
        break
    tid = st["body"].get("threadId", 1)
    print(f"\n== stopped thread {tid} reason={st['body'].get('reason')} ==")
    s = send("threads"); ths = wait_response(s)["body"]["threads"]; print("threads:", ths)
    s = send("stackTrace", {"threadId": tid}); frames = wait_response(s)["body"]["stackFrames"]
    print("stack:", [(f["name"], f["line"]) for f in frames])
    if frames:
        s = send("scopes", {"frameId": frames[0]["id"]}); ref = wait_response(s)["body"]["scopes"][0]["variablesReference"]
        s = send("variables", {"variablesReference": ref}); vars = wait_response(s)["body"]["variables"]
        for v in vars:
            print(f"   {v['name']}: {v['value']}  [{v['type']}] ref={v['variablesReference']}")
    send("continue", {"threadId": tid})

time.sleep(0.5)
send("disconnect")
time.sleep(0.3)
proc.terminate()
err = proc.stderr.read(600)
if err:
    print("STDERR:", err.decode(errors="replace")[:600])
