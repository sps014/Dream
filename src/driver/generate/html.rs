//! Host expander for the sample `html { ... }` syntax DSL (`sample/generators/html/`).
//!
//! Not a language builtin and not part of `system.codegen`. Runs only when a generator registers
//! `@syntax_block("html")`. Emits nested `Html.el` / string Dream source that calls the sample's
//! runtime `Html` helpers.

use super::context::GeneratorContext;

/// Expands every `html { ... }` site when that introducer is registered.
pub fn expand_if_registered(ctx: &mut GeneratorContext) {
    if !ctx.syntax_block_names.iter().any(|n| n == "html") {
        return;
    }
    let blocks = ctx.syntax_blocks("html");
    for id in blocks {
        let Some(site) = ctx.syntax.block_keys.get(&id).cloned() else {
            continue;
        };
        match compile_html(&site.body_text, &site.splice_sources) {
            Ok(dream) => ctx.replace(id, dream),
            Err(msg) => ctx.error(id, msg),
        }
    }
}

fn compile_html(body: &str, splice_sources: &[String]) -> Result<String, String> {
    let fragment = compile_fragment(body.trim(), splice_sources)?;
    Ok(format!("Html.render({})", fragment))
}

fn compile_fragment(body: &str, splices: &[String]) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok("\"\"".into());
    }
    if !trimmed.contains('<') {
        return compile_text_with_splices(trimmed, splices);
    }
    if let Some(compiled) = try_compile_element(trimmed, splices)? {
        return Ok(compiled);
    }
    // Looks like markup but failed to parse as an element.
    if trimmed.starts_with('<') {
        return Err(format!(
            "html syntax block: could not parse markup starting with '{}'",
            trimmed.chars().take(32).collect::<String>()
        ));
    }
    compile_text_with_splices(trimmed, splices)
}

fn try_compile_element(s: &str, splices: &[String]) -> Result<Option<String>, String> {
    let s = s.trim();
    if !s.starts_with('<') {
        return Ok(None);
    }
    let after = &s[1..];
    let name_end = after
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(after.len());
    let tag = &after[..name_end];
    if tag.is_empty() || tag.starts_with('/') {
        return Ok(None);
    }
    let rest = &after[name_end..];
    let Some(close_angle) = rest.find('>') else {
        return Err(format!("html syntax block: unclosed opening tag '<{tag}'"));
    };
    let attrs_raw = &rest[..close_angle];
    let self_closing = attrs_raw.trim_end().ends_with('/');
    let attrs_src = attrs_raw.trim().trim_end_matches('/').trim();
    let after_open = &rest[close_angle + 1..];
    let attrs = compile_attrs(attrs_src)?;
    if self_closing {
        return Ok(Some(format!("Html.el(\"{}\", {}, \"\")", tag, attrs)));
    }
    let close_tag = format!("</{}>", tag);
    let Some(close_pos) = after_open.rfind(&close_tag) else {
        return Err(format!(
            "html syntax block: missing closing tag '</{tag}>'"
        ));
    };
    let inner = compile_children(&after_open[..close_pos], splices)?;
    Ok(Some(format!("Html.el(\"{}\", {}, {})", tag, attrs, inner)))
}

fn compile_attrs(attrs_src: &str) -> Result<String, String> {
    if attrs_src.is_empty() {
        return Ok("[]".into());
    }
    let mut parts = Vec::new();
    let mut rest = attrs_src;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let Some(eq) = rest.find('=') else {
            return Err(format!(
                "html syntax block: attribute missing '=': '{}'",
                rest.chars().take(24).collect::<String>()
            ));
        };
        let key = rest[..eq].trim();
        rest = rest[eq + 1..].trim_start();
        if !rest.starts_with('"') {
            return Err(format!(
                "html syntax block: attribute '{key}' value must be a double-quoted string"
            ));
        }
        let Some(end) = rest[1..].find('"').map(|i| i + 1) else {
            return Err(format!(
                "html syntax block: unclosed attribute value for '{key}'"
            ));
        };
        let val = &rest[1..end];
        parts.push(format!("\"{}\", \"{}\"", key, escape_str(val)));
        rest = &rest[end + 1..];
    }
    if parts.is_empty() {
        Ok("[]".into())
    } else {
        Ok(format!("[{}]", parts.join(", ")))
    }
}

fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn compile_children(inner: &str, splices: &[String]) -> Result<String, String> {
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok("\"\"".into());
    }
    let mut parts = Vec::new();
    let mut rest = inner;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if rest.starts_with('<') {
            if let Some(el) = try_compile_element(rest, splices)? {
                parts.push(el);
                if let Some(len) = element_source_len(rest) {
                    rest = &rest[len..];
                    continue;
                }
            }
            return Err(format!(
                "html syntax block: could not parse nested markup '{}'",
                rest.chars().take(32).collect::<String>()
            ));
        }
        let next_tag = rest.find('<').unwrap_or(rest.len());
        let text = rest[..next_tag].trim();
        if !text.is_empty() {
            parts.push(compile_text_with_splices(text, splices)?);
        }
        rest = &rest[next_tag..];
    }
    if parts.is_empty() {
        Ok("\"\"".into())
    } else {
        Ok(parts.join(" + "))
    }
}

fn element_source_len(s: &str) -> Option<usize> {
    let after = s.strip_prefix('<')?;
    let name_end = after
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(after.len());
    let tag = &after[..name_end];
    let rest = &after[name_end..];
    let close_angle = rest.find('>')?;
    let attrs_raw = &rest[..close_angle];
    if attrs_raw.trim_end().ends_with('/') {
        return Some(1 + name_end + close_angle + 1);
    }
    let after_open = &rest[close_angle + 1..];
    let close_tag = format!("</{}>", tag);
    let close_pos = after_open.find(&close_tag)?;
    Some(1 + name_end + close_angle + 1 + close_pos + close_tag.len())
}

fn compile_text_with_splices(text: &str, splices: &[String]) -> Result<String, String> {
    if text.is_empty() {
        return Ok("\"\"".into());
    }
    let mut result_parts: Vec<String> = Vec::new();
    let mut rest = text;
    let mut splice_i = 0;
    while let Some(start) = rest.find('{') {
        let before = &rest[..start];
        if !before.is_empty() {
            result_parts.push(format!("\"{}\"", escape_str(before)));
        }
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            if splice_i < splices.len() {
                result_parts.push(format!("({})", splices[splice_i]));
                splice_i += 1;
            } else {
                result_parts.push(format!("({})", &after[..end]));
            }
            rest = &after[end + 1..];
        } else {
            return Err(
                "html syntax block: unclosed '{…}' splice (missing closing '}')".to_string(),
            );
        }
    }
    if !rest.is_empty() {
        result_parts.push(format!("\"{}\"", escape_str(rest)));
    }
    if result_parts.is_empty() {
        Ok("\"\"".into())
    } else {
        Ok(result_parts.join(" + "))
    }
}
