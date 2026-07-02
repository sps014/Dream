use crate::syntax::nodes::Type;

/// The embedded standard-library prelude, in the exact order it must be merged. This is the
/// single source of truth shared by the compiler's source manager and the `dream-analyzer`
/// language service, so the two can never drift. The primitive files (int/char/string/...)
/// only attach methods to built-in types, so their relative order does not matter.
pub const PRELUDE_FILES: &[(&str, &str)] = &[
    // Core intrinsic-backed types: raw arrays, `Option`/`Result`, futures, JS interop, math.
    ("<std>/core/array.dream", include_str!("core/array.dream")),
    // Comparison interfaces (`Equatable`/`Comparable`) come before types that implement them.
    ("<std>/core/compare.dream", include_str!("core/compare.dream")),
    ("<std>/core/option.dream", include_str!("core/option.dream")),
    ("<std>/core/result.dream", include_str!("core/result.dream")),
    ("<std>/core/promise.dream", include_str!("core/promise.dream")),
    ("<std>/core/jsref.dream", include_str!("core/jsref.dream")),
    ("<std>/core/math.dream", include_str!("core/math.dream")),
    // Collections (`List`/`Map` and their cursors), one class per file under `collections/`.
    ("<std>/collections/list.dream", include_str!("collections/list.dream")),
    (
        "<std>/collections/list_iterator.dream",
        include_str!("collections/list_iterator.dream"),
    ),
    ("<std>/collections/map.dream", include_str!("collections/map.dream")),
    (
        "<std>/collections/key_value_pair.dream",
        include_str!("collections/key_value_pair.dream"),
    ),
    (
        "<std>/collections/map_iterator.dream",
        include_str!("collections/map_iterator.dream"),
    ),
    // Scalar primitives: each attaches methods to a built-in numeric/bool/char type.
    ("<std>/primitives/int.dream", include_str!("primitives/int.dream")),
    ("<std>/primitives/long.dream", include_str!("primitives/long.dream")),
    ("<std>/primitives/uint.dream", include_str!("primitives/uint.dream")),
    ("<std>/primitives/ulong.dream", include_str!("primitives/ulong.dream")),
    ("<std>/primitives/byte.dream", include_str!("primitives/byte.dream")),
    ("<std>/primitives/char.dream", include_str!("primitives/char.dream")),
    ("<std>/primitives/bool.dream", include_str!("primitives/bool.dream")),
    ("<std>/primitives/float.dream", include_str!("primitives/float.dream")),
    ("<std>/primitives/double.dream", include_str!("primitives/double.dream")),
    // Text: the `string` type, its character cursor, and regular expressions.
    ("<std>/text/string.dream", include_str!("text/string.dream")),
    (
        "<std>/text/string_iterator.dream",
        include_str!("text/string_iterator.dream"),
    ),
    ("<std>/text/regex.dream", include_str!("text/regex.dream")),
    // JSON: value tree, parser, and the public `JSON` API (one class per file).
    ("<std>/json/json_value.dream", include_str!("json/json_value.dream")),
    ("<std>/json/json_parser.dream", include_str!("json/json_parser.dream")),
    ("<std>/json/json.dream", include_str!("json/json.dream")),
    // Networking: HTTP client and its response type (one class per file).
    ("<std>/net/http_response.dream", include_str!("net/http_response.dream")),
    ("<std>/net/http_client.dream", include_str!("net/http_client.dream")),
    // Filesystem I/O: static `File` API and the buffered `FileStream` (one class per file).
    ("<std>/io/file.dream", include_str!("io/file.dream")),
    ("<std>/io/file_stream.dream", include_str!("io/file_stream.dream")),
    // System services: console output/input, the `ConsoleColor` enum, timing, and debug helpers.
    ("<std>/system/system.dream", include_str!("system/system.dream")),
    ("<std>/system/console_color.dream", include_str!("system/console_color.dream")),
    ("<std>/system/time.dream", include_str!("system/time.dream")),
    ("<std>/system/datetime.dream", include_str!("system/datetime.dream")),
    ("<std>/system/debug.dream", include_str!("system/debug.dream")),
];

pub struct StdlibFunction {
    pub name: String,
    pub parameters: Vec<String>,
    pub return_type: Option<Type>,
    /// When `true`, codegen emits this function's body inline (see `RUNTIME_STRINGS` / the object
    /// runtime) instead of importing it from the host. This is the single source of truth for the
    /// import-vs-inline decision; the module import emitter consults it rather than a parallel list.
    pub inline: bool,
}

impl StdlibFunction {
    /// A host-imported stdlib function (lowered to a WASM `(import "env" ...)`).
    fn imported(name: &str, parameters: &[&str], return_type: Option<Type>) -> Self {
        Self {
            name: name.to_string(),
            parameters: parameters.iter().map(|s| s.to_string()).collect(),
            return_type,
            inline: false,
        }
    }

    /// Host functions that are always imported into every module but are NOT user-callable.
    /// The `print`/`println` builtins lower to these; users never name them directly.
    pub fn host_imports() -> Vec<StdlibFunction> {
        let imports = vec![
            Self::imported("print_string", &["string"], None),
            Self::imported("print_int", &["int"], None),
            Self::imported("print_float", &["float"], None),
            Self::imported("print_double", &["double"], None),
            Self::imported("print_char", &["char"], None),
        ];
        imports
    }

    /// User-callable stdlib *free* functions registered in the function table. There are none: the
    /// former string/array/debug primitives are now class members (`String.alloc`/`String.set`,
    /// `Array.new`, `Debug.free_list_head`) or builtin pseudo-methods (`s.char_at(i)`), lowered by
    /// the compiler to their `RUNTIME_STRINGS` helpers (`$string_alloc`/`$char_at`/...). The runtime
    /// bodies themselves are still emitted unconditionally from `RUNTIME_STRINGS`.
    pub fn get_all() -> Vec<StdlibFunction> {
        vec![]
    }
}
