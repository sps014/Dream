//! `@operator("...")` / `@cast("implicit"|"explicit")` registration: recognizes the two
//! operator-overload attributes on a struct/class method (their generic shape — one string arg,
//! method-only placement — is already validated by [`crate::attributes`]), resolves the argument
//! to a concrete [`OperatorSymbol`]/[`CastKind`], and records the mangled method name so
//! `expressions::operators`/`expressions::dispatch`/`expressions::casts` can dispatch `a + b`,
//! `-a`, and `(T)a` to it. One [`OperatorOverloads`] table per registered type, keyed the same way
//! as `register_methods_for`'s `target_type_str` (so generic instantiations get independent tables).

use super::*;
use crate::syntax::nodes::FunctionNode;
use crate::syntax::token::token_kind::TokenKind;
use indexmap::IndexMap;

/// A user-overloadable operator symbol. `Sub`/`Neg` (and no other pair) share a surface spelling
/// (`"-"`); parameter arity at the `@operator` declaration site tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorSymbol {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Neg,
    Not,
    BitNot,
}

impl OperatorSymbol {
    /// Resolves an `@operator("...")` argument string given the tagged method's declared parameter
    /// count (1 = binary, 0 = unary), or `None` if the pairing is not a recognized operator.
    fn from_attr_str(symbol: &str, arity: usize) -> Option<Self> {
        use OperatorSymbol::*;
        Some(match (symbol, arity) {
            ("+", 1) => Add,
            ("-", 1) => Sub,
            ("-", 0) => Neg,
            ("*", 1) => Mul,
            ("/", 1) => Div,
            ("%", 1) => Mod,
            ("&", 1) => BitAnd,
            ("|", 1) => BitOr,
            ("^", 1) => BitXor,
            ("<<", 1) => Shl,
            (">>", 1) => Shr,
            ("==", 1) => Eq,
            ("!", 0) => Not,
            ("~", 0) => BitNot,
            _ => return None,
        })
    }

    /// True for the unary-only symbols (`Neg`/`Not`/`BitNot`) — the ones registered in the
    /// `unary` table rather than `binary`.
    fn is_unary(self) -> bool {
        matches!(
            self,
            OperatorSymbol::Neg | OperatorSymbol::Not | OperatorSymbol::BitNot
        )
    }

    /// The token that spells this operator at a *use* site (`a + b`), for the binary dispatch
    /// lookup in `expressions::operators`.
    pub fn from_binary_token(kind: TokenKind) -> Option<Self> {
        use OperatorSymbol::*;
        Some(match kind {
            TokenKind::PlusToken => Add,
            TokenKind::MinusToken => Sub,
            TokenKind::StarToken => Mul,
            TokenKind::SlashToken => Div,
            TokenKind::ModulusToken => Mod,
            TokenKind::BitWiseAmpersandToken => BitAnd,
            TokenKind::BitWisePipeToken => BitOr,
            TokenKind::BitWiseXorToken => BitXor,
            TokenKind::ShiftLeftToken => Shl,
            TokenKind::ShiftRightToken => Shr,
            TokenKind::EqualEqualToken => Eq,
            _ => return None,
        })
    }

    /// The token that spells this operator at a unary *use* site (`-a`), for the unary dispatch
    /// lookup in `expressions::dispatch`.
    pub fn from_unary_token(kind: TokenKind) -> Option<Self> {
        use OperatorSymbol::*;
        Some(match kind {
            TokenKind::MinusToken => Neg,
            TokenKind::BangToken => Not,
            TokenKind::TildeToken => BitNot,
            _ => return None,
        })
    }

    /// The surface spelling, for diagnostics (`"the '+' operator"`).
    fn symbol_str(self) -> &'static str {
        use OperatorSymbol::*;
        match self {
            Add => "+",
            Sub | Neg => "-",
            Mul => "*",
            Div => "/",
            Mod => "%",
            BitAnd => "&",
            BitOr => "|",
            BitXor => "^",
            Shl => "<<",
            Shr => ">>",
            Eq => "==",
            Not => "!",
            BitNot => "~",
        }
    }
}

/// A registered `@operator`-tagged method: its emitted (mangled) name, the parameter type it
/// expects (the right-hand operand for a binary operator; `None` for unary), and its declared
/// return type.
#[derive(Debug, Clone)]
pub struct OperatorMethod {
    pub mangled_name: String,
    pub param_type: Option<Type>,
    pub return_type: Type,
}

/// A registered `@cast`-tagged method: whether it is implicit or explicit, the target type it
/// converts to, and its emitted (mangled) name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    Implicit,
    Explicit,
}

impl CastKind {
    fn from_attr_str(s: &str) -> Option<Self> {
        match s {
            "implicit" => Some(CastKind::Implicit),
            "explicit" => Some(CastKind::Explicit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CastMethod {
    pub kind: CastKind,
    pub target: Type,
    pub mangled_name: String,
}

/// One type's full set of operator/cast overloads, keyed the same way as
/// `register_methods_for`'s `target_type_str` (so `Box_int`/`Box_string` get independent tables).
#[derive(Debug, Clone, Default)]
pub struct OperatorOverloads {
    pub binary: IndexMap<OperatorSymbol, OperatorMethod>,
    pub unary: IndexMap<OperatorSymbol, OperatorMethod>,
    pub casts: Vec<CastMethod>,
}

impl<'a> Analyzer<'a> {
    /// Recognizes `@operator("...")`/`@cast("...")` on `method` (already known to carry a valid
    /// single string argument and to be method-only-placed — see `attributes::validate_attributes`,
    /// run before analysis) and, if present, records it against `target_type_str`. Reports the
    /// operator-specific rules the generic attribute layer can't know: the argument must resolve to
    /// a known symbol/cast-kind for the method's declared arity, and no two methods on the same
    /// type may claim the same operator/cast.
    pub(in crate::semantics::analyzer) fn validate_and_register_operator(
        &mut self,
        target_type_str: &str,
        method: &FunctionNode<'a>,
        mangled_name: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        if let Some(attr) = method.attributes.iter().find(|a| a.name.text == "operator") {
            let Some(symbol_text) = attr.args.first().map(|a| a.text.trim_matches('"')) else {
                return;
            };
            let arity = method.parameters.len();
            let Some(symbol) = OperatorSymbol::from_attr_str(symbol_text, arity) else {
                diagnostics.report_error(
                    format!(
                        "'@operator(\"{}\")' is not a recognized operator for a method with {} parameter(s)",
                        symbol_text, arity
                    ),
                    Some(method.name.position),
                );
                return;
            };
            let return_type = method.return_type.clone().unwrap_or(Type::Void);
            let overloads = self
                .operator_overloads
                .entry(target_type_str.to_string())
                .or_default();
            let table = if symbol.is_unary() {
                &mut overloads.unary
            } else {
                &mut overloads.binary
            };
            if table.contains_key(&symbol) {
                diagnostics.report_error(
                    format!(
                        "'{}' already declares an operator overload for '{}' on '{}'",
                        target_type_str,
                        symbol.symbol_str(),
                        target_type_str
                    ),
                    Some(method.name.position),
                );
                return;
            }
            let param_type = if symbol.is_unary() {
                None
            } else {
                method.parameters.first().map(|p| p.type_.clone())
            };
            table.insert(
                symbol,
                OperatorMethod {
                    mangled_name: mangled_name.to_string(),
                    param_type,
                    return_type,
                },
            );
        }

        if let Some(attr) = method.attributes.iter().find(|a| a.name.text == "cast") {
            let Some(kind_text) = attr.args.first().map(|a| a.text.trim_matches('"')) else {
                return;
            };
            let Some(kind) = CastKind::from_attr_str(kind_text) else {
                diagnostics.report_error(
                    format!(
                        "'@cast(\"{}\")' must be \"implicit\" or \"explicit\"",
                        kind_text
                    ),
                    Some(method.name.position),
                );
                return;
            };
            if !method.parameters.is_empty() {
                diagnostics.report_error(
                    format!(
                        "'@cast' method '{}' must not declare parameters",
                        method.name.text
                    ),
                    Some(method.name.position),
                );
                return;
            }
            let Some(target) = method.return_type.clone() else {
                diagnostics.report_error(
                    format!(
                        "'@cast' method '{}' must declare a return type (the cast's target type)",
                        method.name.text
                    ),
                    Some(method.name.position),
                );
                return;
            };
            let target_str = target.get_type();
            let overloads = self
                .operator_overloads
                .entry(target_type_str.to_string())
                .or_default();
            if overloads
                .casts
                .iter()
                .any(|c| c.target.get_type() == target_str)
            {
                diagnostics.report_error(
                    format!(
                        "'{}' already declares a cast to '{}'",
                        target_type_str, target_str
                    ),
                    Some(method.name.position),
                );
                return;
            }
            overloads.casts.push(CastMethod {
                kind,
                target,
                mangled_name: mangled_name.to_string(),
            });
        }
    }

    /// The registered binary-operator method for `opr_kind` on `left`'s type, if `left` is a
    /// struct/class that declared one via `@operator(...)`. Returns an owned clone (the tables are
    /// small) so callers can freely make further `&mut self` calls (type-checking the other
    /// operand, emitting HIR) without fighting the borrow checker over a borrowed lookup result.
    pub(in crate::semantics::analyzer) fn operator_binary_fn(
        &self,
        left: &Type,
        opr_kind: TokenKind,
    ) -> Option<OperatorMethod> {
        let (base, args) = Self::resolve_struct_parts(left)?;
        let recv = crate::syntax::nodes::types::mangle_generic(&base, &args);
        let symbol = OperatorSymbol::from_binary_token(opr_kind)?;
        self.operator_overloads
            .get(&recv)?
            .binary
            .get(&symbol)
            .cloned()
    }

    /// The registered unary-operator method for `opr_kind` on `operand`'s type, if `operand` is a
    /// struct/class that declared one via `@operator(...)`. See [`Self::operator_binary_fn`] for
    /// why this returns an owned clone.
    pub(in crate::semantics::analyzer) fn operator_unary_fn(
        &self,
        operand: &Type,
        opr_kind: TokenKind,
    ) -> Option<OperatorMethod> {
        let (base, args) = Self::resolve_struct_parts(operand)?;
        let recv = crate::syntax::nodes::types::mangle_generic(&base, &args);
        let symbol = OperatorSymbol::from_unary_token(opr_kind)?;
        self.operator_overloads
            .get(&recv)?
            .unary
            .get(&symbol)
            .cloned()
    }

    /// The registered cast method converting `from` to `to`, if any. `only_implicit` restricts the
    /// search to `@cast("implicit")` methods (used for implicit-coercion sites); explicit `(T)expr`
    /// casts pass `false` to accept either kind, matching the common convention that an explicit
    /// cast may always invoke an implicit conversion. See [`Self::operator_binary_fn`] for why this
    /// returns an owned clone.
    pub(in crate::semantics::analyzer) fn operator_cast_fn(
        &self,
        from: &Type,
        to: &Type,
        only_implicit: bool,
    ) -> Option<CastMethod> {
        let (base, args) = Self::resolve_struct_parts(from)?;
        let recv = crate::syntax::nodes::types::mangle_generic(&base, &args);
        let target_str = to.get_type();
        self.operator_overloads
            .get(&recv)?
            .casts
            .iter()
            .find(|c| {
                c.target.get_type() == target_str
                    && (!only_implicit || c.kind == CastKind::Implicit)
            })
            .cloned()
    }
}
