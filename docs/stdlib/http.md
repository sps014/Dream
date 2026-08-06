# HTTP

**Package:** `system.net` — `import system.net;` (typically also `import system;` for console output)

`HttpClient` and `HttpResponse` are a small, instantiable HTTP client. Like [`File`](file.md), the capability is a pair of [`extern async fun`](../language/async.md) imports implemented once per host, so the same `.dream` runs unchanged everywhere. Each call performs the whole request and hands back the entire response — status, headers, and raw body — that you `await`.

Fallible ops return `Result<_, HttpError>` (`message()` / `code()` such as `HTTP_0`, `HTTP_404`). Typed headers use `HttpHeaders`. `Url.parse` returns `Result<Url, ParseError>`.

## Platform notes

| Runtime | HTTP backend |
| --- | --- |
| Native (`dream run`) | Native HTTP client |
| Node.js | Global `fetch` (Node 18+) |
| Browser | Page `fetch` |

The API is identical across all three; only the transport differs. Unlike the dynamic [`js`](../language/js-type.md) type, there is nothing to release — the body bytes are in hand once the future resolves.

## Creating a client

Construct with a base URL (`""` for none) and, optionally, default headers applied to every request. `set_header` returns the client, so calls chain:

```dream
import system;
import system.net;
import system.json;

let api = HttpClient("https://api.example.com")
    .set_header("Authorization", "Bearer secret")
    .set_header("Accept", "application/json");
```

## Fetching text

`text(path)` resolves to the body directly. Relative paths join onto the base URL:

```dream
import system;
import system.net;
import system.json;

async fun main(): void {
    let api = HttpClient("https://api.example.com");
    switch (await api.text("/users/42")) {
        Ok(body) => {
            switch (Json.parse(body)) {
                Ok(user) => System.println(user.get("name").unwrap_or(JsonValue.none()).as_string().unwrap_or("")),
                Err(e) => System.println(e.message()),
            }
        },
        Err(e) => System.println(e.code()),
    }
}
```

## Richer responses

`get(path)` resolves to an `HttpResponse` exposing status, headers, and body. Reads are synchronous — the bytes arrive with the response:

```dream
async fun main(): void {
    let api = HttpClient("https://api.example.com");
    switch (await api.get("/data")) {
        Ok(res) => {
            if (res.ok()) {                          // 2xx
                System.println(res.status());        // 200
                System.println(res.header("content-type"));
                switch (res.json()) {
                    Ok(data) => System.println(data.length),
                    Err(e) => System.println(e.message()),
                }
            }
        },
        Err(e) => System.println(e.message()),
    }
}
```

## HTTP methods

`get`/`delete`/`head` take a path; `post`/`put`/`patch` also take a body; `request` gives full control including per-call `HttpHeaders` (merged over the client's defaults):

```dream
async fun main(): void {
    let api = HttpClient("https://api.example.com");
    let headers = HttpHeaders();
    headers.set("Content-Type", "application/json");

    let created = await api.post("/users", "{\"name\":\"Grace\"}");
    switch (created) {
        Ok(res) => System.println(res.status()),
        Err(e) => System.println(e.message()),
    }

    let res = await api.request("PUT", "/users/1", "{\"name\":\"Ada\"}", headers);
}
```

## Binary bodies

For non-text data, the response body is byte-exact via `bytes()`, and `request_bytes`/`post_bytes`/`put_bytes` send a raw `byte[]` — both directions avoid any UTF-8 round-trip:

```dream
async fun main(): void {
    let http = HttpClient("");

    switch (await http.get("https://example.com/logo.png")) {
        Ok(img) => {
            await File.write_bytes("logo.png", img.bytes());
        },
        Err(e) => System.println(e.message()),
    }
}
```

## API reference

### HttpClient

| Member | Description |
| --- | --- |
| `HttpClient(base_url)` | construct; `base_url` is prepended to relative paths (`""` for none) |
| `with_timeout(ms): HttpClient` | request timeout in milliseconds (`0` = none) |
| `with_cookie_jar(jar): HttpClient` | attach a `CookieJar` for Cookie / Set-Cookie |
| `set_header(name, value): HttpClient` | add/overwrite a default header (chainable) |
| `text(path): Future<Result<string, HttpError>>` | GET and return the body as text |
| `get(path): Future<Result<HttpResponse, HttpError>>` | GET and return an `HttpResponse` |
| `post/put/patch(path, body): Future<Result<HttpResponse, HttpError>>` | send a text `body` with the given verb |
| `delete/head(path): Future<Result<HttpResponse, HttpError>>` | DELETE / HEAD request |
| `request(method, path, body, headers): Future<Result<HttpResponse, HttpError>>` | arbitrary verb; `headers` is `HttpHeaders` |
| `request_bytes(method, path, body, headers): Future<Result<HttpResponse, HttpError>>` | arbitrary verb with a binary `byte[]` body |
| `post_bytes/put_bytes(path, body): Future<Result<HttpResponse, HttpError>>` | POST / PUT a raw `byte[]` body |
| `post_multipart(path, form): Future<Result<HttpResponse, HttpError>>` | POST a `MultipartForm` |

### CookieJar / MultipartForm

`CookieJar.set`/`get`/`clear`/`to_header`/`store_from_response`. `MultipartForm.add_field`/`add_file`/`build()` → `MultipartBuilt` with headers + body.

### HttpHeaders

| Member | Description |
| --- | --- |
| `HttpHeaders()` | empty map |
| `set` / `get` / `contains` / `remove` | case-insensitive name match |
| `to_wire` / `from_wire` | JSON object string for the host bridge |

### HttpError

Implements [`Error`](option-result.md): `message()`, `code()`, plus `status` (0 for transport). Factories: `HttpError.transport(msg)`, `HttpError.status(code, msg)`.

### HttpResponse

A view over the raw response bytes. All reads are synchronous.

| Member | Description |
| --- | --- |
| `status(): int` | HTTP status code (`0` on a transport error) |
| `ok(): bool` | true for a 2xx status |
| `header(name): string` | value of response header `name` (case-insensitive), or "" |
| `text(): string` | body as UTF-8 text |
| `bytes(): byte[]` | body as raw bytes (binary-safe) |
| `json(): Result<JsonValue, ParseError>` | body parsed as [JSON](json.md) |

### Url

`Url.parse(text): Result<Url, ParseError>` splits scheme/host/port/path/query/fragment. `to_string()`, `with_path`, and `join` rebuild or resolve relative paths.

A runnable example lives in [`sample/interop/http.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/http.dream).
