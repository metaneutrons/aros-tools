//! Generic, positioned ordinary/static-pattern rule and recipe events.
//!
//! This layer does not run recipes, search the filesystem, select compiler
//! roles or authenticate inputs. Callers retain the original variable scope,
//! expand headers at their own source position, and must account for every
//! `other_lines` event before claiming complete program coverage.

#[cfg(test)]
use crate::make_rule_header::{parse, RuleHeader};
#[cfg(test)]
use crate::make_rule_program::RuleProgram;
#[cfg(test)]
use crate::make_vars::{
    directive_tail, make_define_suppression, strip_make_comment, variable_assignment,
};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
const MAX_PROGRAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_LINES: usize = 262_144;
const MAX_RULES: usize = 16_384;
const MAX_WORDS: usize = 16_384;
const MAX_VALUE_BYTES: usize = 1024 * 1024;
const MAX_INSTANCE_BYTES: usize = 64 * 1024 * 1024;

/// Deferred recipe text and its output/original joined-source positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRecipe {
    pub output_line: usize,
    pub source_line: usize,
    /// Command prefixes remain intact; only Make's initial tab is removed.
    pub text: String,
}

/// An original rule before positional expressions or static stems are resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRule {
    pub output_line: usize,
    pub source_line: usize,
    pub targets: String,
    pub target_pattern: Option<String>,
    pub prerequisites: String,
    pub order_only: String,
    pub double_colon: bool,
    pub recipes: Vec<SourceRecipe>,
}

/// Parsed rules plus statements requiring separate scope/effect accounting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleEvents {
    pub rules: Vec<SourceRule>,
    pub other_lines: Vec<usize>,
    pub target_assignment_lines: Vec<usize>,
    pub search_paths: Vec<SearchPathDeclaration>,
}

/// A selective directory-search declaration, still unevaluated at this stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPathDeclaration {
    pub output_line: usize,
    pub source_line: usize,
    pub arguments: String,
}

/// Collect rule/recipe events without flattening their original positions.
///
/// Conditional directives do not break a pending recipe, but variable
/// assignments do. The supplied branch states must already be independently
/// resolved; active unknown rules/recipes refuse. Definitions are data. This
/// is not an assertion that other Make statements have been interpreted.
///
/// # Errors
/// Rejects inconsistent/oversized inputs, unsupported headers, unresolved rule
/// selection and orphan recipes.
#[cfg(test)]
pub(crate) fn collect(program: &RuleProgram) -> Result<RuleEvents, String> {
    if program.text.len() > MAX_PROGRAM_BYTES
        || program.lines.len() > MAX_LINES
        || program.lines.len() != program.text.lines().count()
    {
        return Err("invalid positioned Make rule-event input or limits".into());
    }
    let suppressed = make_define_suppression(&program.text);
    let mut result = RuleEvents::default();
    let mut pending = None;
    for (output_line, raw) in program.text.lines().enumerate() {
        let origin = &program.lines[output_line];
        if origin.active == Some(false) {
            continue;
        }
        if suppressed[output_line] {
            pending = None;
            continue;
        }
        if let Some(text) = raw.strip_prefix('\t') {
            if origin.active != Some(true) {
                return Err("Make recipe event has unresolved selection".into());
            }
            let index: usize = pending.ok_or("Make recipe has no active rule")?;
            result.rules[index].recipes.push(SourceRecipe {
                output_line,
                source_line: origin.source_line,
                text: text.to_owned(),
            });
            continue;
        }
        let visible = strip_make_comment(raw).trim();
        if visible.is_empty() {
            continue;
        }
        if ["ifeq", "ifneq", "ifdef", "ifndef", "else", "endif"]
            .iter()
            .any(|word| directive_tail(visible, word).is_some())
        {
            continue;
        }
        if variable_assignment(visible).is_none() {
            if let Some(arguments) = directive_tail(visible, "vpath") {
                if origin.active != Some(true) {
                    return Err("Make vpath event has unresolved selection".into());
                }
                if result.search_paths.len() == MAX_RULES || arguments.len() > MAX_VALUE_BYTES {
                    return Err("Make vpath events exceed declaration count or byte limit".into());
                }
                result.search_paths.push(SearchPathDeclaration {
                    output_line,
                    source_line: origin.source_line,
                    arguments: arguments.trim().to_owned(),
                });
                pending = None;
                continue;
            }
        }
        match parse(raw)? {
            Some(RuleHeader::Rule {
                targets,
                target_pattern,
                prerequisites,
                order_only,
                double_colon,
                inline_recipe,
            }) => {
                if origin.active != Some(true) {
                    return Err("Make rule event has unresolved selection".into());
                }
                if result.rules.len() == MAX_RULES {
                    return Err("Make rule events exceed rule limit".into());
                }
                let recipes = inline_recipe
                    .map(|text| SourceRecipe {
                        output_line,
                        source_line: origin.source_line,
                        text,
                    })
                    .into_iter()
                    .collect();
                result.rules.push(SourceRule {
                    output_line,
                    source_line: origin.source_line,
                    targets,
                    target_pattern,
                    prerequisites,
                    order_only,
                    double_colon,
                    recipes,
                });
                pending = Some(result.rules.len() - 1);
            }
            Some(RuleHeader::TargetAssignment { .. }) => {
                if origin.active != Some(true) {
                    return Err("Make target-assignment event has unresolved selection".into());
                }
                result.target_assignment_lines.push(output_line);
                pending = None;
            }
            None => {
                result.other_lines.push(output_line);
                pending = None;
            }
        }
    }
    Ok(result)
}

/// One concrete rule instance; no path-existence or compiler claim is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleInstance {
    pub output_line: usize,
    pub source_line: usize,
    pub target: String,
    pub stem: Option<String>,
    pub prerequisites: Vec<String>,
    pub order_only: Vec<String>,
    /// Declaration-local normal/order-only edge order, including repetitions.
    /// `$+` follows the normal list, while `$^` can begin with an earlier
    /// order-only prerequisite promoted to normal by a subsequent declaration.
    pub prerequisite_order: Vec<String>,
    pub double_colon: bool,
    pub recipes: Vec<SourceRecipe>,
}

impl RuleInstance {
    pub(crate) fn validate_declaration_counts(&self) -> Result<(), String> {
        if self.prerequisites.len() > MAX_WORDS
            || self.order_only.len() > MAX_WORDS
            || self.recipes.len() > MAX_LINES
            || self.prerequisite_order.len() != self.prerequisites.len() + self.order_only.len()
        {
            return Err(
                "Make prerequisite order has inconsistent size or exceeds collection limit".into(),
            );
        }
        Ok(())
    }

    /// Source declarations have a normal list followed by an order-only list.
    /// A merged effective instance is not a new source declaration and may
    /// legitimately interleave these lists across its contributing declarations.
    pub(crate) fn validate_declaration_order(&self) -> Result<(), String> {
        self.validate_declaration_counts()?;
        if !self
            .prerequisite_order
            .iter()
            .eq(self.prerequisites.iter().chain(&self.order_only))
        {
            return Err("Make prerequisite order differs from its source declaration".into());
        }
        Ok(())
    }

    /// Known pure automatic values. Time-dependent `$?`, archive-member `$%`
    /// and an explicit rule's suffix-derived `$*` are intentionally not guessed.
    #[must_use]
    pub fn automatic_variables(&self) -> BTreeMap<String, String> {
        let first = self.prerequisites.first().map_or("", String::as_str);
        let normal_set = self
            .prerequisites
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let normal = unique_words(&self.prerequisite_order)
            .into_iter()
            .filter(|word| normal_set.contains(word))
            .collect::<Vec<_>>();
        let order_only = unique_words(&self.order_only)
            .into_iter()
            .filter(|word| !normal_set.contains(word))
            .collect::<Vec<_>>()
            .join(" ");
        let mut values = BTreeMap::from([
            ("@".into(), self.target.clone()),
            ("<".into(), first.to_owned()),
            ("^".into(), normal.join(" ")),
            ("+".into(), self.prerequisites.join(" ")),
            ("|".into(), order_only),
        ]);
        for (name, value) in [("@", self.target.as_str()), ("<", first)] {
            insert_path_parts(&mut values, name, value);
        }
        for (name, paths) in [
            ("^", normal),
            ("+", self.prerequisites.iter().map(String::as_str).collect()),
        ] {
            let parts = paths.into_iter().map(path_parts).collect::<Vec<_>>();
            values.insert(
                format!("{name}D"),
                parts
                    .iter()
                    .map(|(directory, _)| *directory)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            values.insert(
                format!("{name}F"),
                parts
                    .iter()
                    .map(|(_, file)| *file)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        if let Some(stem) = &self.stem {
            values.insert("*".into(), stem.clone());
            insert_path_parts(&mut values, "*", stem);
        }
        values
    }
}

fn unique_words(words: &[String]) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    words
        .iter()
        .map(String::as_str)
        .filter(|word| seen.insert(*word))
        .collect()
}

fn insert_path_parts(values: &mut BTreeMap<String, String>, name: &str, path: &str) {
    let (directory, file) = path_parts(path);
    values.insert(format!("{name}D"), directory.to_owned());
    values.insert(format!("{name}F"), file.to_owned());
}

fn path_parts(path: &str) -> (&str, &str) {
    if path.is_empty() {
        ("", "")
    } else {
        path.rsplit_once('/').unwrap_or((".", path))
    }
}

/// Resolve a finite explicit/static-pattern rule at its original header position.
///
/// Word order and duplicates in prerequisites survive. A static stem includes
/// directories, as in `obj/%.o : src/%.c`. No implicit-rule search, suffix-rule
/// inference, VPATH lookup, inheritance or recipe execution happens here.
/// Callers must additionally bind paths to their allowed source/output roots.
///
/// # Errors
/// Rejects unresolved/control characters, nonfinite targets, duplicate targets,
/// unsupported patterns, mismatches or resource exhaustion.
pub fn instantiate(
    rule: &SourceRule,
    evaluate: &impl Fn(&str, usize) -> Result<String, String>,
) -> Result<Vec<RuleInstance>, String> {
    let targets = words(&evaluate(&rule.targets, rule.source_line)?)?;
    if targets.is_empty() || targets.len() > MAX_WORDS {
        return Err("Make rule requires a bounded nonempty target set".into());
    }
    let mut seen = BTreeSet::new();
    if targets
        .iter()
        .any(|target| target.contains('%') || !seen.insert(target))
    {
        return Err("Make rule has an implicit pattern or duplicate concrete target".into());
    }
    let pattern = rule
        .target_pattern
        .as_ref()
        .map(|raw| evaluate(raw, rule.source_line))
        .transpose()?;
    let pattern_parts = pattern
        .as_ref()
        .map(|value| {
            let pattern = words(value)?;
            if pattern.len() != 1 || rule.double_colon {
                return Err("Make static pattern is ambiguous or double-colon".into());
            }
            let pattern = &pattern[0];
            if pattern.matches('%').count() != 1 {
                return Err("Make static target pattern requires exactly one percent".into());
            }
            let (prefix, suffix) = pattern.split_once('%').ok_or("missing Make percent")?;
            Ok::<_, String>((prefix.to_owned(), suffix.to_owned()))
        })
        .transpose()?;
    let prerequisites = words(&evaluate(&rule.prerequisites, rule.source_line)?)?;
    let order_only = words(&evaluate(&rule.order_only, rule.source_line)?)?;
    let mut retained_bytes = 0usize;
    let mut instances = Vec::with_capacity(targets.len());
    for target in targets {
        let stem = pattern_parts
            .as_ref()
            .map(|(prefix, suffix)| {
                target
                    .strip_prefix(prefix)
                    .and_then(|tail| tail.strip_suffix(suffix))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        "Make concrete target does not match its static pattern".to_owned()
                    })
            })
            .transpose()?;
        let prerequisites = substitute_prerequisites(&prerequisites, stem.as_deref())?;
        let order_only = substitute_prerequisites(&order_only, stem.as_deref())?;
        let recipe_bytes = rule
            .recipes
            .iter()
            .map(|recipe| recipe.text.len() + std::mem::size_of::<SourceRecipe>())
            .sum::<usize>();
        retained_bytes = retained_bytes
            .checked_add(target.len())
            .and_then(|bytes| bytes.checked_add(stem.as_ref().map_or(0, String::len)))
            .and_then(|bytes| {
                bytes.checked_add(
                    prerequisites
                        .iter()
                        .map(|word| word.len() + std::mem::size_of::<String>())
                        .sum::<usize>(),
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    order_only
                        .iter()
                        .map(|word| word.len() + std::mem::size_of::<String>())
                        .sum::<usize>(),
                )
            })
            .and_then(|bytes| bytes.checked_add(recipe_bytes))
            .filter(|bytes| *bytes <= MAX_INSTANCE_BYTES)
            .ok_or("Make rule instances exceed retained byte limit")?;
        retained_bytes = retained_bytes
            .checked_add(
                prerequisites
                    .iter()
                    .chain(&order_only)
                    .map(|word| word.len() + std::mem::size_of::<String>())
                    .sum::<usize>(),
            )
            .filter(|bytes| *bytes <= MAX_INSTANCE_BYTES)
            .ok_or("Make rule instances exceed retained byte limit")?;
        let prerequisite_order = prerequisites
            .iter()
            .chain(&order_only)
            .cloned()
            .collect::<Vec<_>>();
        instances.push(RuleInstance {
            output_line: rule.output_line,
            source_line: rule.source_line,
            target,
            stem,
            prerequisites,
            order_only,
            prerequisite_order,
            double_colon: rule.double_colon,
            recipes: rule.recipes.clone(),
        });
    }
    Ok(instances)
}

/// Instantiate matching implicit-pattern candidates from one frozen source rule.
///
/// This is candidate data, NOT GNU's implicit-rule search or a winning-rule
/// selection. Preserve candidate order, prerequisite repetitions and recipe
/// provenance; callers must separately resolve competing/cancelled rules,
/// prerequisites, policies, timestamps and the actual native invocation phase.
/// Recipe-less matches remain visible rather than being silently discarded.
///
/// This bounded mode admits same-directory names only. GNU's directory removal
/// and reconstruction for slashless patterns must not be approximated by static
/// pattern substitution. No source or recipe expressions are expanded here.
///
/// # Errors
/// Refuses nonliteral targets, mixed/static/double-colon declarations, unsupported
/// pattern/directory syntax, malformed words and the ordinary instance limits.
pub fn instantiate_implicit_candidate(
    rule: &SourceRule,
    target: &str,
) -> Result<Vec<RuleInstance>, String> {
    let targets = words(target)?;
    if targets.len() != 1 || targets[0] != target || targets[0].contains(['%', '/']) {
        return Err("Make implicit candidate requires one same-directory literal target".into());
    }
    if rule.target_pattern.is_some() || rule.double_colon {
        return Err("Make implicit candidate cannot use static or double-colon syntax".into());
    }
    let patterns = words(&rule.targets)?;
    if patterns.is_empty()
        || patterns
            .iter()
            .any(|pattern| pattern.matches('%').count() != 1 || pattern.contains('/'))
    {
        return Err("Make implicit candidate requires same-directory percent patterns".into());
    }
    let mut candidates = Vec::new();
    let mut retained_bytes = 0usize;
    for pattern in patterns {
        let (prefix, suffix) = pattern.split_once('%').ok_or("missing Make percent")?;
        let Some(stem) = targets[0]
            .strip_prefix(prefix)
            .and_then(|tail| tail.strip_suffix(suffix))
            .filter(|stem| !stem.is_empty())
        else {
            continue;
        };
        // Reuse finite instantiation, without representing this candidate as an
        // admitted explicit declaration. Its known stem has identical bounded
        // substitution semantics in this same-directory mode.
        let frozen = SourceRule {
            output_line: rule.output_line,
            source_line: rule.source_line,
            targets: targets[0].clone(),
            target_pattern: Some(pattern.clone()),
            prerequisites: rule.prerequisites.clone(),
            order_only: rule.order_only.clone(),
            double_colon: rule.double_colon,
            recipes: rule.recipes.clone(),
        };
        let mut instances = instantiate(&frozen, &|literal, _| Ok(literal.to_owned()))?;
        let instance = instances
            .pop()
            .ok_or("Make implicit candidate lost its instance")?;
        if !instances.is_empty() || instance.stem.as_deref() != Some(stem) {
            return Err("Make implicit candidate has inconsistent stem or count".into());
        }
        retained_bytes = retained_bytes
            .checked_add(std::mem::size_of::<RuleInstance>())
            .and_then(|bytes| bytes.checked_add(instance.target.len()))
            .and_then(|bytes| bytes.checked_add(stem.len()))
            .and_then(|bytes| {
                bytes.checked_add(
                    instance
                        .recipes
                        .iter()
                        .map(|recipe| recipe.text.len() + std::mem::size_of::<SourceRecipe>())
                        .sum::<usize>(),
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    instance
                        .prerequisites
                        .iter()
                        .chain(&instance.order_only)
                        .chain(&instance.prerequisite_order)
                        .map(|word| word.len() + std::mem::size_of::<String>())
                        .sum::<usize>(),
                )
            })
            .filter(|bytes| *bytes <= MAX_INSTANCE_BYTES)
            .ok_or("Make implicit candidates exceed retained byte limit")?;
        if candidates.len() == MAX_RULES {
            return Err("Make implicit candidates exceed count limit".into());
        }
        candidates.push(instance);
    }
    Ok(candidates)
}

fn words(value: &str) -> Result<Vec<String>, String> {
    if value.len() > MAX_VALUE_BYTES
        || value.chars().any(|character| {
            character.is_control() && !matches!(character, ' ' | '\t' | '\n' | '\r')
        })
    {
        return Err("Make rule expression exceeds byte limit or has control characters".into());
    }
    let mut words = Vec::new();
    for word in value.split_whitespace() {
        if words.len() == MAX_WORDS || word.contains(['$', ':', ';', '|', '=', '\\']) {
            return Err("Make rule expression is unresolved, unsafe or exceeds word limit".into());
        }
        words.push(word.to_owned());
    }
    Ok(words)
}

fn substitute_prerequisites(words: &[String], stem: Option<&str>) -> Result<Vec<String>, String> {
    let mut result = Vec::with_capacity(words.len());
    let mut emitted_bytes = 0usize;
    for word in words {
        let expanded_len = match stem {
            Some(stem) if word.contains('%') => {
                word.len().saturating_sub(1).saturating_add(stem.len())
            }
            _ => word.len(),
        };
        emitted_bytes = emitted_bytes
            .checked_add(expanded_len)
            .filter(|bytes| *bytes <= MAX_VALUE_BYTES)
            .ok_or("Make static prerequisite expansion exceeds value byte limit")?;
        let value = match stem {
            Some(stem) if word.contains('%') => {
                if word.matches('%').count() != 1 {
                    return Err("Make static prerequisite has ambiguous percent pattern".into());
                }
                word.replacen('%', stem, 1)
            }
            _ => word.clone(),
        };
        result.push(value);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "make_rule_events_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "make_implicit_candidate_tests.rs"]
mod implicit_candidate_tests;
