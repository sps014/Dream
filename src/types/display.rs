//! Human-readable rendering of interned types for diagnostics and the LSP. Generics render with
//! angle brackets (`Box<int>`), never the internal monomorphization spelling.

use super::{DefTable, TyKind, TypeId, TypeInterner};

/// The poison type's display spelling, matching the legacy `UNKNOWN_TYPE_NAME`.
pub const UNKNOWN_TYPE_NAME: &str = "<unknown>";

/// Renders `id` as source-level syntax (`int[]`, `Box<int>`, `int?`, `fun(int): bool`).
pub fn display_name(interner: &TypeInterner, defs: &DefTable, id: TypeId) -> String {
    match interner.kind(id) {
        TyKind::Prim(p) => p.name().to_string(),
        TyKind::Object => "object".to_string(),
        TyKind::Void => "void".to_string(),
        TyKind::Error => UNKNOWN_TYPE_NAME.to_string(),
        TyKind::Array(e) => format!("{}[]", display_name(interner, defs, *e)),
        TyKind::Nullable(inner) => format!("{}?", display_name(interner, defs, *inner)),
        TyKind::Enum(def) => defs.name(*def).to_string(),
        TyKind::Js => "js".to_string(),
        TyKind::Struct(def, args) | TyKind::Union(def, args) | TyKind::Interface(def, args) => {
            let base = defs.name(*def);
            if args.is_empty() {
                base.to_string()
            } else {
                let rendered = args
                    .iter()
                    .map(|a| display_name(interner, defs, *a))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", base, rendered)
            }
        }
        TyKind::Func(params, ret) => {
            let rendered = params
                .iter()
                .map(|p| display_name(interner, defs, *p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fun({}): {}", rendered, display_name(interner, defs, *ret))
        }
    }
}
