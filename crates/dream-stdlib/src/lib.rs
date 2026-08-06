use dream_syntax::nodes::Type;
use indexmap::IndexSet;

/// One embedded stdlib package: dotted import name, ordered source files, and package deps.
pub struct StdPackage {
    /// Dotted path users write in `import system.net;`.
    pub name: &'static str,
    /// `(virtual path, source)` pairs in merge order within this package.
    pub files: &'static [(&'static str, &'static str)],
    /// Other packages that must be loaded before this one.
    pub deps: &'static [&'static str],
}

/// Bootstrap packages always merged into every program (no user `import` required).
pub const BOOTSTRAP_PACKAGES: &[&str] = &["system.core", "system.primitives"];

/// All stdlib packages. Order is the global merge order when loading the full prelude.
pub const STD_PACKAGES: &[StdPackage] = &[
    StdPackage {
        name: "system.core",
        deps: &[],
        files: &[
            (
                "<std>/system/core/buffer.dream",
                include_str!("system/core/buffer.dream"),
            ),
            (
                "<std>/system/core/bytes.dream",
                include_str!("system/core/bytes.dream"),
            ),
            (
                "<std>/system/core/span.dream",
                include_str!("system/core/span.dream"),
            ),
            (
                "<std>/system/core/pointer.dream",
                include_str!("system/core/pointer.dream"),
            ),
            (
                "<std>/system/core/closure.dream",
                include_str!("system/core/closure.dream"),
            ),
            (
                "<std>/system/core/cell.dream",
                include_str!("system/core/cell.dream"),
            ),
            (
                "<std>/system/core/ref_box.dream",
                include_str!("system/core/ref_box.dream"),
            ),
            (
                "<std>/system/core/string_abi.dream",
                include_str!("system/core/string_abi.dream"),
            ),
            (
                "<std>/system/core/collection_tuning.dream",
                include_str!("system/core/collection_tuning.dream"),
            ),
            (
                "<std>/system/core/equatable.dream",
                include_str!("system/core/equatable.dream"),
            ),
            (
                "<std>/system/core/comparable.dream",
                include_str!("system/core/comparable.dream"),
            ),
            (
                "<std>/system/core/error.dream",
                include_str!("system/core/error.dream"),
            ),
            (
                "<std>/system/core/parse_error.dream",
                include_str!("system/core/parse_error.dream"),
            ),
            (
                "<std>/system/core/cancellation.dream",
                include_str!("system/core/cancellation.dream"),
            ),
            (
                "<std>/system/core/option.dream",
                include_str!("system/core/option.dream"),
            ),
            (
                "<std>/system/core/collection.dream",
                include_str!("system/core/collection.dream"),
            ),
            (
                "<std>/system/core/result.dream",
                include_str!("system/core/result.dream"),
            ),
            (
                "<std>/system/core/promise.dream",
                include_str!("system/core/promise.dream"),
            ),
            (
                "<std>/system/core/webworker.dream",
                include_str!("system/core/webworker.dream"),
            ),
            (
                "<std>/system/core/webworker_pool.dream",
                include_str!("system/core/webworker_pool.dream"),
            ),
            (
                "<std>/system/core/lock.dream",
                include_str!("system/core/lock.dream"),
            ),
            (
                "<std>/system/core/semaphore.dream",
                include_str!("system/core/semaphore.dream"),
            ),
            (
                "<std>/system/core/js.dream",
                include_str!("system/core/js.dream"),
            ),
            (
                "<std>/system/core/math.dream",
                include_str!("system/core/math.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.primitives",
        deps: &["system.core"],
        files: &[
            (
                "<std>/system/primitives/int.dream",
                include_str!("system/primitives/int.dream"),
            ),
            (
                "<std>/system/primitives/long.dream",
                include_str!("system/primitives/long.dream"),
            ),
            (
                "<std>/system/primitives/uint.dream",
                include_str!("system/primitives/uint.dream"),
            ),
            (
                "<std>/system/primitives/ulong.dream",
                include_str!("system/primitives/ulong.dream"),
            ),
            (
                "<std>/system/primitives/byte.dream",
                include_str!("system/primitives/byte.dream"),
            ),
            (
                "<std>/system/primitives/char.dream",
                include_str!("system/primitives/char.dream"),
            ),
            (
                "<std>/system/primitives/bool.dream",
                include_str!("system/primitives/bool.dream"),
            ),
            (
                "<std>/system/primitives/float.dream",
                include_str!("system/primitives/float.dream"),
            ),
            (
                "<std>/system/primitives/double.dream",
                include_str!("system/primitives/double.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.collections",
        deps: &["system.core", "system.primitives"],
        files: &[
            (
                "<std>/system/collections/list.dream",
                include_str!("system/collections/list.dream"),
            ),
            (
                "<std>/system/collections/list_iterator.dream",
                include_str!("system/collections/list_iterator.dream"),
            ),
            (
                "<std>/system/collections/map.dream",
                include_str!("system/collections/map.dream"),
            ),
            (
                "<std>/system/collections/key_value_pair.dream",
                include_str!("system/collections/key_value_pair.dream"),
            ),
            (
                "<std>/system/collections/map_iterator.dream",
                include_str!("system/collections/map_iterator.dream"),
            ),
            (
                "<std>/system/collections/set.dream",
                include_str!("system/collections/set.dream"),
            ),
            (
                "<std>/system/collections/set_iterator.dream",
                include_str!("system/collections/set_iterator.dream"),
            ),
            (
                "<std>/system/collections/queue.dream",
                include_str!("system/collections/queue.dream"),
            ),
            (
                "<std>/system/collections/stack.dream",
                include_str!("system/collections/stack.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.text",
        deps: &["system.core", "system.primitives", "system.collections"],
        files: &[
            (
                "<std>/system/text/string.dream",
                include_str!("system/text/string.dream"),
            ),
            (
                "<std>/system/text/unicode.dream",
                include_str!("system/text/unicode.dream"),
            ),
            (
                "<std>/system/text/string_iterator.dream",
                include_str!("system/text/string_iterator.dream"),
            ),
            (
                "<std>/system/text/string_builder.dream",
                include_str!("system/text/string_builder.dream"),
            ),
            (
                "<std>/system/text/regex_node_kind.dream",
                include_str!("system/text/regex_node_kind.dream"),
            ),
            (
                "<std>/system/text/regex_empty_kind.dream",
                include_str!("system/text/regex_empty_kind.dream"),
            ),
            (
                "<std>/system/text/regex_node.dream",
                include_str!("system/text/regex_node.dream"),
            ),
            (
                "<std>/system/text/regex_op.dream",
                include_str!("system/text/regex_op.dream"),
            ),
            (
                "<std>/system/text/regex_inst.dream",
                include_str!("system/text/regex_inst.dream"),
            ),
            (
                "<std>/system/text/regex_compiler.dream",
                include_str!("system/text/regex_compiler.dream"),
            ),
            (
                "<std>/system/text/regex_parser.dream",
                include_str!("system/text/regex_parser.dream"),
            ),
            (
                "<std>/system/text/regex_thread.dream",
                include_str!("system/text/regex_thread.dream"),
            ),
            (
                "<std>/system/text/regex_add_frame.dream",
                include_str!("system/text/regex_add_frame.dream"),
            ),
            (
                "<std>/system/text/regex_match.dream",
                include_str!("system/text/regex_match.dream"),
            ),
            (
                "<std>/system/text/regex_vm.dream",
                include_str!("system/text/regex_vm.dream"),
            ),
            (
                "<std>/system/text/regex.dream",
                include_str!("system/text/regex.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.json",
        deps: &[
            "system.core",
            "system.primitives",
            "system.collections",
            "system.text",
            "system.codegen",
        ],
        files: &[
            (
                "<std>/system/json/json_value.dream",
                include_str!("system/json/json_value.dream"),
            ),
            (
                "<std>/system/json/json_parser.dream",
                include_str!("system/json/json_parser.dream"),
            ),
            (
                "<std>/system/json/json.dream",
                include_str!("system/json/json.dream"),
            ),
            (
                "<std>/system/json/json_gen_model.dream",
                include_str!("system/json/json_gen_model.dream"),
            ),
            (
                "<std>/system/json/json_generator.dream",
                include_str!("system/json/json_generator.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.io",
        deps: &[
            "system.core",
            "system.primitives",
            "system.collections",
            "system.text",
        ],
        files: &[
            (
                "<std>/system/io/io_error.dream",
                include_str!("system/io/io_error.dream"),
            ),
            (
                "<std>/system/io/path.dream",
                include_str!("system/io/path.dream"),
            ),
            (
                "<std>/system/io/file_handle.dream",
                include_str!("system/io/file_handle.dream"),
            ),
            (
                "<std>/system/io/file_stream.dream",
                include_str!("system/io/file_stream.dream"),
            ),
            (
                "<std>/system/io/file.dream",
                include_str!("system/io/file.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.net",
        deps: &[
            "system.core",
            "system.primitives",
            "system.collections",
            "system.text",
            "system.json",
        ],
        files: &[
            (
                "<std>/system/net/http_error.dream",
                include_str!("system/net/http_error.dream"),
            ),
            (
                "<std>/system/net/http_headers.dream",
                include_str!("system/net/http_headers.dream"),
            ),
            (
                "<std>/system/net/url.dream",
                include_str!("system/net/url.dream"),
            ),
            (
                "<std>/system/net/http_response.dream",
                include_str!("system/net/http_response.dream"),
            ),
            (
                "<std>/system/net/cookie_jar.dream",
                include_str!("system/net/cookie_jar.dream"),
            ),
            (
                "<std>/system/net/multipart.dream",
                include_str!("system/net/multipart.dream"),
            ),
            (
                "<std>/system/net/http_client.dream",
                include_str!("system/net/http_client.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.encoding",
        deps: &["system.core", "system.primitives", "system.text"],
        files: &[
            (
                "<std>/system/encoding.dream",
                include_str!("system/encoding.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.crypto",
        deps: &["system.core", "system.primitives"],
        files: &[
            (
                "<std>/system/crypto/crypto.dream",
                include_str!("system/crypto/crypto.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system",
        deps: &["system.core", "system.primitives", "system.text", "system.io"],
        files: &[
            (
                "<std>/system/arg_error.dream",
                include_str!("system/arg_error.dream"),
            ),
            (
                "<std>/system/system.dream",
                include_str!("system/system.dream"),
            ),
            (
                "<std>/system/random.dream",
                include_str!("system/random.dream"),
            ),
            (
                "<std>/system/console_color.dream",
                include_str!("system/console_color.dream"),
            ),
            (
                "<std>/system/time.dream",
                include_str!("system/time.dream"),
            ),
            (
                "<std>/system/stopwatch.dream",
                include_str!("system/stopwatch.dream"),
            ),
            (
                "<std>/system/datetime_ymd.dream",
                include_str!("system/datetime_ymd.dream"),
            ),
            (
                "<std>/system/datetime.dream",
                include_str!("system/datetime.dream"),
            ),
            (
                "<std>/system/debug.dream",
                include_str!("system/debug.dream"),
            ),
        ],
    },
    StdPackage {
        name: "system.codegen",
        deps: &["system.core", "system.primitives", "system.text"],
        files: &[(
            "<std>/system/codegen/codegen.dream",
            include_str!("system/codegen/codegen.dream"),
        )],
    },
    StdPackage {
        name: "system.logging",
        deps: &[
            "system.core",
            "system.primitives",
            "system.collections",
            "system",
            "system.io",
        ],
        files: &[
            (
                "<std>/system/logging/logging.dream",
                include_str!("system/logging/logging.dream"),
            ),
        ],
    },
];

/// Returns every `(virtual_path, source)` across all packages in deterministic registry order.
pub fn all_prelude_files() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for pkg in STD_PACKAGES {
        out.extend_from_slice(pkg.files);
    }
    out
}

/// Looks up a package by dotted name (`system.net`).
pub fn find_package(name: &str) -> Option<&'static StdPackage> {
    STD_PACKAGES.iter().find(|p| p.name == name)
}

/// True when `slash_path` (parser form of a plain import, e.g. `system/net`) names a std package.
pub fn std_package_from_slash_path(slash_path: &str) -> Option<&'static StdPackage> {
    let dotted = slash_path.replace('/', ".");
    find_package(&dotted)
}

/// Expands `requested` package names with bootstrap + transitive deps, in registry merge order.
pub fn resolve_packages_to_load(requested: &IndexSet<String>) -> Vec<&'static StdPackage> {
    let mut needed: IndexSet<&'static str> = IndexSet::new();
    for &boot in BOOTSTRAP_PACKAGES {
        needed.insert(boot);
    }
    for name in requested {
        collect_deps(name, &mut needed);
    }
    STD_PACKAGES
        .iter()
        .filter(|p| needed.contains(p.name))
        .collect()
}

fn collect_deps(name: &str, needed: &mut IndexSet<&'static str>) {
    let Some(pkg) = find_package(name) else {
        return;
    };
    if !needed.insert(pkg.name) {
        return;
    }
    for &dep in pkg.deps {
        collect_deps(dep, needed);
    }
}

/// Maps a public top-level stdlib symbol name to the package that exports it (for LSP auto-import).
/// Built by scanning package sources for `public class|enum|interface|fun|extend` at file top-level.
pub fn symbol_to_package() -> std::collections::HashMap<String, &'static str> {
    let mut map = std::collections::HashMap::new();
    for pkg in STD_PACKAGES {
        // Bootstrap packages need no user import — skip so auto-import won't suggest them.
        if BOOTSTRAP_PACKAGES.contains(&pkg.name) {
            continue;
        }
        for &(_, src) in pkg.files {
            for name in public_top_level_names(src) {
                map.entry(name).or_insert(pkg.name);
            }
        }
    }
    map
}

fn public_top_level_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with("module ") || t.starts_with("import ") {
            continue;
        }
        // Nested members are indented; only look at column-0 public decls (approx).
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let rest = if let Some(r) = t.strip_prefix("public ") {
            r.trim_start()
        } else {
            continue;
        };
        let rest = rest
            .strip_prefix("sealed ")
            .unwrap_or(rest)
            .strip_prefix("static ")
            .unwrap_or(rest)
            .strip_prefix("async ")
            .unwrap_or(rest)
            .trim_start();
        for kind in ["class ", "enum ", "interface ", "fun ", "extend ", "struct ", "union "] {
            if let Some(after) = rest.strip_prefix(kind) {
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    names.push(name);
                }
                break;
            }
        }
    }
    names
}

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

    /// User-callable stdlib *free* functions.
    pub fn get_all() -> Vec<StdlibFunction> {
        vec![]
    }
}
