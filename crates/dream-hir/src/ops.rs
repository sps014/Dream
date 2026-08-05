//! IR-level operator vocabulary, decoupled from the parser's `TokenKind` so the middle/back-end
//! never re-inspects syntax. The analyzer maps surface operators to these when emitting HIR.

/// Binary operators. Comparison and logical operators always produce `bool`; arithmetic/bitwise
/// operators produce the (already type-checked) operand type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl BinOp {
    /// True if the result is always `bool` regardless of operand type.
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    /// True for the short-circuiting logical connectives (`&&`/`||`), which lower to control flow
    /// rather than a single arithmetic instruction.
    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    /// Arithmetic negation (`-x`).
    Neg,
    /// Logical negation (`!x`).
    Not,
    /// Bitwise complement (`~x`). Integer operands only (`int`/`long`/`uint`/`ulong`/`byte`).
    BitNot,
}
