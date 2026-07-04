//! Synchronous regex host functions (the `Dream` module behind `src/stdlib/text/regex.dream`),
//! implemented with the `regex` crate. These mirror the JS helpers in `runtime/dream.js` so
//! `Regex.test`/`replace`/`match` behave the same on wasmtime, Node, and the browser (for the
//! common pattern subset the `regex` crate supports).

use wasmtime::*;

use super::memory::{read_arg_string, write_string_to_memory};

/// Builds a `regex::Regex` from a pattern and a JS-style flags string ("i"/"m"/"s"; the global
/// "g" flag is handled per call site, and "u"/"y" have no Rust equivalent). Returns `None` on a
/// compile error (e.g. a pattern using lookaround/backreferences, which the `regex` crate rejects),
/// so callers can fall back to a safe default.
fn build_regex(pattern: &str, flags: &str) -> Option<regex::Regex> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .build()
        .ok()
}

/// Registers the synchronous regex host functions on `linker`.
pub fn link_regex_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap(
        "Dream",
        "regexTest",
        |mut caller: Caller<'_, ()>,
         pattern_ptr: i32,
         flags_ptr: i32,
         input_ptr: i32|
         -> Result<i32> {
            let pattern = read_arg_string(&mut caller, pattern_ptr)?;
            let flags = read_arg_string(&mut caller, flags_ptr)?;
            let input = read_arg_string(&mut caller, input_ptr)?;
            Ok(build_regex(&pattern, &flags).map_or(0, |re| re.is_match(&input) as i32))
        },
    )?;

    linker.func_wrap(
        "Dream",
        "regexReplace",
        |mut caller: Caller<'_, ()>,
         pattern_ptr: i32,
         flags_ptr: i32,
         input_ptr: i32,
         replacement_ptr: i32|
         -> Result<i32> {
            let pattern = read_arg_string(&mut caller, pattern_ptr)?;
            let flags = read_arg_string(&mut caller, flags_ptr)?;
            let input = read_arg_string(&mut caller, input_ptr)?;
            let replacement = read_arg_string(&mut caller, replacement_ptr)?;
            let out = match build_regex(&pattern, &flags) {
                Some(re) => {
                    if flags.contains('g') {
                        re.replace_all(&input, replacement.as_str()).into_owned()
                    } else {
                        re.replace(&input, replacement.as_str()).into_owned()
                    }
                }
                None => input.clone(),
            };
            write_string_to_memory(&mut caller, &out)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "regexMatchJoined",
        |mut caller: Caller<'_, ()>,
         pattern_ptr: i32,
         flags_ptr: i32,
         input_ptr: i32,
         sep_ptr: i32|
         -> Result<i32> {
            let pattern = read_arg_string(&mut caller, pattern_ptr)?;
            let flags = read_arg_string(&mut caller, flags_ptr)?;
            let input = read_arg_string(&mut caller, input_ptr)?;
            let sep = read_arg_string(&mut caller, sep_ptr)?;
            let joined = match build_regex(&pattern, &flags) {
                Some(re) => {
                    if flags.contains('g') {
                        // Global: every full match (no capture groups), like JS `match` with `g`.
                        re.find_iter(&input)
                            .map(|m| m.as_str().to_string())
                            .collect::<Vec<_>>()
                            .join(&sep)
                    } else {
                        // Non-global: the first match plus its capture groups, like JS `match`
                        // without `g`. Missing optional groups render as "".
                        match re.captures(&input) {
                            Some(caps) => (0..caps.len())
                                .map(|i| caps.get(i).map_or("", |m| m.as_str()).to_string())
                                .collect::<Vec<_>>()
                                .join(&sep),
                            None => String::new(),
                        }
                    }
                }
                None => String::new(),
            };
            write_string_to_memory(&mut caller, &joined)
        },
    )?;

    Ok(())
}
