//! Exact SDK file producers and dependencies for finite source-owned assets.

use super::{arch_compatible, arch_of, DependencyGraph};
use crate::sdk_asset_rules::SdkAssetOperation;
use crate::TargetContext;
use std::collections::{BTreeMap, BTreeSet};

/// A program declaration whose explicit output directory is the configured SDK bin root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkProgramOutput {
    pub owner: String,
    pub output: String,
    pub directory: std::path::PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk_asset_rules::{SdkAssetOperationDecl, SdkAssetRuleDecl};

    fn copy(owner: &str, input: &str, output: &str) -> SdkAssetRuleDecl {
        SdkAssetRuleDecl {
            owner: owner.into(),
            file: "external/example/mmakefile.src".into(),
            line: 1,
            operations: vec![SdkAssetOperationDecl {
                line: 2,
                operation: SdkAssetOperation::Copy {
                    input: format!("${{AROS_DEVELOPER_BIN_DIR}}/{input}"),
                    output: format!("${{AROS_DEVELOPER_BIN_DIR}}/{output}"),
                },
            }],
        }
    }

    fn program(owner: &str, file: &str) -> SdkProgramOutput {
        SdkProgramOutput {
            owner: owner.into(),
            output: format!("${{AROS_DEVELOPER_BIN_DIR}}/{file}"),
            directory: "external/example".into(),
        }
    }

    #[test]
    fn sdk_alias_binds_the_exact_file_producer_and_chained_aggregate() {
        let mut graph = DependencyGraph::new();
        graph
            .sdk_program_outputs
            .push(program("compiler-example", "example"));
        graph.sdk_asset_rules = vec![
            copy("aliases", "example", "alias"),
            copy("aliases-next", "alias", "next"),
        ];
        assert_eq!(
            graph.sdk_asset_dependencies("aliases", None).unwrap(),
            BTreeSet::from(["compiler-example".into()])
        );
        assert_eq!(
            graph.sdk_asset_dependencies("aliases-next", None).unwrap(),
            BTreeSet::from(["aliases".into()])
        );
    }

    #[test]
    fn sdk_alias_refuses_absent_ambiguous_and_self_producers() {
        let mut graph = DependencyGraph::new();
        graph
            .sdk_asset_rules
            .push(copy("aliases", "example", "alias"));
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
        graph.sdk_program_outputs = vec![program("first", "example"), program("second", "example")];
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
        graph.sdk_program_outputs = vec![program("aliases", "example")];
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
    }

    #[test]
    fn sdk_alias_refuses_case_collisions_and_multiple_aggregate_declarations() {
        let mut graph = DependencyGraph::new();
        graph
            .sdk_program_outputs
            .push(program("compiler-example", "example"));
        graph
            .sdk_asset_rules
            .push(copy("aliases", "example", "alias"));
        graph.sdk_program_outputs.push(program("other", "Alias"));
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
        graph.sdk_program_outputs.pop();
        graph
            .sdk_asset_rules
            .push(copy("aliases", "example", "other"));
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
    }

    #[test]
    fn sdk_alias_resolves_only_programs_for_the_selected_architecture() {
        let context = TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            ..TargetContext::default()
        };
        let mut graph = DependencyGraph::new();
        graph
            .sdk_asset_rules
            .push(copy("aliases", "example", "alias"));
        let mut foreign = program("foreign", "example");
        foreign.directory = "arch/x86_64-pc/example".into();
        graph.sdk_program_outputs.push(foreign);
        assert!(graph
            .sdk_asset_dependencies("aliases", Some(&context))
            .is_err());
        let mut applicable = program("applicable", "example");
        applicable.directory = "arch/riscv-esp32p4/example".into();
        graph.sdk_program_outputs.push(applicable);
        assert_eq!(
            graph
                .sdk_asset_dependencies("aliases", Some(&context))
                .unwrap(),
            BTreeSet::from(["applicable".into()])
        );
        assert!(graph.sdk_asset_dependencies("aliases", None).is_err());
    }
}

#[cfg(test)]
#[test]
fn sdk_alias_rejects_file_dependency_cycles_before_generation() {
    use crate::sdk_asset_rules::{SdkAssetOperationDecl, SdkAssetRuleDecl};
    let mut graph = DependencyGraph::new();
    for (owner, input, output) in [("first", "b", "a"), ("second", "a", "b")] {
        graph.sdk_asset_rules.push(SdkAssetRuleDecl {
            owner: owner.into(),
            file: "fixture/mmakefile.src".into(),
            line: 1,
            operations: vec![SdkAssetOperationDecl {
                line: 2,
                operation: SdkAssetOperation::Copy {
                    input: format!("${{AROS_DEVELOPER_BIN_DIR}}/{input}"),
                    output: format!("${{AROS_DEVELOPER_BIN_DIR}}/{output}"),
                },
            }],
        });
    }
    assert!(graph
        .sdk_asset_dependencies("first", None)
        .unwrap_err()
        .contains("cyclic"));
    assert!(graph
        .sdk_asset_dependencies("second", None)
        .unwrap_err()
        .contains("cyclic"));
}

impl DependencyGraph {
    /// Source-declared file identity -> every concrete producer. Keeping all
    /// owners makes ambiguity an error rather than choosing by iteration order.
    pub(crate) fn sdk_asset_producers(
        &self,
        context: Option<&TargetContext>,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut add = |output: &str, owner: &str| {
            result
                .entry(output.to_ascii_lowercase())
                .or_default()
                .insert(owner.into());
        };
        for program in &self.sdk_program_outputs {
            if !Self::sdk_program_applicable(program, context) {
                continue;
            }
            add(&program.output, &program.owner);
        }
        for copy in &self.sdk_file_copies {
            for file in &copy.files {
                add(&format!("{}/{file}", copy.destination), &copy.owner);
            }
        }
        for rule in &self.sdk_asset_rules {
            for operation in &rule.operations {
                let output = match &operation.operation {
                    SdkAssetOperation::Copy { output, .. }
                    | SdkAssetOperation::WriteText { output, .. } => output,
                };
                add(output, &rule.owner);
            }
        }
        result
    }

    pub(crate) fn sdk_program_applicable(
        program: &SdkProgramOutput,
        context: Option<&TargetContext>,
    ) -> bool {
        context.is_none_or(|context| {
            let arch = context
                .cpu
                .as_ref()
                .zip(context.platform.as_ref())
                .map(|(cpu, platform)| (cpu.clone(), platform.clone()));
            arch_compatible(arch_of(&program.directory).as_ref(), arch.as_ref())
        })
    }

    pub(crate) fn sdk_asset_dependencies(
        &self,
        owner: &str,
        context: Option<&TargetContext>,
    ) -> Result<BTreeSet<String>, String> {
        let dependencies = self.sdk_asset_direct_dependencies(owner, context)?;
        let asset_owners: BTreeSet<_> = self
            .sdk_asset_rules
            .iter()
            .map(|rule| rule.owner.as_str())
            .collect();
        let mut pending = vec![(owner.to_owned(), BTreeSet::new())];
        let mut expanded = BTreeSet::new();
        while let Some((current, mut ancestors)) = pending.pop() {
            if !ancestors.insert(current.clone()) {
                return Err(format!(
                    "SDK asset {owner}: cyclic concrete file dependencies at {current}"
                ));
            }
            if !expanded.insert(current.clone()) {
                continue;
            }
            for dependency in self.sdk_asset_direct_dependencies(&current, context)? {
                if asset_owners.contains(dependency.as_str()) {
                    pending.push((dependency, ancestors.clone()));
                }
            }
        }
        Ok(dependencies)
    }

    fn sdk_asset_direct_dependencies(
        &self,
        owner: &str,
        context: Option<&TargetContext>,
    ) -> Result<BTreeSet<String>, String> {
        let producers = self.sdk_asset_producers(context);
        let rules: Vec<_> = self
            .sdk_asset_rules
            .iter()
            .filter(|rule| rule.owner == owner)
            .collect();
        if rules.len() != 1 {
            return Err(format!(
                "SDK asset {owner} requires one source-owned aggregate"
            ));
        }
        let mut dependencies = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for operation in &rules[0].operations {
            let output = match &operation.operation {
                SdkAssetOperation::Copy { output, .. }
                | SdkAssetOperation::WriteText { output, .. } => output,
            };
            let key = output.to_ascii_lowercase();
            if !seen.insert(key.clone())
                || producers
                    .get(&key)
                    .is_none_or(|owners| owners.len() != 1 || !owners.contains(owner))
            {
                return Err(format!(
                    "SDK asset {owner}: output {output} has competing producers"
                ));
            }
            if let SdkAssetOperation::Copy { input, .. } = &operation.operation {
                let owners = producers.get(&input.to_ascii_lowercase());
                if owners.is_none_or(|owners| owners.len() != 1 || owners.contains(owner)) {
                    return Err(format!(
                        "SDK asset {owner}: input {input} requires one distinct concrete producer"
                    ));
                }
                dependencies.extend(owners.unwrap().iter().cloned());
            }
        }
        Ok(dependencies)
    }
}
