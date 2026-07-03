//! HTTP host functions (the `Dream` module behind `src/stdlib/net/http_client.dream`). Each performs the whole
//! request synchronously (blocking `reqwest`) and bridges the serialized response into Dream's async
//! runtime, so the same `.dream` works under wasmtime, Node, and the browser.

use wasmtime::*;

use crate::mir::abi;
use crate::mir::async_emit::{F_SLOTS, HOST_POLL_INDEX, KIND_HOST};

use super::memory::{read_arg_bytes, read_arg_string, write_bytes_to_memory};

/// Calls an exported function on the caller's module by name with the given typed signature.
fn call_export_2(caller: &mut Caller<'_, ()>, name: &str, a: i32, b: i32) {
    let func = caller
        .get_export(name)
        .and_then(Extern::into_func)
        .unwrap_or_else(|| panic!("module must export `{}`", name))
        .typed::<(i32, i32), ()>(&*caller)
        .unwrap_or_else(|_| panic!("unexpected `{}` signature", name));
    func.call(&mut *caller, (a, b))
        .unwrap_or_else(|_| panic!("`{}` call failed", name));
}

/// Bridges a synchronous (blocking) host result into Dream's async runtime, mirroring
/// `wrapAsyncImport` in `runtime/dream.js`: allocate a host `Future` via the module's exported
/// `__dream_new_future`, write `bytes` as a `char[]`, resolve the future via `__dream_resolve`, and
/// return the future pointer. The future is already settled when the awaiting task inspects it, so
/// the scheduler simply re-polls the waiter.
fn resolve_host_future_bytes(caller: &mut Caller<'_, ()>, bytes: &[u8]) -> i32 {
    let new_future = caller
        .get_export(abi::EXPORT_NEW_FUTURE)
        .and_then(Extern::into_func)
        .expect("module must export `__dream_new_future`")
        .typed::<(i32, i32, i32), i32>(&*caller)
        .expect("unexpected `__dream_new_future` signature");
    let future = new_future
        .call(&mut *caller, (F_SLOTS, HOST_POLL_INDEX, KIND_HOST))
        .expect("`__dream_new_future` call failed");
    let data_ptr = write_bytes_to_memory(caller, bytes);
    call_export_2(caller, abi::EXPORT_RESOLVE, future, data_ptr);
    future
}

/// Performs one blocking HTTP request and serializes the whole response into the wire format shared
/// with `runtime/dream.js` (and parsed by `src/stdlib/net/http_response.dream`): an ASCII head ("<status>\n" plus
/// "Name: value\n" lines), a blank line, then the raw body bytes. `body` is sent verbatim unless the
/// verb is GET/HEAD or it is empty. Network/protocol errors come back as a status `0` response whose
/// body is the error text.
fn perform_http(method: &str, url: &str, headers_json: &str, body: Vec<u8>) -> Vec<u8> {
    let verb = method.to_uppercase();
    let http_method = reqwest::Method::from_bytes(verb.as_bytes()).unwrap_or(reqwest::Method::GET);

    let client = reqwest::blocking::Client::new();
    let mut builder = client.request(http_method, url);

    if !headers_json.is_empty() {
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(headers_json)
        {
            for (name, value) in map.iter() {
                if let Some(v) = value.as_str() {
                    builder = builder.header(name.as_str(), v);
                }
            }
        }
    }

    if !body.is_empty() && verb != "GET" && verb != "HEAD" {
        builder = builder.body(body);
    }

    match builder.send() {
        Ok(response) => {
            let status = response.status().as_u16();
            let mut head = format!("{}\n", status);
            for (name, value) in response.headers().iter() {
                if let Ok(v) = value.to_str() {
                    head.push_str(name.as_str());
                    head.push_str(": ");
                    head.push_str(v);
                    head.push('\n');
                }
            }
            head.push('\n'); // blank line separating head from body
            let mut out = head.into_bytes();
            if let Ok(body_bytes) = response.bytes() {
                out.extend_from_slice(&body_bytes);
            }
            out
        }
        Err(e) => {
            let mut out = b"0\n\n".to_vec(); // status 0 = transport error; body is the message
            out.extend_from_slice(e.to_string().as_bytes());
            out
        }
    }
}

/// Registers the HTTP host functions on `linker`. `httpRequest` takes a text body; `httpRequestBytes`
/// takes a binary `char[]` body.
pub fn link_http_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap(
        "Dream",
        "httpRequest",
        |mut caller: Caller<'_, ()>,
         url_ptr: i32,
         method_ptr: i32,
         headers_ptr: i32,
         body_ptr: i32|
         -> i32 {
            let url = read_arg_string(&mut caller, url_ptr);
            let method = read_arg_string(&mut caller, method_ptr);
            let headers = read_arg_string(&mut caller, headers_ptr);
            let body = read_arg_string(&mut caller, body_ptr).into_bytes();
            let response = perform_http(&method, &url, &headers, body);
            resolve_host_future_bytes(&mut caller, &response)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "httpRequestBytes",
        |mut caller: Caller<'_, ()>,
         url_ptr: i32,
         method_ptr: i32,
         headers_ptr: i32,
         body_ptr: i32|
         -> i32 {
            let url = read_arg_string(&mut caller, url_ptr);
            let method = read_arg_string(&mut caller, method_ptr);
            let headers = read_arg_string(&mut caller, headers_ptr);
            let body = read_arg_bytes(&mut caller, body_ptr);
            let response = perform_http(&method, &url, &headers, body);
            resolve_host_future_bytes(&mut caller, &response)
        },
    )?;

    Ok(())
}
