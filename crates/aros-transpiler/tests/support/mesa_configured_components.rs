use super::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};

/// The selected source flag components plus the value frozen for GCA.
///
/// Each vector contains GNU Make words in source order, with duplicates kept.
/// `gca_abi_flags` is the exact whitespace-normalized string produced by the
/// reviewed GCA immediate assignment at `gca_assignment_context`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedMesaContract {
    /// Configured target ISA words, before any compilation projection.
    pub target_isa_cflags: Vec<String>,
    /// Source preprocessor words in declaration-time order.
    pub user_cppflags: Vec<String>,
    /// Source C option words in declaration-time order.
    pub user_cflags: Vec<String>,
    /// Source C++ option words in declaration-time order.
    pub user_cxxflags: Vec<String>,
    /// Whitespace-normalized ABI input at the immediate assignment position.
    pub gca_abi_flags: String,
}

/// Evaluate Mesa's named source flag values at their respective Make positions.
///
/// `declaration_context` is the caller's declaration position for the four
/// components. `gca_assignment_context` is the VC4 mmakefile position where
/// `GCA_ABI_FLAGS` is immediately assigned. Missing configured variables,
/// unresolved conditionals, and unsupported expressions remain errors from the
/// strict Make evaluator; an absent value is never treated as known-empty.
///
/// # Errors
/// Rejects unresolved variables, ambiguous values and unsupported expressions.
/// The caller must independently verify the reviewed recipe and both positions.
pub fn evaluate_ordered_mesa_contract(
    declaration_context: &MakeExprContext<'_>,
    gca_assignment_context: &MakeExprContext<'_>,
) -> Result<OrderedMesaContract, MakeExprError> {
    let target_isa_cflags = evaluate_make_list("$(TARGET_ISA_CFLAGS)", declaration_context)?;
    let user_cppflags = evaluate_make_list("$(USER_CPPFLAGS)", declaration_context)?;
    let user_cflags = evaluate_make_list("$(USER_CFLAGS)", declaration_context)?;
    let user_cxxflags = evaluate_make_list("$(USER_CXXFLAGS)", declaration_context)?;

    // Keep this expression aligned with the source's immediate := assignment;
    // evaluating at the caller's declaration site would miss Make's freeze.
    let gca_abi_flags = evaluate_make_expr(
        "$(strip $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS))",
        gca_assignment_context,
    )?;

    Ok(OrderedMesaContract {
        target_isa_cflags,
        user_cppflags,
        user_cflags,
        user_cxxflags,
        gca_abi_flags,
    })
}
