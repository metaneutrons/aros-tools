use std::path::PathBuf;

#[allow(
    dead_code,
    reason = "not every integration binary uses the current-source oracle"
)]
pub fn source_root() -> PathBuf {
    let configured = std::env::var_os("AROS_TEST_SOURCE_ROOT").unwrap_or_else(|| {
        panic!("AROS_TEST_SOURCE_ROOT must name the AROS checkout used by source-contract tests")
    });
    let root = PathBuf::from(configured)
        .canonicalize()
        .expect("AROS_TEST_SOURCE_ROOT must resolve to a directory");
    for marker in ["configure", "Makefile.in", "arch", "compiler", "rom"] {
        assert!(
            root.join(marker).exists(),
            "AROS_TEST_SOURCE_ROOT is missing required marker {marker}"
        );
    }
    root
}

#[allow(dead_code)]
pub fn mesa20_source_root() -> PathBuf {
    let configured = std::env::var_os("AROS_TEST_MESA20_SOURCE_ROOT")
        .expect("AROS_TEST_MESA20_SOURCE_ROOT must name the pinned Mesa 20 checkout");
    PathBuf::from(configured)
        .canonicalize()
        .expect("AROS_TEST_MESA20_SOURCE_ROOT must resolve to a directory")
}
