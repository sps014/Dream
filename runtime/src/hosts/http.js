/**
 * Performs one HTTP request via the platform `fetch` and serializes the whole response into a single
 * `Uint8Array` for `src/stdlib/net/http_response.dream`: an ASCII head (status line + header lines) and a blank
 * line, then the raw body bytes. Keeping the body raw (an `arrayBuffer`) makes binary responses
 * byte-exact. `body` is either a string or a `Uint8Array` (or "" / empty for none).
 */
async function httpDo(url, method, headersJson, body, timeoutMs) {
  const verb = (method || "GET").toUpperCase();
  const init = { method: verb };
  if (headersJson && headersJson !== "") {
    try { init.headers = JSON.parse(headersJson); } catch (_) { /* ignore bad header json */ }
  }
  const hasBody = typeof body === "string" ? body !== "" : body && body.length > 0;
  if (hasBody && verb !== "GET" && verb !== "HEAD") {
    init.body = body;
  }
  const ms = Number(timeoutMs) || 0;
  if (ms > 0) {
    if (typeof AbortSignal !== "undefined" && typeof AbortSignal.timeout === "function") {
      init.signal = AbortSignal.timeout(ms);
    } else {
      const ctrl = new AbortController();
      init.signal = ctrl.signal;
      setTimeout(() => ctrl.abort(), ms);
    }
  }
  try {
    const res = await fetch(url, init);

    let head = `${res.status}\n`;
    res.headers.forEach((value, name) => {
      head += `${name}: ${value}\n`;
    });
    head += "\n"; // blank line separating head from body
    const headBytes = new TextEncoder().encode(head);
    const bodyBytes = new Uint8Array(await res.arrayBuffer());

    const out = new Uint8Array(headBytes.length + bodyBytes.length);
    out.set(headBytes, 0);
    out.set(bodyBytes, headBytes.length);
    return out;
  } catch (e) {
    const msg = (e && (e.name === "TimeoutError" || e.name === "AbortError"))
      ? "timeout"
      : String((e && e.message) || e || "fetch failed");
    const headBytes = new TextEncoder().encode("0\n\n");
    const bodyBytes = new TextEncoder().encode(msg);
    const out = new Uint8Array(headBytes.length + bodyBytes.length);
    out.set(headBytes, 0);
    out.set(bodyBytes, headBytes.length);
    return out;
  }
}

export function makeHttpHost() {
  return {
    httpRequest: (url, method, headersJson, body, timeoutMs) =>
      httpDo(url, method, headersJson, body, timeoutMs),
    httpRequestBytes: (url, method, headersJson, body, timeoutMs) =>
      httpDo(url, method, headersJson, Uint8Array.from(body || []), timeoutMs),
  };
}
