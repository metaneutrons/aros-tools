//! Expand only a computed assignment name, never its deferred right-hand side.

use super::{charge_work, evaluate, invalid, literal_variable_name, Evaluation, State, MAX_VALUE};
use crate::make_expr::MakeExprError;
use std::cell::RefCell;

pub(super) fn resolve(
    lhs: &str,
    operator: &str,
    rhs: &str,
    line: usize,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<String, MakeExprError> {
    if data.borrow().coordinate_projection {
        return Err(invalid(
            "computed Make assignments require complete source interpretation",
        ));
    }
    let name = evaluate(lhs.trim(), line, data, execution)?;
    if !literal_variable_name(&name) {
        return Err(invalid("computed Make assignment name is unsupported"));
    }
    let bytes = name
        .len()
        .checked_add(operator.len())
        .and_then(|size| size.checked_add(rhs.len()))
        .filter(|size| *size <= MAX_VALUE)
        .ok_or_else(|| invalid("computed Make assignment exceeds its byte limit"))?;
    charge_work(execution.work, bytes)?;
    let mut result = String::with_capacity(bytes);
    result.push_str(&name);
    result.push_str(operator);
    result.push_str(rhs);
    Ok(result)
}
