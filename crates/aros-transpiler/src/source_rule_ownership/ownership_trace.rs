//! Owner tracing and diagnostic budgeting over the source graph.

use super::{
    pattern_stem, safe_identity, CompileMultiGroup, SourceGraph, SourceRuleOwnership,
    MAX_DIAGNOSTIC_PATH_BYTES, MAX_DIAGNOSTIC_WORK, MAX_IDENTITIES, MAX_LINES,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Default)]
pub(super) struct DiagnosticBudget {
    pub(super) work: usize,
    pub(super) retained_path_bytes: usize,
}

impl DiagnosticBudget {
    pub(super) const fn charge_work(&mut self, amount: usize) -> bool {
        let Some(total) = self.work.checked_add(amount) else {
            return false;
        };
        if total > MAX_DIAGNOSTIC_WORK {
            return false;
        }
        self.work = total;
        true
    }

    pub(super) const fn charge_path_bytes(&mut self, amount: usize) -> bool {
        let Some(total) = self.retained_path_bytes.checked_add(amount) else {
            return false;
        };
        if total > MAX_DIAGNOSTIC_PATH_BYTES {
            return false;
        }
        self.retained_path_bytes = total;
        true
    }
}

pub(super) fn attribute_graph_outputs(
    graph: &SourceGraph,
    rejected_rule_line: usize,
    outputs: &[String],
) -> Option<Vec<SourceRuleOwnership>> {
    let mut budget = DiagnosticBudget::default();
    trace_all_then_paired(graph, outputs, rejected_rule_line, &mut budget)
}

pub(super) fn trace_all_then_paired(
    graph: &SourceGraph,
    outputs: &[String],
    rejected_rule_line: usize,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    trace_all_targets(graph, outputs, budget)
        .or_else(|| trace_paired_compile_outputs(graph, outputs, rejected_rule_line, budget))
}

fn trace_all_targets(
    graph: &SourceGraph,
    outputs: &[String],
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    let mut owners = BTreeMap::<String, SourceRuleOwnership>::new();
    for output in outputs {
        for proof in trace_all_owners(graph, output, budget)? {
            merge_ownership(&mut owners, proof, budget)?;
        }
    }
    (!owners.is_empty()).then(|| owners.into_values().collect())
}

fn merge_ownership(
    owners: &mut BTreeMap<String, SourceRuleOwnership>,
    proof: SourceRuleOwnership,
    budget: &mut DiagnosticBudget,
) -> Option<()> {
    if owners.contains_key(&proof.owner) {
        return Some(());
    }
    let key_bytes = std::mem::size_of::<String>().checked_add(proof.owner.len())?;
    if !budget.charge_path_bytes(key_bytes) {
        return None;
    }
    let owner_key = clone_path_string(&proof.owner)?;
    owners.insert(owner_key, proof);
    Some(())
}

pub(super) fn trace_all_owners(
    graph: &SourceGraph,
    output: &str,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    let start = graph.identities.get(output)?.as_str();
    let mut queue = VecDeque::new();
    queue.try_reserve(1).ok()?;
    queue.push_back(start);

    // BFS first discovery is the shortest path. Store one predecessor per
    // vertex instead of cloning every path prefix at every edge.
    let mut predecessors = BTreeMap::<&str, Option<&str>>::new();
    let mut indegree = BTreeMap::<&str, usize>::new();
    predecessors.insert(start, None);
    indegree.insert(start, 0);
    let mut candidates = BTreeSet::<&str>::new();

    while let Some(identity) = queue.pop_front() {
        if !budget.charge_work(1) {
            return None;
        }
        let is_owner = graph.owners.contains(identity);
        if is_owner {
            candidates.insert(identity);
        }
        let mut has_successor = false;
        let mut degree_overflow = false;
        if !visit_sorted_successors(graph, identity, budget, |consumer| {
            has_successor = true;
            let Some(next_degree) = indegree.get(consumer).copied().unwrap_or(0).checked_add(1)
            else {
                degree_overflow = true;
                return false;
            };
            indegree.insert(consumer, next_degree);
            if !predecessors.contains_key(consumer) {
                if predecessors.len() >= MAX_IDENTITIES {
                    return false;
                }
                predecessors.insert(consumer, Some(identity));
                if queue.try_reserve(1).is_err() {
                    return false;
                }
                queue.push_back(consumer);
            }
            true
        }) || degree_overflow
        {
            return None;
        }
        if !has_successor && !is_owner {
            // A known owner on one branch cannot hide a separate, unowned
            // consumer. Complete attribution requires every reachable branch
            // to terminate at a source-proven owner.
            return None;
        }
    }

    // Re-enumerate the borrowed edges for Kahn's cycle check. No cloned
    // adjacency or retained path vectors are needed.
    let mut ready = VecDeque::new();
    ready.try_reserve_exact(indegree.len()).ok()?;
    for (identity, degree) in &indegree {
        if *degree == 0 {
            ready.push_back(*identity);
        }
    }
    let mut visited = 0usize;
    while let Some(identity) = ready.pop_front() {
        if !budget.charge_work(1) {
            return None;
        }
        visited += 1;
        let mut invalid_indegree = false;
        if !visit_sorted_successors(graph, identity, budget, |consumer| {
            let Some(degree) = indegree.get_mut(consumer) else {
                invalid_indegree = true;
                return false;
            };
            if *degree == 0 {
                invalid_indegree = true;
                return false;
            }
            *degree -= 1;
            if *degree == 0 {
                if ready.try_reserve(1).is_err() {
                    return false;
                }
                ready.push_back(consumer);
            }
            true
        }) || invalid_indegree
        {
            return None;
        }
    }
    if visited != indegree.len() || candidates.is_empty() {
        return None;
    }

    let mut proofs = Vec::new();
    proofs.try_reserve_exact(candidates.len()).ok()?;
    for owner in candidates {
        proofs.push(materialize_ownership_path(
            &predecessors,
            start,
            owner,
            budget,
        )?);
    }
    Some(proofs)
}

fn visit_sorted_successors<'a>(
    graph: &'a SourceGraph,
    identity: &str,
    budget: &mut DiagnosticBudget,
    mut visit: impl FnMut(&'a str) -> bool,
) -> bool {
    let mut consumers = graph
        .consumers
        .get(identity)
        .into_iter()
        .flat_map(|set| set.iter())
        .peekable();
    let mut macro_owners = graph
        .macro_owners
        .get(identity)
        .into_iter()
        .flat_map(|set| set.iter())
        .peekable();

    loop {
        let ordering = match (consumers.peek(), macro_owners.peek()) {
            (None, None) => return true,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => left.as_str().cmp(right.as_str()),
        };
        let next = match ordering {
            std::cmp::Ordering::Less => consumers.next(),
            std::cmp::Ordering::Greater => macro_owners.next(),
            std::cmp::Ordering::Equal => {
                let left = consumers.next();
                let _ = macro_owners.next();
                left
            }
        };
        let Some(next) = next else {
            return true;
        };
        if !budget.charge_work(1) || !visit(next.as_str()) {
            return false;
        }
    }
}

fn materialize_ownership_path(
    predecessors: &BTreeMap<&str, Option<&str>>,
    root: &str,
    owner: &str,
    budget: &mut DiagnosticBudget,
) -> Option<SourceRuleOwnership> {
    let mut path_len = 0usize;
    let mut path_bytes = 0usize;
    let mut cursor = owner;
    loop {
        if !budget.charge_work(1) {
            return None;
        }
        path_len = path_len.checked_add(1)?;
        path_bytes = path_bytes.checked_add(cursor.len())?;
        match predecessors.get(cursor)? {
            Some(parent) => cursor = parent,
            None if cursor == root => break,
            None => return None,
        }
    }
    let retained_bytes = std::mem::size_of::<SourceRuleOwnership>()
        .checked_add(owner.len())?
        .checked_add(path_len.checked_mul(std::mem::size_of::<String>())?)?
        .checked_add(path_bytes)?;
    if !budget.charge_path_bytes(retained_bytes) {
        return None;
    }

    let mut chain = Vec::new();
    chain.try_reserve_exact(path_len).ok()?;
    cursor = owner;
    loop {
        if !budget.charge_work(1) {
            return None;
        }
        chain.push(clone_path_string(cursor)?);
        match predecessors.get(cursor)? {
            Some(parent) => cursor = parent,
            None => break,
        }
    }
    chain.reverse();
    Some(SourceRuleOwnership {
        owner: clone_path_string(owner)?,
        chain,
    })
}

fn clone_path_string(value: &str) -> Option<String> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len()).ok()?;
    owned.push_str(value);
    Some(owned)
}

fn trace_paired_compile_outputs(
    graph: &SourceGraph,
    rejected_outputs: &[String],
    rejected_rule_line: usize,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    if rejected_outputs.len() < 2 || graph.recipe_rule_lines.contains(&rejected_rule_line) {
        return None;
    }
    let mut rejected_set = BTreeSet::<&str>::new();
    for output in rejected_outputs {
        if !budget.charge_work(1) || !rejected_set.insert(output.as_str()) {
            return None;
        }
    }

    // Build bounded indexes once. Re-scanning every invocation for each output
    // turns a finite snapshot into a groups × outputs walk.
    let mut groups_by_outputs = BTreeMap::<Vec<&str>, Vec<usize>>::new();
    let mut output_producers = BTreeMap::<&str, usize>::new();
    for (group_index, group) in graph.compile_multi_groups.iter().enumerate() {
        let output_count = group.pairs.len().checked_mul(2)?;
        if !budget.charge_work(output_count) {
            return None;
        }
        let group_outputs = compile_multi_group_outputs(group)?;
        for output in &group_outputs {
            let count = output_producers.entry(*output).or_default();
            *count = count.checked_add(1)?;
        }
        let mut output_key = Vec::new();
        output_key.try_reserve_exact(group_outputs.len()).ok()?;
        output_key.extend(group_outputs);
        let matching_groups = groups_by_outputs.entry(output_key).or_default();
        matching_groups.try_reserve(1).ok()?;
        matching_groups.push(group_index);
    }
    let mut rejected_key = Vec::new();
    rejected_key.try_reserve_exact(rejected_set.len()).ok()?;
    rejected_key.extend(rejected_set.iter().copied());
    let [group_index] = groups_by_outputs.get(&rejected_key)?.as_slice() else {
        return None;
    };
    let group = graph.compile_multi_groups.get(*group_index)?;
    if group.pairs.is_empty() {
        return None;
    }

    let group_outputs = compile_multi_group_outputs(group)?;
    let mut ordinary_targets = BTreeSet::<&str>::new();
    for (line, targets) in &graph.targets_by_line {
        if *line == rejected_rule_line {
            continue;
        }
        for target in targets {
            if !budget.charge_work(1) {
                return None;
            }
            ordinary_targets.insert(target.as_str());
        }
    }
    for output in &group_outputs {
        if !graph.macro_outputs.contains(*output)
            || graph.ambiguous_macro_outputs.contains(*output)
            || graph.macro_owners.contains_key(*output)
            || ordinary_targets.contains(*output)
            || output_producers.get(*output) != Some(&1)
        {
            return None;
        }
        for pattern in &graph.patterns {
            if !budget.charge_work(1) {
                return None;
            }
            if pattern_stem(&pattern.target, output).is_some() {
                return None;
            }
        }
    }

    // Keep the object's real consumer chain as the proof for an otherwise
    // ownerless sidecar; never invent a Make edge from `.d` to `.o`.
    let mut owners = BTreeMap::<String, SourceRuleOwnership>::new();
    for pair in &group.pairs {
        let object_owners = trace_all_owners(graph, &pair.object, budget)?;
        for proof in object_owners {
            merge_ownership(&mut owners, proof, budget)?;
        }

        // This detects source graph consumers/owners only. A `%include_deps`
        // invocation consumes generated text, but its runtime-expanded
        // prerequisites are not part of this source snapshot or this proof.
        let depfile_has_source_consumers = graph
            .consumers
            .get(&pair.depfile)
            .is_some_and(|consumers| !consumers.is_empty())
            || graph.owners.contains(&pair.depfile)
            || graph.macro_owners.contains_key(&pair.depfile);
        if depfile_has_source_consumers {
            for proof in trace_all_owners(graph, &pair.depfile, budget)? {
                merge_ownership(&mut owners, proof, budget)?;
            }
        }
    }
    (!owners.is_empty()).then(|| owners.into_values().collect())
}

fn compile_multi_group_outputs(group: &CompileMultiGroup) -> Option<BTreeSet<&str>> {
    if group.pairs.is_empty() || group.invocation_line >= MAX_LINES {
        return None;
    }
    let mut outputs = BTreeSet::new();
    for pair in &group.pairs {
        if !safe_identity(&pair.object)
            || !safe_identity(&pair.depfile)
            || !outputs.insert(pair.object.as_str())
            || !outputs.insert(pair.depfile.as_str())
        {
            return None;
        }
    }
    Some(outputs)
}
