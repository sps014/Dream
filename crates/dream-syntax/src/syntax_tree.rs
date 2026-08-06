//! Thin program root wrapper plus a walkable [`SyntaxKind`] facade used by source generators.

use crate::nodes::ProgramNode;

/// Kind tags for the generator-facing syntax walk API (mirrors `system.codegen` / plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxKind {
    CompilationUnit,
    ClassDecl,
    StructDecl,
    EnumDecl,
    InterfaceDecl,
    FieldDecl,
    MethodDecl,
    ConstructorDecl,
    Parameter,
    Attribute,
    SyntaxBlock,
    Splice,
    Text,
    Other,
}

pub struct SyntaxTree<'a> {
    root: ProgramNode<'a>,
}

impl<'a> SyntaxTree<'a> {
    pub fn new(root: ProgramNode<'a>) -> SyntaxTree<'a> {
        SyntaxTree { root }
    }
    pub fn get_root(&self) -> &ProgramNode<'a> {
        &self.root
    }
}
