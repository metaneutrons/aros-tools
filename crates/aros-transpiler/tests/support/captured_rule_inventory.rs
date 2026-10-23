//! Private source-rule metadata for the captured Make read experiment.
//!
//! This is an inventory of interpreted input data. It does not select native
//! rules, authorize recipes, or qualify an SDK.

use aros_common::{
    make_startup_arguments::LiteralMakeStartupArguments, sha256_bytes, Sha256Digest,
};
use aros_transpiler::{
    make_rule_header::RuleHeader,
    make_vars::{MakeFileEventKind, MakeFileProgram},
};
use serde_json::{json, Value};
use std::path::Path;

const MAX_EVENTS: usize = 65_536;
const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
const ESCAPED_BYTES_PER_SOURCE_BYTE: usize = 6;
const ROOT_METADATA_BYTES: usize = 1_024;
const RULE_METADATA_BYTES: usize = 512;
const POLICY_METADATA_BYTES: usize = 512;
const GOAL_METADATA_BYTES: usize = 16;
const POLICY_NAME_METADATA_BYTES: usize = 16;

/// Retain active rule and selection-policy metadata from one already-read program.
///
/// The startup goals come from the caller's admitted argument object. This
/// function does not parse arguments again and never retains recipe text.
pub fn retain(
    output: &Path,
    file: &MakeFileProgram,
    startup: &LiteralMakeStartupArguments,
    read_records_sha256: &Sha256Digest,
) -> Sha256Digest {
    let bytes = inventory_bytes(
        file,
        startup,
        read_records_sha256,
        MAX_EVENTS,
        MAX_JSON_BYTES,
    )
    .expect("captured source-rule inventory exceeded its bound or was inconsistent");
    super::write_private(&output.join("source-rule-inventory.json"), &bytes);
    sha256_bytes(&bytes)
}

fn inventory_bytes(
    file: &MakeFileProgram,
    startup: &LiteralMakeStartupArguments,
    read_records_sha256: &Sha256Digest,
    max_events: usize,
    max_json_bytes: usize,
) -> Result<Vec<u8>, &'static str> {
    if file.events.len() > max_events {
        return Err("source event count exceeds inventory limit");
    }
    // Reuse the generic source-event association checks, including active owner
    // and input/generated origin binding. Counting must not silently detach a
    // foreign recipe from its ordinary declaration.
    aros_transpiler::make_interpreted_rules::collect(file)
        .map_err(|_| "source inventory rule association is inconsistent")?;

    let mut budget = JsonBudget::new(max_json_bytes);
    budget.charge_metadata(ROOT_METADATA_BYTES)?;
    budget.charge_string("local-v1")?;
    budget.charge_string(read_records_sha256.as_str())?;
    for goal in startup.goals() {
        budget.charge_string(goal)?;
        budget.charge_metadata(GOAL_METADATA_BYTES)?;
    }

    // Event-indexed state makes recipe association linear in the event stream.
    // Recipe payloads are inspected only for their indices and are never copied.
    let event_count = file.events.len();
    let mut is_recipe_owner = vec![false; event_count];
    let mut is_active_recipe_counted = vec![false; event_count];
    let mut tab_recipe_counts = vec![0usize; event_count];
    let mut active_rule_indices = Vec::new();
    let mut active_policy_indices = Vec::new();
    let mut active_default_rule_count = 0usize;
    let mut active_phony_count = 0usize;
    let mut active_suffix_count = 0usize;
    let mut active_search_path_count = 0usize;

    for (event_index, event) in file.events.iter().enumerate() {
        match &event.kind {
            MakeFileEventKind::Rule(RuleHeader::Rule {
                targets,
                target_pattern,
                prerequisites,
                order_only,
                ..
            }) => {
                is_recipe_owner[event_index] = true;
                if event.active {
                    is_active_recipe_counted[event_index] = true;
                    active_rule_indices.push(event_index);
                    budget.charge_metadata(RULE_METADATA_BYTES)?;
                    budget.charge_string(&event.input)?;
                    budget.charge_string(targets)?;
                    if let Some(pattern) = target_pattern {
                        budget.charge_string(pattern)?;
                    }
                    budget.charge_string(prerequisites)?;
                    budget.charge_string(order_only)?;
                }
            }
            MakeFileEventKind::DefaultRule(header) => {
                if matches!(header, RuleHeader::Rule { .. }) {
                    is_recipe_owner[event_index] = true;
                }
                if event.active {
                    let RuleHeader::Rule {
                        targets,
                        target_pattern,
                        prerequisites,
                        order_only,
                        ..
                    } = header
                    else {
                        return Err("active default rule has a non-rule header");
                    };
                    active_default_rule_count += 1;
                    is_active_recipe_counted[event_index] = true;
                    active_policy_indices.push(event_index);
                    budget.charge_metadata(POLICY_METADATA_BYTES)?;
                    budget.charge_string(&event.input)?;
                    budget.charge_string(targets)?;
                    if let Some(pattern) = target_pattern {
                        budget.charge_string(pattern)?;
                    }
                    budget.charge_string(prerequisites)?;
                    budget.charge_string(order_only)?;
                }
            }
            MakeFileEventKind::PhonyTargets(names) if event.active => {
                active_phony_count += 1;
                active_policy_indices.push(event_index);
                budget.charge_metadata(POLICY_METADATA_BYTES)?;
                budget.charge_string(&event.input)?;
                for name in names {
                    budget.charge_string(name)?;
                    budget.charge_metadata(POLICY_NAME_METADATA_BYTES)?;
                }
            }
            MakeFileEventKind::Suffixes(names) if event.active => {
                active_suffix_count += 1;
                active_policy_indices.push(event_index);
                budget.charge_metadata(POLICY_METADATA_BYTES)?;
                budget.charge_string(&event.input)?;
                for name in names {
                    budget.charge_string(name)?;
                    budget.charge_metadata(POLICY_NAME_METADATA_BYTES)?;
                }
            }
            MakeFileEventKind::SearchPath(arguments) if event.active => {
                active_search_path_count += 1;
                active_policy_indices.push(event_index);
                budget.charge_metadata(POLICY_METADATA_BYTES)?;
                budget.charge_string(&event.input)?;
                budget.charge_string(arguments)?;
            }
            MakeFileEventKind::Recipe { rule_event, .. } => {
                if *rule_event >= event_index {
                    return Err("recipe refers to a forward or current event");
                }
                if !is_recipe_owner[*rule_event] {
                    return Err("recipe refers to an event that cannot own a recipe");
                }
                if event.active && is_active_recipe_counted[*rule_event] {
                    tab_recipe_counts[*rule_event] = tab_recipe_counts[*rule_event]
                        .checked_add(1)
                        .ok_or("recipe count overflow")?;
                }
            }
            _ => {}
        }
    }

    // No JSON values or cloned source strings are created until the complete
    // upper-bound estimate has passed.
    let rules = active_rule_indices
        .iter()
        .map(|event_index| {
            let event = &file.events[*event_index];
            let MakeFileEventKind::Rule(RuleHeader::Rule {
                targets,
                target_pattern,
                prerequisites,
                order_only,
                double_colon,
                inline_recipe,
            }) = &event.kind
            else {
                unreachable!("preflight retained only ordinary rule headers");
            };
            json!({
                "event_index": event_index,
                "input": event.input,
                "source_line": event.source_line,
                "generated": event.generated,
                "targets": targets,
                "target_pattern": target_pattern,
                "prerequisites": prerequisites,
                "order_only": order_only,
                "double_colon": double_colon,
                "recipe_count": tab_recipe_counts[*event_index]
                    + usize::from(inline_recipe.is_some()),
            })
        })
        .collect::<Vec<Value>>();
    let policies = active_policy_indices
        .iter()
        .map(|event_index| {
            let event = &file.events[*event_index];
            let origin = json!({
                "event_index": event_index,
                "input": event.input,
                "source_line": event.source_line,
                "generated": event.generated,
            });
            match &event.kind {
                MakeFileEventKind::DefaultRule(RuleHeader::Rule {
                    targets,
                    target_pattern,
                    prerequisites,
                    order_only,
                    double_colon,
                    inline_recipe,
                }) => json!({
                    "event_index": origin["event_index"],
                    "input": origin["input"],
                    "source_line": origin["source_line"],
                    "generated": origin["generated"],
                    "policy": "default_rule",
                    "targets": targets,
                    "target_pattern": target_pattern,
                    "prerequisites": prerequisites,
                    "order_only": order_only,
                    "double_colon": double_colon,
                    "recipe_count": tab_recipe_counts[*event_index]
                        + usize::from(inline_recipe.is_some()),
                }),
                MakeFileEventKind::PhonyTargets(names) => json!({
                    "event_index": origin["event_index"],
                    "input": origin["input"],
                    "source_line": origin["source_line"],
                    "generated": origin["generated"],
                    "policy": "phony_targets",
                    "names": names,
                }),
                MakeFileEventKind::Suffixes(names) => json!({
                    "event_index": origin["event_index"],
                    "input": origin["input"],
                    "source_line": origin["source_line"],
                    "generated": origin["generated"],
                    "policy": "suffixes",
                    "names": names,
                }),
                MakeFileEventKind::SearchPath(arguments) => json!({
                    "event_index": origin["event_index"],
                    "input": origin["input"],
                    "source_line": origin["source_line"],
                    "generated": origin["generated"],
                    "policy": "search_path",
                    "arguments": arguments,
                }),
                _ => unreachable!("preflight retained only active selection policies"),
            }
        })
        .collect::<Vec<Value>>();

    let document = json!({
        "schema": "local-v1",
        "goals": startup.goals(),
        "read_records_sha256": read_records_sha256,
        "active_event_counts": {
            "default_rule": active_default_rule_count,
            "phony_targets": active_phony_count,
            "suffixes": active_suffix_count,
            "search_path": active_search_path_count,
        },
        "rules": rules,
        "policies": policies,
        "sdk_qualification": false,
        "source_recipes_executed": false,
        "native_selection_proven": false,
        "policy_discharged": false,
    });
    serde_json::to_vec_pretty(&document).map_err(|_| "source inventory serialization failed")
}

struct JsonBudget {
    used: usize,
    limit: usize,
}

impl JsonBudget {
    const fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    fn charge_string(&mut self, value: &str) -> Result<(), &'static str> {
        let escaped = value
            .len()
            .checked_mul(ESCAPED_BYTES_PER_SOURCE_BYTE)
            .and_then(|bytes| bytes.checked_add(2))
            .ok_or("source inventory byte estimate overflow")?;
        self.charge_metadata(escaped)
    }

    fn charge_metadata(&mut self, bytes: usize) -> Result<(), &'static str> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|used| *used <= self.limit)
            .ok_or("source inventory exceeds its JSON byte budget")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::inventory_bytes;
    use aros_common::{
        make_startup_arguments::LiteralMakeStartupArguments, sha256_bytes, Sha256Digest,
    };
    use aros_transpiler::{
        dirs::DirVars,
        make_rule_header::RuleHeader,
        make_vars::{
            MakeFileEvent, MakeFileEventKind, MakeFileProgram, MakeProgramPaths,
            MakeVariableProgram,
        },
        parser::TargetContext,
    };
    use serde_json::Value;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    fn startup() -> LiteralMakeStartupArguments {
        LiteralMakeStartupArguments::admit(&[
            "-f".into(),
            "fixture.mk".into(),
            "first-goal".into(),
            "second-goal".into(),
            "ASSIGNMENT=private".into(),
        ])
        .unwrap()
    }

    #[test]
    fn retains_frozen_rule_metadata_with_recipe_counts_and_private_hash_link() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("output");
        fs::create_dir(&output).unwrap();

        let dirs = DirVars::default();
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: Path::new("/synthetic/source"),
            relative_dir: Path::new("unit"),
            top_directory: Path::new("/synthetic/build"),
            working_directory: Path::new("/synthetic/build"),
        };
        let no_include = |_: &str, _: &str| Ok(None);
        let source = concat!(
            "all: dependency ; echo INLINE_RECIPE_PRIVATE\n",
            "\tprintf TAB_RECIPE_PRIVATE\n",
            "static.obj: %.obj: %.src | order-only\n",
            "\tprintf STATIC_RECIPE_PRIVATE\n",
            "%.obj: %.src\n",
            "\tprintf IMPLICIT_RECIPE_PRIVATE\n",
            ".DEFAULT: fallback\n",
            "\tprintf DEFAULT_RECIPE_PRIVATE\n",
            ".PHONY: all\n",
            ".SUFFIXES: .src .obj\n",
            "vpath %.src sources\n",
            "$(eval generated: generated-source)\n",
        );
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        let file = program
            .read_file("fixture.mk", source, &paths, &no_include)
            .unwrap();
        let startup = startup();
        let read_hash = sha256_bytes(b"bound read records");
        let digest = super::retain(&output, &file, &startup, &read_hash);
        let retained_path = output.join("source-rule-inventory.json");
        let retained = fs::read(&retained_path).unwrap();
        assert_eq!(sha256_bytes(&retained), digest);
        assert_eq!(
            fs::metadata(&retained_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let document: Value = serde_json::from_slice(&retained).unwrap();
        assert_eq!(document["schema"], "local-v1");
        assert_eq!(document["goals"], json!(startup.goals()));
        assert_eq!(document["read_records_sha256"], json!(read_hash));
        assert_eq!(document["sdk_qualification"], false);
        assert_eq!(document["source_recipes_executed"], false);
        assert_eq!(document["native_selection_proven"], false);
        assert_eq!(document["policy_discharged"], false);
        assert_eq!(document["active_event_counts"]["default_rule"], 1);
        assert_eq!(document["active_event_counts"]["phony_targets"], 1);
        assert_eq!(document["active_event_counts"]["suffixes"], 1);
        assert_eq!(document["active_event_counts"]["search_path"], 1);

        let rules = document["rules"].as_array().unwrap();
        let find_rule = |targets: &str| {
            rules
                .iter()
                .find(|rule| rule["targets"] == targets)
                .unwrap_or_else(|| panic!("missing rule metadata for {targets}"))
        };
        let explicit = find_rule("all");
        assert_eq!(explicit["target_pattern"], Value::Null);
        assert_eq!(explicit["prerequisites"], "dependency");
        assert_eq!(explicit["recipe_count"], 2);
        assert_eq!(explicit["input"], "fixture.mk");
        let static_rule = find_rule("static.obj");
        assert_eq!(static_rule["target_pattern"], "%.obj");
        assert_eq!(static_rule["prerequisites"], "%.src");
        assert_eq!(static_rule["order_only"], "order-only");
        assert_eq!(static_rule["recipe_count"], 1);
        let implicit = find_rule("%.obj");
        assert_eq!(implicit["target_pattern"], Value::Null);
        assert_eq!(implicit["recipe_count"], 1);
        let generated = find_rule("generated");
        assert_eq!(generated["generated"], true);
        assert_eq!(generated["recipe_count"], 0);

        let policies = document["policies"].as_array().unwrap();
        assert_eq!(
            policies
                .iter()
                .map(|policy| policy["policy"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["default_rule", "phony_targets", "suffixes", "search_path"]
        );
        assert_eq!(policies[0]["targets"], ".DEFAULT");
        assert_eq!(policies[0]["prerequisites"], "fallback");
        assert_eq!(policies[0]["recipe_count"], 1);
        assert_eq!(policies[1]["names"], json!(["all"]));
        assert_eq!(policies[2]["names"], json!([".src", ".obj"]));
        assert_eq!(policies[3]["arguments"], "%.src sources");
        let policy_indices = policies
            .iter()
            .map(|policy| policy["event_index"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert!(policy_indices.windows(2).all(|pair| pair[0] < pair[1]));
        for policy in policies {
            let event = &file.events[policy["event_index"].as_u64().unwrap() as usize];
            assert_eq!(policy["input"], event.input);
            assert_eq!(policy["source_line"], event.source_line);
            assert_eq!(policy["generated"], event.generated);
        }
        for private_recipe in [
            "INLINE_RECIPE_PRIVATE",
            "TAB_RECIPE_PRIVATE",
            "STATIC_RECIPE_PRIVATE",
            "IMPLICIT_RECIPE_PRIVATE",
            "DEFAULT_RECIPE_PRIVATE",
            "ASSIGNMENT=private",
        ] {
            assert!(!String::from_utf8_lossy(&retained).contains(private_recipe));
        }
    }

    #[test]
    fn rejects_recipe_events_with_invalid_or_forward_references() {
        let startup = startup();
        let digest = Sha256Digest::parse(&"a".repeat(64)).unwrap();
        let invalid = MakeFileProgram {
            events: vec![
                event(MakeFileEventKind::Control("control".into())),
                event(MakeFileEventKind::Recipe {
                    rule_event: 0,
                    text: "private recipe".into(),
                }),
            ],
        };
        assert_eq!(
            inventory_bytes(&invalid, &startup, &digest, 8, 4096).unwrap_err(),
            "source inventory rule association is inconsistent"
        );

        let forward = MakeFileProgram {
            events: vec![
                event(MakeFileEventKind::Recipe {
                    rule_event: 1,
                    text: "private recipe".into(),
                }),
                event(MakeFileEventKind::Rule(RuleHeader::Rule {
                    targets: "target".into(),
                    target_pattern: None,
                    prerequisites: String::new(),
                    order_only: String::new(),
                    double_colon: false,
                    inline_recipe: None,
                })),
            ],
        };
        assert_eq!(
            inventory_bytes(&forward, &startup, &digest, 8, 4096).unwrap_err(),
            "source inventory rule association is inconsistent"
        );
    }

    #[test]
    fn rejects_foreign_and_inactive_recipe_owners_and_invalid_default_headers() {
        let startup = startup();
        let digest = Sha256Digest::parse(&"c".repeat(64)).unwrap();
        let ordinary_rule = || {
            MakeFileEventKind::Rule(RuleHeader::Rule {
                targets: "target".into(),
                target_pattern: None,
                prerequisites: String::new(),
                order_only: String::new(),
                double_colon: false,
                inline_recipe: None,
            })
        };
        let mut foreign_recipe = event(MakeFileEventKind::Recipe {
            rule_event: 0,
            text: "foreign recipe".into(),
        });
        foreign_recipe.input = "other.mk".into();
        let foreign = MakeFileProgram {
            events: vec![event(ordinary_rule()), foreign_recipe],
        };
        assert_eq!(
            inventory_bytes(&foreign, &startup, &digest, 8, 4096).unwrap_err(),
            "source inventory rule association is inconsistent"
        );

        let mut inactive_owner = event(ordinary_rule());
        inactive_owner.active = false;
        let inactive = MakeFileProgram {
            events: vec![
                inactive_owner,
                event(MakeFileEventKind::Recipe {
                    rule_event: 0,
                    text: "inactive owner recipe".into(),
                }),
            ],
        };
        assert_eq!(
            inventory_bytes(&inactive, &startup, &digest, 8, 4096).unwrap_err(),
            "source inventory rule association is inconsistent"
        );

        let invalid_default = MakeFileProgram {
            events: vec![event(MakeFileEventKind::DefaultRule(
                RuleHeader::TargetAssignment {
                    targets: ".DEFAULT".into(),
                    assignment: "VALUE := invalid".into(),
                },
            ))],
        };
        assert_eq!(
            inventory_bytes(&invalid_default, &startup, &digest, 8, 4096).unwrap_err(),
            "active default rule has a non-rule header"
        );
    }

    #[test]
    fn enforces_event_and_estimated_json_byte_limits_before_serialization() {
        let startup = startup();
        let digest = Sha256Digest::parse(&"b".repeat(64)).unwrap();
        let file = MakeFileProgram {
            events: vec![event(MakeFileEventKind::Rule(RuleHeader::Rule {
                targets: "a-long-target-name".into(),
                target_pattern: None,
                prerequisites: String::new(),
                order_only: String::new(),
                double_colon: false,
                inline_recipe: None,
            }))],
        };
        assert_eq!(
            inventory_bytes(&file, &startup, &digest, 0, 4096).unwrap_err(),
            "source event count exceeds inventory limit"
        );
        assert_eq!(
            inventory_bytes(&file, &startup, &digest, 8, 128).unwrap_err(),
            "source inventory exceeds its JSON byte budget"
        );
    }

    fn event(kind: MakeFileEventKind) -> MakeFileEvent {
        MakeFileEvent {
            input: "fixture.mk".into(),
            source_line: 0,
            generated: false,
            active: true,
            variable_position: 0,
            kind,
        }
    }

    use serde_json::json;
}
