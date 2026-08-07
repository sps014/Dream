# HTTP

**Package:** `system.net` — `import system.net;` (typically also `import system;`)

`HttpClient` / `HttpResponse` wrap host `fetch` / native HTTP. Each call awaits the full response. Fallible ops return `Result<_, HttpError>`.

```dream
import system;
import system.net;
import system.json;
```

## Platform notes

| Runtime | HTTP backend |
| --- | --- |
| Native (`dream run`) | Native HTTP client |
| Node.js | Global `fetch` (Node 18+) |
| Browser | Page `fetch` |

## `HttpClient`

#### `HttpClient(base_url: string)`

Creates a client with an optional base URL. Pass `""` when every request uses a full URL; otherwise relative paths join onto the base.

```dream
let api = HttpClient("https://api.example.com");
```

#### `set_header(name, value): HttpClient`

Adds a default header and returns `this` for chaining. Use for auth tokens or `Accept` shared across many requests.

```dream
let api = HttpClient("https://api.example.com")
    .set_header("Authorization", "Bearer secret")
    .set_header("Accept", "application/json");
```

#### `with_timeout(ms: int): HttpClient` / `.timeout_ms`

Sets a per-request timeout in milliseconds (`0` = none). Read `.timeout_ms` to inspect the current limit.

```dream
api = api.with_timeout(5000);
System.println(api.timeout_ms);
```

#### `with_cookie_jar(jar: CookieJar)`

Attaches a cookie jar for automatic cookie storage and replay. Use for session-based APIs that set `Set-Cookie`.

```dream
let jar = CookieJar();
api = api.with_cookie_jar(jar);
```

#### `with_cancellation(token: CancellationToken)`

Links outbound requests to a cancellation token. Prefer when user abort or timeout should stop in-flight fetches cooperatively.

```dream
let src = CancellationSource();
api = api.with_cancellation(src.token());
```

#### `await text(path): Result<string, HttpError>`

Issues a GET and returns the response body as text. Shortcut when you only need the body string, not status or headers.

```dream
switch (await api.text("/users/42")) {
    Ok(body) => System.println(body),
    Err(e) => System.println(e.code()),
}
```

#### `await get(path): Result<HttpResponse, HttpError>`

Issues a GET and returns the full response object. Use when you need status codes, headers, or multiple body formats.

```dream
switch (await api.get("/data")) {
    Ok(res) => {
        if (res.ok()) {
            System.println(res.status());
            System.println(res.header("content-type"));
        }
    },
    Err(e) => System.println(e.message()),
}
```

#### `await post` / `put` / `patch` `(path, body): Result<HttpResponse, HttpError>`

Sends a request with a string body and returns the response. Pick the verb that matches the API semantics (create / replace / partial update).

```dream
switch (await api.post("/users", "{\"name\":\"Grace\"}")) {
    Ok(res) => System.println(res.status()),
    Err(e) => System.println(e.message()),
}
```

#### `await delete` / `head` `(path): Result<HttpResponse, HttpError>`

`delete` removes a resource; `head` fetches headers only (no body). Use `head` for cheap existence or metadata checks.

```dream
await api.delete("/users/1");
await api.head("/health");
```

#### `await request(method, path, body, headers): Result<HttpResponse, HttpError>`

Fully custom request with explicit method, body, and per-call headers. Use when the convenience verbs are not enough (e.g. uncommon methods).

```dream
let headers = HttpHeaders();
headers.set("Content-Type", "application/json");
await api.request("PUT", "/users/1", "{\"name\":\"Ada\"}", headers);
```

#### `await request_bytes` / `post_bytes` / `put_bytes`

Same as the string variants but with a `byte[]` body. Use for uploads/downloads of binary content (images, protobuf, etc.).

```dream
let http = HttpClient("");
switch (await http.get("https://example.com/logo.png")) {
    Ok(img) => { await File.write_bytes("logo.png", img.bytes()); },
    Err(e) => System.println(e.message()),
}
```

#### `await post_multipart(path, form): Result<HttpResponse, HttpError>`

POSTs a `MultipartForm` (fields and file parts). Use for browser-style file uploads and form submissions with attachments.

```dream
let form = MultipartForm();
form.add_field("name", "Ada");
form.add_file("avatar", "a.png", "image/png", Buffer.alloc<byte>(0));
await api.post_multipart("/upload", form);
```

## `HttpResponse`

#### `status(): int` / `ok(): bool`

Returns the numeric HTTP status and whether it is 2xx. Check `ok()` before assuming a successful payload.

```dream
System.println(res.status());  // 200
System.println(res.ok());      // true for 2xx
```

#### `header(name): string`

Returns a response header value (case-insensitive lookup). Empty string when absent — do not confuse with a legitimate empty header value.

```dream
System.println(res.header("content-type"));
```

#### `text(): string` / `bytes(): byte[]`

Materializes the body as UTF-8 text or raw bytes. Call only once — the body is buffered in memory after the await completes.

```dream
System.println(res.text());
let raw = res.bytes();
```

#### `json(): Result<JsonValue, ParseError>`

Parses the body as JSON into a `JsonValue`. Prefer `Json.deserialize<T>` when you have a `@json` type and know the schema.

```dream
switch (res.json()) {
    Ok(data) => System.println(data.length),
    Err(e) => System.println(e.message()),
}
```

## `HttpHeaders`

#### `HttpHeaders()` / `set` / `get` / `contains` / `remove` / `.length`

Mutable header bag with case-insensitive `contains` / `get`. Build per-request headers or inspect a collected set.

```dream
let h = HttpHeaders();
h.set("Accept", "application/json");
System.println(h.contains("accept"));  // true (case-insensitive)
System.println(h.get("Accept").unwrap_or(""));
h.remove("Accept");
System.println(h.length);
```

#### `to_wire(): string` / `HttpHeaders.from_wire(text): HttpHeaders`

Serializes headers to a JSON object string for the host bridge and parses them back. Used internally — rarely needed in application code.

```dream
let wire = h.to_wire();
let back = HttpHeaders.from_wire(wire);
```

Iterate: `for (let pair in h) { … }` yields `KeyValuePair<string, string>`.

## `CookieJar`

#### `CookieJar()` / `set` / `get` / `clear` / `.length` / `to_header` / `store_from_response`

In-memory cookie store with manual `set`/`get` and automatic capture from responses. `to_header` builds a `Cookie` request header; `store_from_response` ingests `Set-Cookie`.

```dream
let jar = CookieJar();
jar.set("sid", "abc");
System.println(jar.get("sid").unwrap_or(""));
System.println(jar.to_header());
jar.store_from_response(res);
jar.clear();
```

## `MultipartForm` / `MultipartBuilt`

#### `add_field` / `add_file` / `build(): MultipartBuilt`

Accumulates text fields and file parts, then `build()` produces wire-ready headers and body for `post_multipart`.

```dream
let form = MultipartForm();
form.add_field("title", "hi");
let built = form.build();  // MultipartBuilt(headers, body)
```

## `HttpError`

Implements [`Error`](option-result.md). Field `status` is `0` for transport errors.

```dream
let e = HttpError.transport("offline");
System.println(e.code());
let s = HttpError.status(404, "missing");
```

## `Url`

Fields: `scheme`, `host`, `port`, `path`, `query`, `fragment`.

#### `Url.parse(text): Result<Url, ParseError>`

Parses a URL string into structured parts. Use before modifying paths or joining relative links.

```dream
switch (Url.parse("https://example.com:443/a?x=1#frag")) {
    Ok(u) => {
        System.println(u.host);
        System.println(u.to_string());
    },
    Err(e) => System.println(e.message()),
}
```

#### `to_string(): string` / `with_path(path): Url` / `join(relative): Result<Url, ParseError>`

Re-serializes the URL, swaps the path, or resolves a relative reference against a base. `join` follows standard relative-URL rules.

```dream
switch (Url.parse("https://example.com/a")) {
    Ok(u) => {
        System.println(u.with_path("/b").to_string());
        switch (u.join("c")) {
            Ok(j) => System.println(j.to_string()),
            Err(e) => System.println(e.message()),
        }
    },
    Err(e) => System.println(e.message()),
}
```

A runnable example: [`sample/interop/http.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/http.dream).
