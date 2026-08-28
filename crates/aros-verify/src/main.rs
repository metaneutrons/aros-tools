//! Thin executable boundary for the independent AROS verifier.

fn main() -> anyhow::Result<()> {
    aros_verify::run()
}
