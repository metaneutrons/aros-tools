//! GNU Make's read-time input list, not an inferred filesystem inventory.

use super::super::{
    apply_scalar_fragment, invalid, record_assignment, record_ignored, AssignmentValue, Evaluation,
    State, VariableFlavor,
};
use crate::make_expr::MakeExprError;
use aros_common::make_namespace::MakeVariableOrigin;
use std::cell::RefCell;

pub(super) fn enter(
    input: &str,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    let before = data.borrow().events.len();
    if !data.borrow().file_read_started {
        // GNU initializes this builtin immediately before the first input,
        // replacing environment and pre-file eval values at file priority.
        // Ordinary command-line precedence still applies.
        apply_scalar_fragment("MAKEFILE_LIST :=", 0, Some(true), data, execution)?;
        data.borrow_mut().file_read_started = true;
    }
    let origin = data.borrow().origins.get("MAKEFILE_LIST").copied();
    if origin == Some(MakeVariableOrigin::CommandLine) {
        record_ignored("MAKEFILE_LIST +=", 0, Some(true), data)?;
    } else {
        let flavor = {
            let state = data.borrow();
            if let Some(flavor) = state.flavors.get("MAKEFILE_LIST") {
                *flavor
            } else if state
                .scope
                .is_defined_before("MAKEFILE_LIST", state.events.len())
            {
                return Err(invalid("Make input list requires a known variable flavor"));
            } else {
                VariableFlavor::Recursive
            }
        };
        let frozen = (flavor == VariableFlavor::Simple).then(|| input.replace('$', "$$"));
        record_assignment(
            "MAKEFILE_LIST +=",
            "MAKEFILE_LIST",
            AssignmentValue {
                frozen: frozen.as_deref(),
                flavor: Some(flavor),
                raw: Some(input),
            },
            0,
            Some(true),
            data,
        )?;
    }
    for event in &mut data.borrow_mut().events[before..] {
        event.input_effect = true;
    }
    Ok(())
}
