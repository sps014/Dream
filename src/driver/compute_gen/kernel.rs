//! Single-kernel emission: bindings header, body, entry point wrapper.

use super::context::EmitCtx;
use super::stmt::emit_stmts;
use super::ty::dream_ty_to_wgsl;
use super::types::{GpuBinding, GpuKernelInfo};
use dream_abi::attributes::{compute_workgroup_size, has_readonly_attr};
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::expression::ExpressionNode;
use dream_syntax::nodes::function::{FunctionNode, ParameterNode};
use dream_syntax::nodes::statement::StatementNode;
use dream_syntax::nodes::types::Type;
use indexmap::{IndexMap, IndexSet};
use std::cell::RefCell;

fn collect_workgroup_names(stmts: &[StatementNode<'_>], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            StatementNode::WorkgroupDecl(name, _, _) => out.push(name.text.clone()),
            StatementNode::IfElse(_, then_b, elifs, else_b) => {
                collect_workgroup_names(then_b, out);
                for (_, b) in elifs {
                    collect_workgroup_names(b, out);
                }
                if let Some(eb) = else_b {
                    collect_workgroup_names(eb, out);
                }
            }
            StatementNode::While(_, body) | StatementNode::DoWhile(body, _) => {
                collect_workgroup_names(body, out);
            }
            StatementNode::For(init, _, step, body) => {
                if let Some(i) = init {
                    collect_workgroup_names(std::slice::from_ref(i), out);
                }
                collect_workgroup_names(body, out);
                if let Some(st) = step {
                    collect_workgroup_names(std::slice::from_ref(st), out);
                }
            }
            StatementNode::Labeled(_, inner) => {
                collect_workgroup_names(std::slice::from_ref(inner), out);
            }
            StatementNode::Switch(_, cases, default) => {
                for (_, b) in cases {
                    collect_workgroup_names(b, out);
                }
                if let Some(db) = default {
                    collect_workgroup_names(db, out);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn emit_kernel(func: &FunctionNode<'_>, diagnostics: &mut DiagnosticBag) -> GpuKernelInfo {
    let name = func.name.text.clone();
    let entry = format!("dream_{}", name);
    let workgroup = compute_workgroup_size(&func.attributes);
    let atomic_bufs = collect_atomic_buffer_names(func.body);

    let mut bindings = Vec::new();
    let mut binding_idx = 0u32;
    let mut header = String::new();
    let mut uniform_fields = String::new();
    let mut has_uniform = false;

    for param in &func.parameters {
        let pname = param.name.text.clone();
        let is_atomic = atomic_bufs.contains(pname.as_str());
        match classify_param(param, is_atomic) {
            ParamClass::Storage {
                elem,
                read_write,
                atomic,
            } => {
                let access = if read_write {
                    "read_write"
                } else {
                    "read"
                };
                let elem_ty = if atomic {
                    format!("atomic<{elem}>")
                } else {
                    elem.clone()
                };
                let wgsl_name = format!("{entry}_{pname}");
                header.push_str(&format!(
                    "@group(0) @binding({binding_idx}) var<storage, {access}> {wgsl_name}: array<{elem_ty}>;\n"
                ));
                bindings.push(GpuBinding {
                    name: pname,
                    binding: binding_idx,
                    kind: "storage",
                    wgsl_ty: elem,
                    read_write,
                    atomic,
                });
                binding_idx += 1;
            }
            ParamClass::Texture { storage } => {
                let wgsl_name = format!("{entry}_{pname}");
                let (kind, decl) = if storage {
                    (
                        "storage_texture",
                        format!(
                            "@group(0) @binding({binding_idx}) var {wgsl_name}: texture_storage_2d<rgba8unorm, write>;\n"
                        ),
                    )
                } else {
                    (
                        "texture",
                        format!(
                            "@group(0) @binding({binding_idx}) var {wgsl_name}: texture_2d<f32>;\n"
                        ),
                    )
                };
                header.push_str(&decl);
                bindings.push(GpuBinding {
                    name: pname,
                    binding: binding_idx,
                    kind,
                    wgsl_ty: if storage {
                        "texture_storage_2d<rgba8unorm, write>".into()
                    } else {
                        "texture_2d<f32>".into()
                    },
                    read_write: storage,
                    atomic: false,
                });
                binding_idx += 1;
            }
            ParamClass::Sampler => {
                let wgsl_name = format!("{entry}_{pname}");
                header.push_str(&format!(
                    "@group(0) @binding({binding_idx}) var {wgsl_name}: sampler;\n"
                ));
                bindings.push(GpuBinding {
                    name: pname,
                    binding: binding_idx,
                    kind: "sampler",
                    wgsl_ty: "sampler".into(),
                    read_write: false,
                    atomic: false,
                });
                binding_idx += 1;
            }
            ParamClass::Uniform { ty } => {
                has_uniform = true;
                uniform_fields.push_str(&format!("  {pname}: {ty},\n"));
                bindings.push(GpuBinding {
                    name: pname,
                    binding: binding_idx, // assigned after we know the uniform struct binding
                    kind: "uniform",
                    wgsl_ty: ty,
                    read_write: false,
                    atomic: false,
                });
            }
        }
    }

    if has_uniform {
        // Uniforms share one binding struct at the end of the storage bindings.
        let u_bind = binding_idx;
        let u_struct = format!("DreamUniforms_{entry}");
        let u_var = format!("dream_uniforms_{entry}");
        header.push_str(&format!("struct {u_struct} {{\n"));
        header.push_str(&uniform_fields);
        header.push_str("}\n");
        header.push_str(&format!(
            "@group(0) @binding({u_bind}) var<uniform> {u_var}: {u_struct};\n"
        ));
        for b in bindings.iter_mut() {
            if b.kind == "uniform" {
                b.binding = u_bind;
            }
        }
        binding_idx += 1;
    }
    let _ = binding_idx;

    let mut workgroup_names = Vec::new();
    collect_workgroup_names(func.body, &mut workgroup_names);
    let ctx = EmitCtx {
        prefix: &entry,
        bindings: &bindings,
        workgroup_names: &workgroup_names,
        scopes: RefCell::new(vec![IndexMap::new()]),
    };

    let mut workgroup_decls = String::new();
    let mut body = String::new();
    emit_stmts(
        func.body,
        &mut body,
        &mut workgroup_decls,
        1,
        &ctx,
        diagnostics,
        &func.name.text,
    );

    let mut wgsl = String::new();
    wgsl.push_str(&header);
    wgsl.push('\n');
    wgsl.push_str(&workgroup_decls);
    if !workgroup_decls.is_empty() {
        wgsl.push('\n');
    }
    wgsl.push_str(&format!(
        "@compute @workgroup_size({}, {}, {})\nfn {entry}(\n",
        workgroup.0, workgroup.1, workgroup.2
    ));
    wgsl.push_str("  @builtin(global_invocation_id) global_id: vec3<u32>,\n");
    wgsl.push_str("  @builtin(local_invocation_id) local_id: vec3<u32>,\n");
    wgsl.push_str("  @builtin(workgroup_id) workgroup_id: vec3<u32>,\n");
    wgsl.push_str("  @builtin(num_workgroups) num_workgroups: vec3<u32>,\n");
    wgsl.push_str(") {\n");
    // Shadow builtins as i32 structs so Dream-style `.x` field access typechecks in source
    // (`GpuId3`) while WGSL uses `vec3<u32>` builtins.
    wgsl.push_str("  let global_id_i = vec3<i32>(i32(global_id.x), i32(global_id.y), i32(global_id.z));\n");
    wgsl.push_str("  let local_id_i = vec3<i32>(i32(local_id.x), i32(local_id.y), i32(local_id.z));\n");
    wgsl.push_str(
        "  let workgroup_id_i = vec3<i32>(i32(workgroup_id.x), i32(workgroup_id.y), i32(workgroup_id.z));\n",
    );
    wgsl.push_str(
        "  let num_workgroups_i = vec3<i32>(i32(num_workgroups.x), i32(num_workgroups.y), i32(num_workgroups.z));\n",
    );
    // Remap Dream identifiers to the i32 vectors.
    let body_mapped = body
        .replace("global_id.", "global_id_i.")
        .replace("local_id.", "local_id_i.")
        .replace("workgroup_id.", "workgroup_id_i.")
        .replace("num_workgroups.", "num_workgroups_i.");
    wgsl.push_str(&body_mapped);
    wgsl.push_str("}\n");

    GpuKernelInfo {
        name,
        entry,
        workgroup,
        bindings,
        wgsl,
    }
}

enum ParamClass {
    Storage {
        elem: String,
        read_write: bool,
        atomic: bool,
    },
    Texture {
        /// `true` → `texture_storage_2d` (write); `false` → sampled `texture_2d`.
        storage: bool,
    },
    Sampler,
    Uniform {
        ty: String,
    },
}

fn classify_param(param: &ParameterNode, is_atomic: bool) -> ParamClass {
    let readonly = has_readonly_attr(&param.attributes);
    match &param.type_ {
        // Bare `T[]` params are rejected in sema; only `GpuBuffer<T>` is storage.
        Type::Struct(tok, Some(args)) if tok.text == "GpuBuffer" && args.len() == 1 => {
            let elem = dream_ty_to_wgsl(&args[0]);
            let atomic = is_atomic && matches!(elem.as_str(), "i32" | "u32");
            ParamClass::Storage {
                elem,
                read_write: !readonly,
                atomic,
            }
        }
        Type::Struct(tok, None) if tok.text == "GpuTexture" => {
            // `@readonly GpuTexture` → sampled; otherwise storage texture (write).
            ParamClass::Texture {
                storage: !readonly,
            }
        }
        Type::Struct(tok, None) if tok.text == "GpuSampler" => ParamClass::Sampler,
        other => ParamClass::Uniform {
            ty: dream_ty_to_wgsl(other),
        },
    }
}

/// Buffer names passed to `Gpu.atomic_*` helpers — those storage bindings become `atomic<T>`.
fn collect_atomic_buffer_names(stmts: &[StatementNode<'_>]) -> IndexSet<String> {
    let mut out = IndexSet::new();
    walk_stmts_atomics(stmts, &mut out);
    out
}

fn walk_stmts_atomics(stmts: &[StatementNode<'_>], out: &mut IndexSet<String>) {
    for s in stmts {
        match s {
            StatementNode::ExpressionStatement(e)
            | StatementNode::Return(Some(e))
            | StatementNode::AwaitStmt(e) => walk_expr_atomics(e, out),
            StatementNode::Assignment(_, e)
            | StatementNode::Declaration(_, _, e, _)
            | StatementNode::MemberAssignment(_, _, e) => walk_expr_atomics(e, out),
            StatementNode::IndexAssignment(arr, idx, value) => {
                walk_expr_atomics(arr, out);
                walk_expr_atomics(idx, out);
                walk_expr_atomics(value, out);
            }
            StatementNode::FunctionInvocation(_, _, args) => {
                for a in args {
                    walk_expr_atomics(a, out);
                }
            }
            StatementNode::MethodInvocation(obj, method, _, args) => {
                if matches!(
                    method.text.as_str(),
                    "atomic_load" | "atomic_add" | "atomic_exchange" | "atomic_store"
                ) {
                    if let Some(ExpressionNode::Identifier(name)) = args.first() {
                        out.insert(name.text.clone());
                    }
                }
                walk_expr_atomics(obj, out);
                for a in args {
                    walk_expr_atomics(a, out);
                }
            }
            StatementNode::IfElse(cond, then_b, elifs, else_b) => {
                walk_expr_atomics(cond, out);
                walk_stmts_atomics(then_b, out);
                for (c, body) in elifs {
                    walk_expr_atomics(c, out);
                    walk_stmts_atomics(body, out);
                }
                if let Some(eb) = else_b {
                    walk_stmts_atomics(eb, out);
                }
            }
            StatementNode::While(cond, body) => {
                walk_expr_atomics(cond, out);
                walk_stmts_atomics(body, out);
            }
            StatementNode::DoWhile(body, cond) => {
                walk_stmts_atomics(body, out);
                walk_expr_atomics(cond, out);
            }
            StatementNode::For(init, cond, step, body) => {
                if let Some(i) = init {
                    walk_stmts_atomics(std::slice::from_ref(i), out);
                }
                if let Some(c) = cond {
                    walk_expr_atomics(c, out);
                }
                if let Some(s) = step {
                    walk_stmts_atomics(std::slice::from_ref(s), out);
                }
                walk_stmts_atomics(body, out);
            }
            StatementNode::Switch(subj, cases, default) => {
                walk_expr_atomics(subj, out);
                for (_, body) in cases {
                    walk_stmts_atomics(body, out);
                }
                if let Some(d) = default {
                    walk_stmts_atomics(d, out);
                }
            }
            StatementNode::Labeled(_, inner) => walk_stmts_atomics(std::slice::from_ref(inner), out),
            StatementNode::Lock(e, body) => {
                walk_expr_atomics(e, out);
                walk_stmts_atomics(body, out);
            }
            _ => {}
        }
    }
}

fn walk_expr_atomics(expr: &ExpressionNode<'_>, out: &mut IndexSet<String>) {
    match expr {
        ExpressionNode::MethodCall(obj, method, _, args)
            if matches!(
                method.text.as_str(),
                "atomic_load" | "atomic_add" | "atomic_exchange" | "atomic_store"
            ) =>
        {
            if let Some(ExpressionNode::Identifier(name)) = args.first() {
                out.insert(name.text.clone());
            }
            walk_expr_atomics(obj, out);
            for a in args {
                walk_expr_atomics(a, out);
            }
        }
        ExpressionNode::FunctionCall(name, _, args)
            if matches!(
                name.text.as_str(),
                "atomic_load" | "atomic_add" | "atomic_exchange" | "atomic_store"
            ) =>
        {
            if let Some(ExpressionNode::Identifier(n)) = args.first() {
                out.insert(n.text.clone());
            }
            for a in args {
                walk_expr_atomics(a, out);
            }
        }
        ExpressionNode::Binary(l, _, r) | ExpressionNode::IndexAccess(l, r) => {
            walk_expr_atomics(l, out);
            walk_expr_atomics(r, out);
        }
        ExpressionNode::MemberAccess(l, _) => walk_expr_atomics(l, out),
        ExpressionNode::Ternary(c, t, e) => {
            walk_expr_atomics(c, out);
            walk_expr_atomics(t, out);
            walk_expr_atomics(e, out);
        }
        ExpressionNode::Unary(_, e)
        | ExpressionNode::Parenthesized(_, e)
        | ExpressionNode::Cast(_, e)
        | ExpressionNode::NamedArg(_, e)
        | ExpressionNode::RefArgument(e)
        | ExpressionNode::Await(_, e)
        | ExpressionNode::Try(e)
        | ExpressionNode::IncDec { target: e, .. } => walk_expr_atomics(e, out),
        ExpressionNode::FunctionCall(_, _, args) => {
            for a in args {
                walk_expr_atomics(a, out);
            }
        }
        ExpressionNode::MethodCall(obj, _, _, args) | ExpressionNode::Call(obj, _, args) => {
            walk_expr_atomics(obj, out);
            for a in args {
                walk_expr_atomics(a, out);
            }
        }
        _ => {}
    }
}
