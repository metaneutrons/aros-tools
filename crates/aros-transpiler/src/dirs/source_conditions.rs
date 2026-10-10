//! Conservative certainty tracking for source-declared Make conditionals.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BranchCertainty {
    Inactive,
    Uncertain,
    Definite,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PriorSelection {
    None,
    Selected,
    Maybe,
}

pub(super) struct SourceCondition {
    prior_selection: PriorSelection,
    current: BranchCertainty,
    pub(super) legacy_supported: bool,
}

impl SourceCondition {
    pub(super) const fn from_condition(value: Option<bool>, legacy_supported: bool) -> Self {
        Self {
            prior_selection: PriorSelection::None,
            current: condition_certainty(value),
            legacy_supported,
        }
    }

    pub(super) const fn unknown() -> Self {
        Self::from_condition(None, false)
    }

    pub(super) const fn else_alternative(&mut self) {
        self.prior_selection = merge_prior_selection(self.prior_selection, self.current);
        self.current = BranchCertainty::Definite;
    }

    pub(super) const fn next_alternative(&mut self, value: Option<bool>) {
        self.prior_selection = merge_prior_selection(self.prior_selection, self.current);
        self.current = condition_certainty(value);
    }

    fn certainty(&self) -> BranchCertainty {
        match self.prior_selection {
            PriorSelection::Selected => BranchCertainty::Inactive,
            PriorSelection::None => self.current,
            PriorSelection::Maybe if self.current == BranchCertainty::Inactive => {
                BranchCertainty::Inactive
            }
            PriorSelection::Maybe => BranchCertainty::Uncertain,
        }
    }
}

const fn condition_certainty(value: Option<bool>) -> BranchCertainty {
    match value {
        Some(true) => BranchCertainty::Definite,
        Some(false) => BranchCertainty::Inactive,
        None => BranchCertainty::Uncertain,
    }
}

const fn merge_prior_selection(prior: PriorSelection, current: BranchCertainty) -> PriorSelection {
    match prior {
        PriorSelection::Selected => PriorSelection::Selected,
        PriorSelection::Maybe => PriorSelection::Maybe,
        PriorSelection::None => match current {
            BranchCertainty::Inactive => PriorSelection::None,
            BranchCertainty::Uncertain => PriorSelection::Maybe,
            BranchCertainty::Definite => PriorSelection::Selected,
        },
    }
}

pub(super) fn source_branch_certainty(conditions: &[SourceCondition]) -> BranchCertainty {
    let mut uncertain = false;
    for condition in conditions {
        match condition.certainty() {
            BranchCertainty::Inactive => return BranchCertainty::Inactive,
            BranchCertainty::Uncertain => uncertain = true,
            BranchCertainty::Definite => {}
        }
    }
    if uncertain {
        BranchCertainty::Uncertain
    } else {
        BranchCertainty::Definite
    }
}
