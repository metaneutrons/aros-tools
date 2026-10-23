//! One bounded transaction for several ordered expression results.

use super::{
    charge_work, evaluate, retained_cost, Evaluation, MakeExprError, MakeProgramPaths,
    MakeVariableProgram, State, MAX_STATEMENTS, MAX_STORAGE,
};
use std::cell::RefCell;

impl MakeVariableProgram {
    /// Evaluate ordered expressions in one atomic staged state.
    ///
    /// Earlier synchronous effects are visible to later expressions. Failure
    /// publishes neither state nor partial results; shared work budgets are not
    /// reset. One state copy is charged, rather than copying the complete state
    /// for each scalar query. Each result remains independent literal data.
    ///
    /// # Errors
    /// Refuses path mismatches, expression failures and exhausted shared/count/
    /// retained-result bounds. This does not authorize external effects.
    pub fn evaluate_many(
        &mut self,
        expressions: &[&str],
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Vec<String>, MakeExprError> {
        self.check_paths(paths)?;
        check_count(expressions)?;
        let staged = self.stage()?;
        let execution = Evaluation {
            paths,
            expressions: &self.expressions,
            work: &self.state_work,
            file: None,
            captured_wildcard: None,
            captured_realpath: None,
            captured_shell: None,
            automatic_variables: None,
        };
        let results = evaluate_staged(expressions, &staged, &execution)?;
        self.state = staged.into_inner();
        Ok(results)
    }
}

pub(super) const fn check_count(expressions: &[&str]) -> Result<(), MakeExprError> {
    if expressions.len() > MAX_STATEMENTS {
        return Err(MakeExprError::ResourceLimit {
            resource: "Make expression batch count",
            limit: MAX_STATEMENTS,
        });
    }
    Ok(())
}

/// Shared loop; the caller owns the one staged state and publication boundary.
pub(super) fn evaluate_staged(
    expressions: &[&str],
    staged: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<Vec<String>, MakeExprError> {
    check_count(expressions)?;
    let mut results = Vec::with_capacity(expressions.len());
    let mut storage = 0usize;
    for expression in expressions {
        let result = evaluate(expression, usize::MAX, staged, execution)?;
        let cost = retained_cost(result.len())?;
        storage = storage
            .checked_add(cost)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or(MakeExprError::ResourceLimit {
                resource: "Make expression batch results",
                limit: MAX_STORAGE,
            })?;
        charge_work(execution.work, cost)?;
        results.push(result);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dirs::DirVars, TargetContext};
    use std::path::Path;

    fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
        MakeProgramPaths {
            dirs,
            source_root: Path::new("/synthetic/source"),
            relative_dir: Path::new("module"),
            top_directory: Path::new("/synthetic/build"),
            working_directory: Path::new("/synthetic/build/module"),
        }
    }

    #[test]
    fn batch_preserves_order_literal_results_and_synchronous_effects() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        let results = program
            .evaluate_many(
                &[
                    "$(eval A := early)$(A)",
                    "$(eval A := late)$(A)",
                    "$(origin A)",
                    "$(flavor A)",
                    "$(value A)",
                    "literal\n|;$$",
                ],
                &paths,
            )
            .unwrap();
        assert_eq!(
            results,
            ["early", "late", "file", "simple", "late", "literal\n|;$"]
        );
        assert_eq!(program.evaluate("$(A)", &paths).unwrap(), "late");
    }

    #[test]
    fn failed_batch_publishes_no_earlier_effect() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        program.execute("A := before", 0, &paths).unwrap();
        let events = program.events().to_vec();
        assert!(program
            .evaluate_many(&["$(eval A := changed)", "$(unsupported x)"], &paths)
            .is_err());
        assert_eq!(program.events(), events);
        assert_eq!(program.evaluate("$(A)", &paths).unwrap(), "before");
    }

    #[test]
    fn batch_limits_do_not_reset_the_shared_budget() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        assert!(program
            .evaluate_many(&vec![""; MAX_STATEMENTS + 1], &paths)
            .is_err());
        program.state_work.set(super::super::MAX_STATE_WORK - 1);
        assert!(matches!(
            program.evaluate_many(&["literal"], &paths),
            Err(MakeExprError::ResourceLimit { .. })
        ));
    }
}
