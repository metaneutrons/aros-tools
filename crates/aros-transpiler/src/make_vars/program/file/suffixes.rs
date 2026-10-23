//! Source-ordered suffix-list data; not an implicit-rule executor.

use super::{
    invalid, FileContext, MakeFileEventKind, MakeFileProgram, RuleHeader, State, MAX_EVENTS,
};
use crate::make_expr::MakeExprError;
use std::cell::RefCell;

const MAX_NAMES: usize = 65_536;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORK: usize = 1_048_576;

impl FileContext<'_> {
    pub(super) fn suffixes(
        &self,
        header: RuleHeader,
        source_header: &str,
        data: &RefCell<State>,
    ) -> Result<(), MakeExprError> {
        let RuleHeader::Rule {
            prerequisites,
            target_pattern: None,
            order_only,
            double_colon: false,
            inline_recipe: None,
            ..
        } = header
        else {
            return Err(invalid("unsupported Make suffix declaration form"));
        };
        if !order_only.is_empty() {
            return Err(invalid("order-only Make suffix declaration is not modeled"));
        }
        // The surrounding rule reader currently uses Unicode trim/split
        // operations. Refuse these spellings before they can silently become
        // different GNU byte-level words, including raw leading/trailing text
        // that an earlier trim may already have removed. Comments are excluded
        // by the caller; semantic expanded data is checked separately.
        if [source_header, prerequisites.as_str()]
            .into_iter()
            .any(|value| {
                value
                    .chars()
                    .any(|character| character.is_whitespace() && !character.is_ascii())
            })
        {
            return Err(invalid(
                "non-ASCII whitespace in Make suffix declarations is not modeled",
            ));
        }
        let count = prerequisites.split_whitespace().take(MAX_NAMES + 1).count();
        if count > MAX_NAMES {
            return Err(invalid("Make suffix name count exceeds limit"));
        }
        let retained = prerequisites
            .len()
            .checked_add(count.saturating_mul(32))
            .ok_or_else(|| invalid("Make suffix declaration exceeds size limit"))?;
        let names = prerequisites
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        self.emit(MakeFileEventKind::Suffixes(names), true, retained, data)?;
        // A recipe following a special declaration must never attach to the
        // preceding ordinary rule. Recipes on this declaration remain refused.
        self.pending.set(None);
        Ok(())
    }
}

impl MakeFileProgram {
    /// Resolve final suffix-list data from an explicitly selected initial list.
    ///
    /// `None` stays unknown until a source reset; it is never replaced with a
    /// guessed built-in list or inferred from a configured architecture. An
    /// empty declaration resets and a nonempty declaration appends all names
    /// in order, including duplicates. Declarations remain intact in the event stream.
    /// This does not interpret suffix-rule bodies, admit an implicit graph or
    /// authenticate that the supplied initial list came from native Make.
    ///
    /// # Errors
    /// Refuses malformed input words or exceeded event, name, byte/work bounds.
    pub fn suffixes<'a>(
        &'a self,
        initial: Option<&'a [String]>,
    ) -> Result<Option<Vec<String>>, String> {
        if self.events.len() > MAX_EVENTS {
            return Err("Make suffix program exceeds event limit".into());
        }
        let mut known = initial.is_some();
        let mut names = Vec::<&str>::new();
        let mut work = 0usize;
        let mut scanned = 0usize;
        let mut retained = 0usize;
        let mut append = |values: &'a [String],
                          known: bool,
                          names: &mut Vec<&'a str>,
                          retained: &mut usize|
         -> Result<(), String> {
            if values.len() > MAX_NAMES {
                return Err("Make suffix name count exceeds limit".into());
            }
            for value in values {
                work = work
                    .checked_add(1)
                    .filter(|count| *count <= MAX_WORK)
                    .ok_or("Make suffix processing exceeds work limit")?;
                scanned = scanned
                    .checked_add(value.len())
                    .filter(|bytes| *bytes <= MAX_BYTES)
                    .ok_or("Make suffix processing exceeds byte limit")?;
                if value.is_empty()
                    || value
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
                {
                    return Err("Make suffix input is not one literal word".into());
                }
                if known {
                    if names.len() == MAX_NAMES {
                        return Err("Make suffix result exceeds name limit".into());
                    }
                    *retained = retained
                        .checked_add(value.len())
                        .filter(|bytes| *bytes <= MAX_BYTES)
                        .ok_or("Make suffix result exceeds byte limit")?;
                    names.push(value);
                }
            }
            Ok(())
        };
        if let Some(initial) = initial {
            append(initial, true, &mut names, &mut retained)?;
        }
        for event in &self.events {
            if !event.active {
                continue;
            }
            if let MakeFileEventKind::Suffixes(values) = &event.kind {
                if values.is_empty() {
                    known = true;
                    names.clear();
                    retained = 0;
                } else {
                    append(values, known, &mut names, &mut retained)?;
                }
            }
        }
        Ok(known.then(|| names.into_iter().map(str::to_owned).collect()))
    }
}
