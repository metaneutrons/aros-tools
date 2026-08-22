use aros_common::read_source;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn galliumvm_has_no_legacy_declaration_or_consumer_without_a_target_llvm_runtime() {
    let root = source_root();
    let tombstone = root.join("workbench/libs/mesa/libgalliumvm/mmakefile.src");
    let explanation = read_source(&tombstone).unwrap();
    assert!(explanation.contains("intentionally retired"));
    assert!(explanation.contains("target-side LLVM"));
    assert!(explanation.contains("active consumer"));

    let mut active_references = Vec::new();
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|entry| {
            matches!(
                entry.file_name().to_str(),
                Some("mmakefile.src" | "mmakefile")
            )
        })
    {
        let relative = entry.path().strip_prefix(&root).unwrap();
        let content = read_source(entry.path()).unwrap();
        for (index, line) in content.lines().enumerate() {
            let semantic = line.trim_start();
            // #MM and #MM- are active MetaMake dependency edges. Only ##MM
            // disables such an edge; ordinary comments remain non-semantic.
            if semantic.starts_with("##MM")
                || (semantic.starts_with('#') && !semantic.starts_with("#MM"))
            {
                continue;
            }
            let semantic = semantic.to_ascii_lowercase();
            if semantic.contains("galliumvm") || semantic.contains("gallivm") {
                active_references.push(format!("{}:{}", relative.display(), index + 1));
            }
        }
    }

    assert!(
        active_references.is_empty(),
        "Gallivm was reintroduced without its target LLVM capability: {active_references:#?}"
    );
}
