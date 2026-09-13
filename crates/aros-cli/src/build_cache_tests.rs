//! Parser contracts for the compiler-cache build option.

use super::Cli;
use clap::{error::ErrorKind, Parser};

fn parse_error(arguments: &[&str]) -> ErrorKind {
    match Cli::try_parse_from(arguments) {
        Ok(_) => panic!("command line unexpectedly parsed: {arguments:?}"),
        Err(error) => error.kind(),
    }
}

#[test]
fn build_compiler_cache_policy_is_explicit_for_product_and_board_builds() {
    for mode in ["auto", "off", "sccache", "ccache"] {
        assert!(
            Cli::try_parse_from(["aros", "build", "--compiler-cache", mode]).is_ok(),
            "product build rejected compiler-cache mode {mode}"
        );
        assert!(
            Cli::try_parse_from([
                "aros",
                "board",
                "build",
                "--profile",
                "fixture",
                "--compiler-cache",
                mode,
            ])
            .is_ok(),
            "board build rejected compiler-cache mode {mode}"
        );
    }
    assert_eq!(
        parse_error(&["aros", "build", "--compiler-cache", "cache"]),
        ErrorKind::InvalidValue
    );
}
