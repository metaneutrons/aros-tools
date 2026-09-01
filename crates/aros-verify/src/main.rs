//! Thin executable boundary for the independent AROS verifier.

fn main() -> std::process::ExitCode {
    aros_verify::entry(std::env::args_os().collect())
}
