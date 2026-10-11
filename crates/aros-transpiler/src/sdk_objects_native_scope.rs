//! Explicit native configuration for physical SDK compile/stage owners.

use super::{collect_from_snapshot, SdkObjectGroupDecl, SdkObjectRejection};
use crate::dirs::DirVars;
use crate::make_vars::{collect_vars_impl, ConditionalTruth, VarScope};
use crate::parser::{macro_invocations, Invocation, TargetContext};
use std::path::Path;

/// Borrowed legacy scope and its independent physical-source positions.
#[derive(Clone, Copy)]
pub struct SdkObjectSnapshot<'a> {
    pub(crate) invocations: &'a [Invocation],
    pub(crate) scope: &'a VarScope,
    pub(crate) line_states: Option<&'a [ConditionalTruth]>,
    pub(crate) text: &'a str,
    /// For each physical line, its exact slot in the legacy joined scope.
    pub(crate) physical_positions: Option<&'a [Option<usize>]>,
}

/// Project physical SDK owners through an explicit native configuration.
pub fn collect_with_native_configuration(
    legacy: SdkObjectSnapshot<'_>,
    physical_source: &str,
    target: Option<&TargetContext>,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> (Vec<SdkObjectGroupDecl>, Vec<SdkObjectRejection>) {
    let baseline = collect_from_snapshot(
        legacy.invocations,
        legacy.scope,
        dirs,
        root,
        rel_dir,
        legacy.line_states,
        legacy.text,
    );
    let Some(target) = target.filter(|target| {
        !target.make_include_bindings.is_empty() || !target.generated_make_templates.is_empty()
    }) else {
        return baseline;
    };
    // A native-only object list can use any source variable name. Do not
    // require the legacy collector to recognize its unbound spelling first.
    // Includes themselves still cannot supply compile/stage/aggregate owners.
    let physical_invocations =
        macro_invocations(&crate::parser::join_continuations(physical_source));
    let physical_compile = physical_invocations.iter().any(|invocation| {
        matches!(
            invocation.name.as_str(),
            "rule_compile" | "rule_compile_cxx"
        )
    });
    if baseline.0.is_empty() && baseline.1.is_empty() && !physical_compile {
        return baseline;
    }
    let recipe = rel_dir.join("mmakefile.src");
    let native = match crate::assembly_headers::native_configuration_snapshot(
        physical_source,
        target,
        dirs,
        root,
        &recipe,
    ) {
        Ok(native) => native,
        Err(reason) => {
            let owners = legacy_owners(&legacy, physical_source);
            return reject_native_scope(baseline, &reason, &owners);
        }
    };
    // Includes supply values, never physical compile/stage/aggregate owners.
    // Empty slots preserve declaration-time scope coordinates.
    let mut rule_view = String::with_capacity(native.joined.len());
    for (index, line) in native.joined.lines().enumerate() {
        if native
            .physical_owner_lines
            .get(index)
            .copied()
            .flatten()
            .is_some()
        {
            rule_view.push_str(line);
        }
        rule_view.push('\n');
    }
    let (scope, states) = collect_vars_impl(&native.joined, Some(target));
    let invocations = macro_invocations(&rule_view);
    let (groups, mut rejections) = collect_from_snapshot(
        &invocations,
        &scope,
        dirs,
        root,
        rel_dir,
        Some(&states),
        &rule_view,
    );
    let mut admitted = Vec::with_capacity(groups.len());
    for mut group in groups {
        let physical_group = physical_line(&native.physical_owner_lines, group.line);
        let physical_objects = group
            .objects
            .iter()
            .map(|object| physical_line(&native.physical_owner_lines, object.line))
            .collect::<Option<Vec<_>>>();
        let (Some(group_line), Some(object_lines)) = (physical_group, physical_objects) else {
            rejections.push(SdkObjectRejection {
                owner: group.owner,
                file: group.file,
                line: group.line,
                reason: "native SDK declaration has no physical recipe owner; fix upstream".into(),
            });
            continue;
        };
        group.line = group_line;
        for (object, line) in group.objects.iter_mut().zip(object_lines) {
            object.line = line;
        }
        admitted.push(group);
    }
    for rejection in &mut rejections {
        if let Some(line) = physical_line(&native.physical_owner_lines, rejection.line) {
            rejection.line = line;
        } else {
            rejection.reason = format!(
                "{}; native scope location has no physical recipe owner",
                rejection.reason
            );
            rejection.line = 1;
        }
    }
    (admitted, rejections)
}

fn physical_line(owners: &[Option<usize>], line: usize) -> Option<usize> {
    owners
        .get(line.checked_sub(1)?)
        .copied()
        .flatten()?
        .checked_add(1)
}

fn reject_native_scope(
    baseline: (Vec<SdkObjectGroupDecl>, Vec<SdkObjectRejection>),
    reason: &str,
    owners: &[Option<usize>],
) -> (Vec<SdkObjectGroupDecl>, Vec<SdkObjectRejection>) {
    let (groups, mut rejections) = baseline;
    for group in groups {
        rejections.push(SdkObjectRejection {
            owner: group.owner,
            file: group.file,
            line: group.line,
            reason: String::new(),
        });
    }
    for rejection in &mut rejections {
        rejection.reason = format!("native SDK configuration is not proved: {reason}");
        rejection.line = physical_line(owners, rejection.line).unwrap_or(1);
    }
    (Vec::new(), rejections)
}

fn legacy_owners(legacy: &SdkObjectSnapshot<'_>, physical_source: &str) -> Vec<Option<usize>> {
    let mut owners = vec![None; legacy.text.lines().count()];
    if let Some(positions) = legacy.physical_positions {
        for (physical, position) in positions.iter().copied().enumerate() {
            if let Some(slot) = position.and_then(|position| owners.get_mut(position)) {
                *slot = Some(physical);
            }
        }
    } else {
        let (joined, physical) = crate::parser::join_continuations_with_origins(physical_source);
        if joined == legacy.text {
            for (slot, physical) in owners.iter_mut().zip(physical) {
                *slot = Some(physical);
            }
        }
    }
    owners
}

#[cfg(test)]
#[path = "sdk_objects_native_scope_tests.rs"]
mod tests;
