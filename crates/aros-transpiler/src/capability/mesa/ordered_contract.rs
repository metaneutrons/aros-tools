//! Ordered source evaluation for the reviewed Mesa VC4 compile/ABI inputs.
//!
//! The source's `config/make.tmpl` compile rules pass `CFLAGS` before
//! `CPPFLAGS`. `config/target.cfg.in` recursively builds `TARGET_CFLAGS` and
//! `TARGET_CXXFLAGS` from optimization, configuration, and source-local user
//! flags; `TARGET_CPPFLAGS` combines user includes, configured preprocessor
//! flags, and source-local `USER_CPPFLAGS`. `config/make.cfg.in` adds the
//! separate C or C++ ISA flags and appends `LLVM_NOWARN` to CFLAGS only. The
//! compiler-options evaluator below projects reviewed recursive Make
//! definitions into C, C++, and VC4 bridge word sequences. These are
//! option-sequence projections, not complete
//! compiler invocations: driver, sysroot, generated include, and other
//! invocation inputs remain outside this boundary. Callers must provide the
//! reviewed source definitions for recursive `TARGET_*`, `CFLAGS`, `CXXFLAGS`,
//! and `CPPFLAGS`; configured global frozen `TARGET_*FLAGS` values must not
//! replace those expressions after local `USER_*` assignments.
//!
//! The GCA string follows the source's immediate assignment
//! `GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)` and its later
//! `$(strip $(GCA_ABI_FLAGS))` use. Its context must point immediately after that assignment,
//! so subsequent local assignments do not alter the frozen value.

use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use serde::{Deserialize, Serialize};

/// Source-ordered option projections for Mesa C, C++, and the VC4 GCA bridge.
///
/// The vectors preserve GNU Make word order and duplicates. They are not full
/// compiler invocations and must not be treated as a driver, sysroot, or
/// generated-include contract. `gca_abi_flags` is evaluated independently at
/// the source's immediate GCA assignment, so later local changes do not alter
/// the frozen ABI input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderedMesaCompilerOptions {
    /// `$(CFLAGS) $(CPPFLAGS)` at the declaration position.
    pub c: Vec<String>,
    /// `$(CXXFLAGS) $(CPPFLAGS)` at the declaration position.
    pub cxx: Vec<String>,
    /// `$(CFLAGS) $(USER_CFLAGS) $(CPPFLAGS) $(USER_CPPFLAGS) $(USER_INCLUDES)`
    /// at the declaration position, retained as the separate VC4 bridge view.
    pub vc4_bridge: Vec<String>,
    /// Whitespace-normalized value of the immediate GCA assignment expression.
    pub gca_abi_flags: String,
}

/// Evaluate Mesa's reviewed C/C++ option projections and the frozen GCA value.
///
/// `declaration_context` must point at the relevant source declaration, with
/// the recursive Make formulas for `CFLAGS`, `CXXFLAGS`, and `CPPFLAGS` in its
/// scope. `gca_assignment_context` must point immediately after
/// the selected source's immediate `GCA_ABI_FLAGS` assignment. The projections use
/// expressions `$(CFLAGS) $(CPPFLAGS)`, `$(CXXFLAGS) $(CPPFLAGS)`, and
/// `$(CFLAGS) $(USER_CFLAGS) $(CPPFLAGS) $(USER_CPPFLAGS) $(USER_INCLUDES)`.
/// The GCA value reads the source-frozen variable, never a reconstructed RHS.
///
/// This reports option words only; it does not claim to capture driver,
/// sysroot, generated include, or every compiler invocation input.
///
/// # Errors
/// Rejects unresolved variables, ambiguous values and unsupported expressions.
pub fn evaluate_ordered_mesa_compiler_options(
    declaration_context: &MakeExprContext<'_>,
    gca_assignment_context: &MakeExprContext<'_>,
) -> Result<OrderedMesaCompilerOptions, MakeExprError> {
    evaluate_ordered_mesa_options(
        declaration_context,
        Some(gca_assignment_context),
        "$(CFLAGS) $(CPPFLAGS)",
        "$(CXXFLAGS) $(CPPFLAGS)",
    )
}

// Only the owning recipe projection may omit an unselected GCA assignment.
// Public compile/ABI projection always requires an explicit source context.
fn evaluate_ordered_mesa_options(
    declaration_context: &MakeExprContext<'_>,
    gca_assignment_context: Option<&MakeExprContext<'_>>,
    c_expression: &str,
    cxx_expression: &str,
) -> Result<OrderedMesaCompilerOptions, MakeExprError> {
    let c = evaluate_make_list(c_expression, declaration_context)?;
    let cxx = evaluate_make_list(cxx_expression, declaration_context)?;
    let vc4_bridge = evaluate_make_list(
        "$(CFLAGS) $(USER_CFLAGS) $(CPPFLAGS) $(USER_CPPFLAGS) $(USER_INCLUDES)",
        declaration_context,
    )?;
    let gca_abi_flags = gca_assignment_context
        .map(|context| evaluate_make_expr("$(strip $(GCA_ABI_FLAGS))", context))
        .transpose()?
        .unwrap_or_default();

    Ok(OrderedMesaCompilerOptions {
        c,
        cxx,
        vc4_bridge,
        gca_abi_flags,
    })
}

/// Add the measured default target-recipe LTO/safety lane in source order.
/// The handwritten bridge is a separate recipe and keeps its own expression.
/// Driver, sysroot, quote roots and source-name mapping still require their
/// structural handoff; this is not a complete compiler-command claim.
pub(super) fn evaluate_ordered_mesa_recipe_options(
    declaration: &MakeExprContext<'_>,
    gca: Option<&MakeExprContext<'_>>,
) -> Result<OrderedMesaCompilerOptions, MakeExprError> {
    let lto = evaluate_make_expr("$(TARGET_LTO)", declaration)?;
    let prefix = match lto.as_str() {
        "yes" => "$(LTO_CFLAGS) ",
        "no" => "",
        _ => {
            return Err(MakeExprError::InvalidSyntax {
                expression: "$(TARGET_LTO)".into(),
                detail: "unsupported configured LTO selection".into(),
            })
        }
    };
    evaluate_ordered_mesa_options(
        declaration,
        gca,
        &format!("{prefix}$(CFLAGS) $(SAFETY_CFLAGS) $(CPPFLAGS)"),
        &format!("{prefix}$(CXXFLAGS) $(SAFETY_CFLAGS) $(CPPFLAGS)"),
    )
}

#[cfg(test)]
#[path = "ordered_contract_tests.rs"]
mod tests;
