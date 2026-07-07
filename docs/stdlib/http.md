# HTTP

`HttpClient` and `HttpResponse` are a small, instantiable HTTP client. Like [`File`](file.md), the capability is a pair of [`extern async fun`](../language/async.md) imports implemented once per host, so the same `.dream` runs unchanged everywhere. Each call performs the whole request and hands back the entire response — status, headers, and raw body — that you `await`.

## Runtime support

| Runtime | HTTP backend |
| --- | --- |
| Wasmtime (native CLI) | A blocking `reqwest` call |
| Node.js | The global `fetch` (Node 18+) |
| Browser | The page's `fetch` |

The API is identical across all three; only the transport differs. Unlike the dynamic [`js`](../language/references.md) type, there is nothing to release — the body bytes are in hand once the future resolves.

## Creating a client

Construct with a base URL (`""` for none) and, optionally, default headers applied to every request. `set_header` returns the client, so calls chain:

```dream
let api = HttpClient("https://api.example.com")
    .set_header("Authorization", "Bearer secret")
    .set_header("Accept", "application/json");
```

## Fetching text

`text(path)` resolves to the body directly. Relative paths join onto the base URL:

```dream
async fun main(): void {
    let api = HttpClient("https://api.example.com");
    let body = await api.text("/users/42");
    let user = JSON.parse(body);
    System.println(user.get("name").unwrap_or(JsonValue.none()).as_string());
}
```

## Richer responses

`get(path)` resolves to an `HttpResponse` exposing status, headers, and body. Reads are synchronous — the bytes arrive with the response:

```dream
async fun main(): void {
    let api = HttpClient("https://api.example.com");
    let res = await api.get("/data");
    if (res.ok()) {                          // 2xx
        System.println(res.status());        // 200
        System.println(res.header("content-type"));
        let data = res.json();               // JsonValue
    }
}
```

## HTTP methods

`get`/`delete`/`head` take a path; `post`/`put`/`patch` also take a body; `request` gives full control including per-call headers (a JSON-object string, merged over the client's defaults):

```dream
async fun main(): void {
    let api = HttpClient("https://api.example.com");

    let created = await api.post("/users", "{\"name\":\"Grace\"}");
    System.println(created.status());

    let res = await api.request("PUT", "/users/1",
                                "{\"name\":\"Ada\"}",
                                "{\"Content-Type\":\"application/json\"}");
    let updated = res.json();
}
```

## Binary bodies

For non-text data, the response body is byte-exact via `bytes()`, and `request_bytes`/`post_bytes`/`put_bytes` send a raw `byte[]` — both directions avoid any UTF-8 round-trip:

```dream
async fun main(): void {
    let http = HttpClient("");

    let img = await http.get("https://example.com/logo.png");
    await File.write_bytes("logo.png", img.bytes());

    let read = await File.read_bytes("logo.png");
    let payload = read.unwrap_or(Buffer.alloc<byte>(0));
    let res = await http.post_bytes("https://example.com/upload", payload);
    System.println(res.status());
}
```

## API reference

### HttpClient

| Member | Description |
| --- | --- |
| `HttpClient(base_url)` | construct; `base_url` is prepended to relative paths (`""` for none) |
| `set_header(name, value): HttpClient` | add/overwrite a default header (chainable) |
| `text(path): Future<string>` | GET and return the body as text |
| `get(path): Future<HttpResponse>` | GET and return an `HttpResponse` |
| `post/put/patch(path, body): Future<HttpResponse>` | send a text `body` with the given verb |
| `delete/head(path): Future<HttpResponse>` | DELETE / HEAD request |
| `request(method, path, body, headers): Future<HttpResponse>` | arbitrary verb; `headers` is a JSON-object string ("" for none) |
| `request_bytes(method, path, body, headers): Future<HttpResponse>` | arbitrary verb with a binary `byte[]` body |
| `post_bytes/put_bytes(path, body): Future<HttpResponse>` | POST / PUT a raw `byte[]` body |

### HttpResponse

A view over the raw response bytes. All reads are synchronous.

| Member | Description |
| --- | --- |
| `status(): int` | HTTP status code (`0` on a transport error) |
| `ok(): bool` | true for a 2xx status |
| `header(name): string` | value of response header `name` (case-insensitive), or "" |
| `text(): string` | body as UTF-8 text |
| `bytes(): byte[]` | body as raw bytes (binary-safe) |
| `json(): JsonValue` | body parsed as [JSON](json.md) |

A runnable example lives in [`sample/interop/http.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/http.dream).
