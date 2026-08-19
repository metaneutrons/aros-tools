use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about = "Safe Rust genmodule implementation for AROS")]
struct Args {
    #[arg(short, long)]
    input: PathBuf,

    #[arg(short, long)]
    output_dir: PathBuf,
}

fn main() {
    let args = Args::parse();
    println!(
        "⚡ aros-genmodule: Processing {} -> {}",
        args.input.display(),
        args.output_dir.display()
    );
}
