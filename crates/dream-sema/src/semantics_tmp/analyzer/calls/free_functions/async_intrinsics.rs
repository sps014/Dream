//! The async built-ins: `sleep(ms): Future<void>`, `all(xs): Future<T[]>`, and
//! `any`/`race(xs): Future<T>`.

use super::*;
use crate::intrinsics;
use crate::syntax::nodes::ExpressionNode;

impl<'a> Analyzer<'a> {
    /// Types the async intrinsics: `sleep(ms: int): Future<void>`, `all(xs: Future<T>[]):
    /// Future<T[]>`, `any`/`race(xs: Future<T>[]): Future<T>`.
    pub(crate) fn analyze_async_intrinsic(
        &mut self,
        name: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        if name.text == intrinsics::SLEEP {
            if params.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'sleep' expects exactly 1 argument (milliseconds), got {}",
                        params.len()
                    ),
                    Some(name.position),
                );
            }
            for p in params {
                let pt = self.analyze_expression(p, parent_function, symbol_table, diagnostics)?;
                if !pt.is_int() {
                    diagnostics.report_error(
                        format!("'sleep' expects an int argument, got {}", pt.get_type()),
                        p.position(),
                    );
                }
            }
            return Ok(Type::Unknown);
        }

        // all/any/race take a single `Future<T>[]` argument.
        if params.len() != 1 {
            diagnostics.report_error(
                format!(
                    "'{}' expects exactly 1 argument (a Future array), got {}",
                    name.text,
                    params.len()
                ),
                Some(name.position),
            );
            return Ok(Type::Unknown);
        }
        let arg_type =
            self.analyze_expression(&params[0], parent_function, symbol_table, diagnostics)?;
        let inner_t = match &arg_type {
            Type::Array(inner) => match Self::future_inner_type(inner) {
                Some(t) => t,
                None => {
                    diagnostics.report_error(
                        format!(
                            "'{}' expects an array of Future values, got {}",
                            name.text,
                            arg_type.get_type()
                        ),
                        params[0].position(),
                    );
                    Type::Void
                }
            },
            _ => {
                diagnostics.report_error(
                    format!(
                        "'{}' expects an array of Future values, got {}",
                        name.text,
                        arg_type.get_type()
                    ),
                    params[0].position(),
                );
                Type::Void
            }
        };
        if name.text == intrinsics::PROMISE_ALL {
            // Future<T[]>
            Ok(Self::future_type(Type::Array(Box::new(inner_t))))
        } else {
            // any / race -> Future<T>
            Ok(Self::future_type(inner_t))
        }
    }
}
