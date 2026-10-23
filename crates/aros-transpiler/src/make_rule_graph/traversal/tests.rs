use super::{select_explicit, ExplicitTraversal};
use crate::make_rule_events::{collect, instantiate, RuleInstance};
use crate::make_rule_graph::{compose, RuleGraph, TargetRules};
use crate::make_rule_program::{expand, RuleProgram};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn positioned(source: &str) -> RuleProgram {
    let states = vec![Some(true); source.lines().count()];
    expand(source, &states, &|_, _, _| {
        Err("unexpected rule-program expression callback".into())
    })
    .unwrap()
}

fn parsed_instances(source: &str) -> Vec<RuleInstance> {
    let events = collect(&positioned(source)).unwrap();
    assert!(events.other_lines.is_empty());
    assert!(events.target_assignment_lines.is_empty());
    events
        .rules
        .iter()
        .flat_map(|rule| instantiate(rule, &|value, _| Ok(value.to_owned())).unwrap())
        .collect()
}

fn graph_from_source(source: &str) -> RuleGraph {
    compose(parsed_instances(source)).unwrap()
}

fn visited_targets(traversal: &ExplicitTraversal) -> Vec<&str> {
    traversal
        .visits()
        .iter()
        .map(super::TargetVisit::target)
        .collect()
}

#[test]
fn shared_targets_are_visited_once_and_keep_the_first_goals_parent_chain() {
    let graph = graph_from_source("rootA: shared\nrootB: shared\nshared:\n");

    let forward = select_explicit(&graph, &["rootA", "rootB"], &|_| Ok(false)).unwrap();
    assert_eq!(visited_targets(&forward), ["rootA", "shared", "rootB"]);
    assert_eq!(forward.goals(), [0, 2]);
    assert_eq!(forward.completion_order(), [1, 0, 2]);
    let shared = forward
        .visits()
        .iter()
        .position(|visit| visit.target() == "shared")
        .unwrap();
    assert_eq!(
        forward
            .visits()
            .iter()
            .filter(|visit| visit.target() == "shared")
            .count(),
        1
    );
    assert_eq!(forward.visits()[shared].parent(), Some(0));
    assert_eq!(forward.scope_chain(shared).unwrap(), ["rootA", "shared"]);

    let reverse = select_explicit(&graph, &["rootB", "rootA"], &|_| Ok(false)).unwrap();
    assert_eq!(visited_targets(&reverse), ["rootB", "shared", "rootA"]);
    assert_eq!(reverse.goals(), [0, 2]);
    let shared = reverse
        .visits()
        .iter()
        .position(|visit| visit.target() == "shared")
        .unwrap();
    assert_eq!(reverse.visits()[shared].parent(), Some(0));
    assert_eq!(reverse.scope_chain(shared).unwrap(), ["rootB", "shared"]);
}

#[test]
fn recipe_provider_is_first_and_each_declaration_keeps_normal_then_order_only_order() {
    let graph = graph_from_source(concat!(
        "target: first | first_stamp\n",
        "target: middle | middle_stamp\n",
        "\t@printf ''\n",
        "target: last | last_stamp\n",
        "first:\n",
        "first_stamp:\n",
        "middle:\n",
        "middle_stamp:\n",
        "last:\n",
        "last_stamp:\n",
    ));

    let traversal = select_explicit(&graph, &["target"], &|_| Ok(false)).unwrap();
    assert_eq!(
        visited_targets(&traversal),
        [
            "target",
            "middle",
            "middle_stamp",
            "first",
            "first_stamp",
            "last",
            "last_stamp"
        ]
    );
    assert_eq!(traversal.completion_order(), [1, 2, 3, 4, 5, 6, 0]);

    let target = traversal.visits()[0].instance().unwrap();
    assert_eq!(
        target.prerequisite_order,
        [
            "middle",
            "middle_stamp",
            "first",
            "first_stamp",
            "last",
            "last_stamp"
        ]
    );
    assert_eq!(target.prerequisites, ["middle", "first", "last"]);
    assert_eq!(
        target.order_only,
        ["middle_stamp", "first_stamp", "last_stamp"]
    );
}

#[test]
fn promoted_early_order_only_keeps_walk_order_and_correct_automatic_lists() {
    let graph = graph_from_source(concat!(
        "target: | early\n",
        "target: normal early\n",
        "target:\n",
        "\t@printf 'target\\n'\n",
        "early:\n",
        "normal:\n",
    ));

    let traversal = select_explicit(&graph, &["target"], &|_| Ok(false)).unwrap();
    assert_eq!(visited_targets(&traversal), ["target", "early", "normal"]);
    assert_eq!(traversal.completion_order(), [1, 2, 0]);

    let target = traversal.visits()[0].instance().unwrap();
    assert_eq!(target.prerequisite_order, ["early", "normal", "early"]);
    let automatic = target.automatic_variables();
    assert_eq!(automatic["<"], "normal");
    assert_eq!(automatic["+"], "normal early");
    assert_eq!(automatic["^"], "early normal");
    assert_eq!(automatic["|"], "");
    assert_eq!(automatic["^D"], ". .");
    assert_eq!(automatic["^F"], "early normal");
}

#[test]
fn missing_leaf_is_admitted_only_when_the_input_callback_confirms_it() {
    let graph = graph_from_source("root: leaf\n");
    let checked = std::cell::RefCell::new(Vec::new());
    let traversal = select_explicit(&graph, &["root"], &|target| {
        checked.borrow_mut().push(target.to_owned());
        Ok(target == "leaf")
    })
    .unwrap();

    assert_eq!(*checked.borrow(), ["leaf"]);
    assert_eq!(visited_targets(&traversal), ["root", "leaf"]);
    assert!(traversal.visits()[1].instance().is_none());
    assert_eq!(traversal.visits()[1].parent(), Some(0));

    let error = select_explicit(&graph, &["root"], &|_| Ok(false)).unwrap_err();
    assert_eq!(
        error,
        "Make traversal leaf has no selected rule or admitted input"
    );
}

#[test]
fn unknown_goal_and_unknown_leaf_fail_without_hiding_callback_errors() {
    let empty = RuleGraph::default();
    let unknown_goal = select_explicit(&empty, &["unknown"], &|_| Ok(false)).unwrap_err();
    assert_eq!(
        unknown_goal,
        "Make traversal leaf has no selected rule or admitted input"
    );

    let graph = graph_from_source("root: unknown_leaf\n");
    let callback_error = select_explicit(&graph, &["root"], &|target| {
        Err(format!("unknown selected target: {target}"))
    })
    .unwrap_err();
    assert_eq!(callback_error, "unknown selected target: unknown_leaf");
}

#[test]
fn cycles_double_colon_rules_and_wait_control_edges_are_refused() {
    let cycle = graph_from_source("first: second\nsecond: first\n");
    assert_eq!(
        select_explicit(&cycle, &["first"], &|_| Ok(false)).unwrap_err(),
        "Make traversal contains a dependency cycle"
    );

    let double_colon = graph_from_source("target:: leaf\nleaf:\n");
    assert_eq!(
        select_explicit(&double_colon, &["target"], &|_| Ok(false)).unwrap_err(),
        "Make traversal does not schedule double-colon rules"
    );

    let wait = graph_from_source("root: .WAIT leaf\nleaf:\n");
    assert_eq!(
        select_explicit(&wait, &["root"], &|_| Ok(false)).unwrap_err(),
        "Make traversal requires a safe concrete target name"
    );
}

#[test]
fn unsafe_control_and_whitespace_goals_are_refused_before_lookup() {
    let graph = RuleGraph::default();
    for goal in [".PHONY", "unsafe goal"] {
        assert_eq!(
            select_explicit(&graph, &[goal], &|_| Ok(false)).unwrap_err(),
            "Make traversal requires a safe concrete target name"
        );
    }
}

#[test]
fn goal_also_encountered_as_an_earlier_child_reuses_its_parent_chain() {
    let graph = graph_from_source("root: child\nchild:\n");
    let traversal = select_explicit(&graph, &["root", "child"], &|_| Ok(false)).unwrap();

    assert_eq!(visited_targets(&traversal), ["root", "child"]);
    assert_eq!(traversal.goals(), [0, 1]);
    assert_eq!(traversal.visits()[1].parent(), Some(0));
    assert_eq!(traversal.scope_chain(1).unwrap(), ["root", "child"]);
}

#[test]
fn depth_target_edge_and_retained_byte_counters_stop_bounded_walks() {
    let mut source = String::new();
    for index in 0..=super::MAX_DEPTH {
        if index == super::MAX_DEPTH {
            writeln!(source, "target{index}:").unwrap();
        } else {
            writeln!(source, "target{index}: target{}", index + 1).unwrap();
        }
    }
    let graph = graph_from_source(&source);
    let at_depth_limit = select_explicit(&graph, &["target1"], &|_| Ok(false)).unwrap();
    assert_eq!(at_depth_limit.visits().len(), super::MAX_DEPTH);
    assert_eq!(
        select_explicit(&graph, &["target0"], &|_| Ok(false)).unwrap_err(),
        "Make traversal exceeds depth or target limit"
    );

    let too_many_goals = vec!["goal"; super::MAX_TARGETS + 1];
    let at_goal_limit = select_explicit(
        &RuleGraph::default(),
        &too_many_goals[..super::MAX_TARGETS],
        &|_| Ok(true),
    )
    .unwrap();
    assert_eq!(at_goal_limit.goals().len(), super::MAX_TARGETS);
    assert_eq!(at_goal_limit.visits().len(), 1);
    assert_eq!(
        select_explicit(&RuleGraph::default(), &too_many_goals, &|_| Ok(false)).unwrap_err(),
        "Make traversal requires bounded explicit goals and targets"
    );

    let mut oversized_graph = RuleGraph::default();
    for index in 0..=super::MAX_TARGETS {
        oversized_graph.targets.insert(
            format!("target{index}"),
            TargetRules::DoubleColon(Vec::new()),
        );
    }
    assert_eq!(
        select_explicit(&oversized_graph, &["target0"], &|_| Ok(false)).unwrap_err(),
        "Make traversal requires bounded explicit goals and targets"
    );

    // Each source declaration remains within its own word bound. The complete
    // selected target exceeds the separate traversal edge bound only after
    // ordinary composition, not through an invalid oversized declaration.
    let graph = compose(
        (0..=super::MAX_EDGES / 1024)
            .map(|line| RuleInstance {
                output_line: line,
                source_line: line,
                target: "root".into(),
                stem: None,
                prerequisites: Vec::new(),
                order_only: vec!["leaf".into(); 1024],
                prerequisite_order: vec!["leaf".into(); 1024],
                double_colon: false,
                recipes: Vec::new(),
            })
            .collect(),
    )
    .unwrap();
    let TargetRules::Ordinary(root) = &graph.targets["root"] else {
        panic!("ordinary root");
    };
    let at_edge_limit = compose(root.declarations()[..super::MAX_EDGES / 1024].to_vec()).unwrap();
    assert!(select_explicit(&at_edge_limit, &["root"], &|name| Ok(name == "leaf")).is_ok());
    assert_eq!(
        select_explicit(&graph, &["root"], &|target| Ok(target == "leaf")).unwrap_err(),
        "Make traversal exceeds prerequisite edge limit"
    );

    let allow_missing = |_: &str| -> Result<bool, String> { Ok(false) };
    let empty_graph = RuleGraph::default();
    let mut walker = super::Walker {
        graph: &empty_graph,
        existing_input: Some(&allow_missing),
        result: ExplicitTraversal::default(),
        seen: BTreeMap::new(),
        edges: 0,
        bytes: 0,
    };
    walker.charge(super::MAX_BYTES).unwrap();
    assert_eq!(
        walker.charge(1).unwrap_err(),
        "Make traversal exceeds retained data limit"
    );
}

#[test]
fn declared_trace_retains_unresolved_frontier_without_admitting_it() {
    let graph =
        graph_from_source("first: shared local\nsecond: shared\nshared: external\nlocal:\n");
    let trace = super::trace_declared(&graph, &["first", "second"]).unwrap();
    assert_eq!(
        trace
            .unresolved_inputs()
            .map(|(_, visit)| visit.target())
            .collect::<Vec<_>>(),
        ["external"]
    );
    let (index, _) = trace.unresolved_inputs().next().unwrap();
    assert_eq!(
        trace.scope_chain(index).unwrap(),
        ["first", "shared", "external"]
    );
    assert!(select_explicit(&graph, &["first", "second"], &|_| Ok(false)).is_err());
    let trace = super::trace_declared(&graph, &["second", "first"]).unwrap();
    let (index, _) = trace.unresolved_inputs().next().unwrap();
    assert_eq!(
        trace.scope_chain(index).unwrap(),
        ["second", "shared", "external"]
    );
    let trace = super::trace_declared(&RuleGraph::default(), &["unknown"]).unwrap();
    assert_eq!(trace.goals(), [0]);
    assert_eq!(trace.unresolved_inputs().count(), 1);
    assert!(trace.scope_chain(1).is_err());
}

#[test]
fn declared_trace_still_rejects_cycles_controls_and_resource_exhaustion() {
    assert!(super::trace_declared(&graph_from_source("a: b\nb: a\n"), &["a"]).is_err());
    assert!(super::trace_declared(&graph_from_source("a:: input\n"), &["a"]).is_err());
    assert!(super::trace_declared(&graph_from_source("a: .WAIT\n"), &["a"]).is_err());
    assert!(super::trace_declared(&RuleGraph::default(), &[]).is_err());
    assert!(super::trace_declared(
        &RuleGraph::default(),
        &vec!["unknown"; super::MAX_TARGETS + 1]
    )
    .is_err());
}

#[test]
fn selected_target_count_accepts_the_limit_and_refuses_one_more() {
    for count in [super::MAX_TARGETS - 1, super::MAX_TARGETS] {
        let source = format!(
            "root: {}\n",
            (0..count)
                .map(|index| format!("leaf{index}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let graph = graph_from_source(&source);
        let result = select_explicit(&graph, &["root"], &|_| Ok(true));
        if count == super::MAX_TARGETS - 1 {
            assert_eq!(result.unwrap().visits().len(), super::MAX_TARGETS);
        } else {
            assert_eq!(
                result.unwrap_err(),
                "Make traversal exceeds depth or target limit"
            );
        }
    }
}

fn find_gnu_make() -> Option<PathBuf> {
    let directories =
        std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect::<Vec<_>>())?;
    ["gmake", "make"].into_iter().find_map(|name| {
        directories
            .iter()
            .map(|directory| directory.join(name))
            .find(|path| {
                Command::new(path)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| {
                        output.status.success()
                            && String::from_utf8_lossy(&output.stdout).contains("GNU Make")
                    })
            })
    })
}

#[test]
fn bounded_stdin_fixture_matches_gnu_make_without_running_build_tools() {
    let executable = find_gnu_make().expect("GNU Make is required for the bounded differential");
    let source = concat!(
        ".PHONY: target early normal\n",
        "target: | early\n",
        "target: normal early\n",
        "target:\n",
        "\t@printf 'target <= [%s] += [%s] ^=[%s] |=[%s] (^D)=[%s] (^F)=[%s]\\n' '$<' '$+' '$^' '$|' '$(^D)' '$(^F)'\n",
        "early:\n",
        "\t@printf 'early\\n'\n",
        "normal:\n",
        "\t@printf 'normal\\n'\n",
    );
    let graph = graph_from_source(source);
    let traversal = select_explicit(&graph, &["target"], &|_| Ok(false)).unwrap();
    let static_completion = traversal
        .completion_order()
        .iter()
        .map(|index| traversal.visits()[*index].target())
        .collect::<Vec<_>>();
    assert_eq!(static_completion, ["early", "normal", "target"]);

    let mut child = Command::new(executable)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .args(["-rR", "-s", "-f", "-", "target"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "GNU Make stdin fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "early\n",
            "normal\n",
            "target <= [normal] += [normal early] ^=[early normal] |=[] (^D)=[. .] (^F)=[early normal]\n"
        )
    );
}
