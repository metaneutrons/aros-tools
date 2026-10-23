//! Retained special fallback declarations, not ordinary dependency edges.

use super::{header_bytes, invalid, FileContext, MakeFileEventKind, RuleHeader, State};
use crate::make_expr::MakeExprError;
use std::cell::RefCell;

impl FileContext<'_> {
    pub(super) fn default_rule(
        &self,
        header: RuleHeader,
        data: &RefCell<State>,
    ) -> Result<(), MakeExprError> {
        let RuleHeader::Rule {
            target_pattern: None,
            double_colon,
            ..
        } = &header
        else {
            return Err(invalid(
                "unsupported Make default-fallback declaration form",
            ));
        };
        if self
            .default_colon
            .get()
            .is_some_and(|prior| prior != *double_colon)
        {
            return Err(invalid("mixed single/double-colon Make default rules"));
        }
        self.default_colon.set(Some(*double_colon));
        let bytes = header_bytes(&header);
        let event = self.emit(MakeFileEventKind::DefaultRule(header), true, bytes, data)?;
        self.pending.set(Some(event));
        Ok(())
    }
}
