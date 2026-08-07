//! Dream → WGSL type mapping and best-effort expression type inference.

use super::context::EmitCtx;
use dream_syntax::nodes::expression::ExpressionNode;
use dream_syntax::nodes::types::Type;
use dream_syntax::token::token_kind::TokenKind;

pub(super) fn dream_ty_to_wgsl(ty: &Type) -> String {
    match ty {
        Type::Float(_) | Type::Double(_) => "f32".into(),
        Type::Integer(_) | Type::Byte(_) => "i32".into(),
        Type::UInt(_) => "u32".into(),
        Type::Boolean(_) => "bool".into(),
        Type::Long(_) => "i32".into(),
        Type::ULong(_) => "u32".into(),
        Type::Struct(tok, _) => match tok.text.as_str() {
            "GpuId3" => "vec3<i32>".into(),
            other => other.to_string(),
        },
        Type::Array(inner) => format!("array<{}>", dream_ty_to_wgsl(inner)),
        _ => "i32".into(),
    }
}

pub(super) fn cast_wgsl_if_needed(rendered: String, got: &str, want: &str) -> String {
    if got == want || want.is_empty() {
        return rendered;
    }
    // Never invent scalar↔array/texture casts — those are emitter/user bugs, not coercions.
    if got.starts_with("array<")
        || want.starts_with("array<")
        || got.contains("texture")
        || want.contains("texture")
        || got == "sampler"
        || want == "sampler"
    {
        return rendered;
    }
    format!("{want}({rendered})")
}

pub(super) fn common_numeric_wgsl_ty(lt: &str, rt: &str) -> String {
    if lt.starts_with("array<") || rt.starts_with("array<") {
        return if lt.starts_with("array<") {
            lt
        } else {
            rt
        }
        .into();
    }
    if lt == "f32" || rt == "f32" {
        "f32".into()
    } else if lt == "u32" && rt == "u32" {
        "u32".into()
    } else if lt == "bool" || rt == "bool" {
        "bool".into()
    } else {
        "i32".into()
    }
}

pub(super) fn is_bool_producing_binop(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::EqualEqualToken
            | TokenKind::NotEqualToken
            | TokenKind::SmallerThanToken
            | TokenKind::SmallerThanEqualToken
            | TokenKind::GreaterThanToken
            | TokenKind::GreaterThanEqualToken
            | TokenKind::AmpersandAmpersandToken
            | TokenKind::PipePipeToken
    )
}

/// Best-effort WGSL type for unannotated `let` bindings (casts/literals/float ops).
pub(super) fn infer_wgsl_ty(expr: &ExpressionNode<'_>, ctx: &EmitCtx<'_>) -> String {
    match expr {
        ExpressionNode::Cast(ty, _) | ExpressionNode::Literal(ty) => dream_ty_to_wgsl(ty),
        ExpressionNode::Parenthesized(_, inner)
        | ExpressionNode::NamedArg(_, inner)
        | ExpressionNode::RefArgument(inner)
        | ExpressionNode::IncDec { target: inner, .. } => infer_wgsl_ty(inner, ctx),
        ExpressionNode::Unary(op, inner) => {
            if op.kind == TokenKind::BangToken {
                "bool".into()
            } else {
                infer_wgsl_ty(inner, ctx)
            }
        }
        ExpressionNode::Binary(l, op, r) => {
            if is_bool_producing_binop(op.kind) {
                return "bool".into();
            }
            let lt = infer_wgsl_ty(l, ctx);
            let rt = infer_wgsl_ty(r, ctx);
            common_numeric_wgsl_ty(&lt, &rt)
        }
        ExpressionNode::Ternary(_, t, e) => {
            let tt = infer_wgsl_ty(t, ctx);
            let et = infer_wgsl_ty(e, ctx);
            common_numeric_wgsl_ty(&tt, &et)
        }
        ExpressionNode::IndexAccess(arr, _) => {
            if let ExpressionNode::Identifier(name) = &**arr {
                if let Some(t) = ctx.lookup_local(&name.text) {
                    if let Some(inner) = t.strip_prefix("array<").and_then(|s| s.strip_suffix('>')) {
                        return inner.to_string();
                    }
                    return t;
                }
                if let Some(b) = ctx.binding(&name.text) {
                    if b.kind == "storage" {
                        return b.wgsl_ty.clone();
                    }
                }
            }
            "f32".into()
        }
        ExpressionNode::MemberAccess(_, _) => "i32".into(),
        ExpressionNode::Identifier(name) => {
            if let Some(t) = ctx.lookup_local(&name.text) {
                return t;
            }
            if let Some(b) = ctx.binding(&name.text) {
                match b.kind {
                    "uniform" => return b.wgsl_ty.clone(),
                    // Bare storage names are arrays — avoid treating them as scalars.
                    "storage" => return format!("array<{}>", b.wgsl_ty),
                    "texture" | "storage_texture" | "sampler" => return b.wgsl_ty.clone(),
                    _ => {}
                }
            }
            "i32".into()
        }
        ExpressionNode::FunctionCall(name, _, _) | ExpressionNode::MethodCall(_, name, _, _) => {
            match name.text.as_str() {
                "sqrt" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "atan2" | "floor"
                | "ceil" | "min" | "max" | "abs" | "clamp" => "f32".into(),
                "atomic_load" | "atomic_add" | "atomic_exchange" => "i32".into(),
                "texture_load" | "texture_sample_level" => "f32".into(),
                _ => "i32".into(),
            }
        }
        _ => "i32".into(),
    }
}
