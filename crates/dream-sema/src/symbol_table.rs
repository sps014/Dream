use crate::errors::SymbolError;
use dream_syntax::nodes::Type;
use dream_syntax::token::syntax_token::SyntaxToken;
use dream_text::text_span::TextSpan;
use indexmap::IndexMap;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Debug)]
pub struct SymbolTable {
    /// Insertion-ordered (declaration order) so codegen emits each function's `(local ...)`
    /// declarations and function-exit releases in a deterministic order.
    symbols: IndexMap<String, Type>,
    /// Names declared with `const` in this scope; reassigning them is an error.
    const_symbols: HashSet<String>,
    /// Locals (`let`/`const`/tuple bindings) subject to unused-variable warnings, with decl span.
    tracked_locals: IndexMap<String, TextSpan>,
    /// Names that have been read (not merely assigned to) in this scope or via lookup here.
    used_locals: HashSet<String>,
    parent: Option<Rc<RefCell<SymbolTable>>>,
    pub children: Vec<Rc<RefCell<SymbolTable>>>,
}

impl SymbolTable {
    pub fn new(parent: Option<Rc<RefCell<SymbolTable>>>) -> SymbolTable {
        SymbolTable {
            symbols: IndexMap::new(),
            const_symbols: HashSet::new(),
            tracked_locals: IndexMap::new(),
            used_locals: HashSet::new(),
            parent,
            children: Vec::new(),
        }
    }

    /// Marks a name as immutable (`const`) within this scope.
    pub fn mark_const(&mut self, name: String) {
        self.const_symbols.insert(name);
    }

    /// Returns true if `name` resolves to a `const` binding in this scope or an enclosing one.
    pub fn is_const(&self, name: &str) -> bool {
        if self.const_symbols.contains(name) {
            return true;
        }
        // Only consult the parent if the name is not shadowed by a local declaration here.
        if self.symbols.contains_key(name) {
            return false;
        }
        match self.parent {
            Some(ref parent) => parent.as_ref().borrow().is_const(name),
            None => false,
        }
    }

    pub fn add_child(&mut self, child: Rc<RefCell<SymbolTable>>) {
        self.children.push(child);
    }

    pub fn add_symbol(&mut self, name: String, token: Type) -> Result<(), SymbolError> {
        match self.symbols.insert(name.clone(), token) {
            Some(_) => Err(SymbolError::new(format!(
                "variable {} already exists at: {}",
                name,
                self.symbols.get(&name).unwrap().get_line_str()
            ))),
            None => Ok(()),
        }
    }

    /// Registers a user `let`/`const`/destructure binding for unused-variable warnings.
    pub fn track_local(&mut self, name: String, span: TextSpan) {
        if name.starts_with("__") || name == "_" {
            return;
        }
        self.tracked_locals.insert(name, span);
    }

    /// Records that `name` was read. Walks parents so a use in an inner scope counts.
    pub fn mark_used(&mut self, name: &str) {
        if self.symbols.contains_key(name) {
            self.used_locals.insert(name.to_string());
            return;
        }
        if let Some(ref parent) = self.parent {
            parent.as_ref().borrow_mut().mark_used(name);
        }
    }

    /// Emits unused-local warnings for this scope and all child scopes.
    pub fn report_unused_locals(&self, diagnostics: &mut dream_diagnostics::DiagnosticBag) {
        for (name, span) in &self.tracked_locals {
            if !self.used_locals.contains(name) {
                diagnostics.report_warning(
                    format!("unused variable '{}'", name),
                    Some(*span),
                );
            }
        }
        for child in &self.children {
            child.as_ref().borrow().report_unused_locals(diagnostics);
        }
    }

    pub fn get_symbol(&self, name: &SyntaxToken) -> Result<Type, SymbolError> {
        if let Some(symbol) = self.symbols.get(&name.text) {
            return Ok(symbol.clone());
        }

        match self.parent {
            Some(ref parent) => parent.as_ref().borrow().get_symbol(name),
            None => Err(SymbolError::new(format!(
                "variable {} does not exist at: {}",
                name.text,
                name.position.get_point_str()
            ))),
        }
    }
}
