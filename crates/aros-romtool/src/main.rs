use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "ROM and Distribution Packaging Tool for AROS"
)]
struct Args {
    #[arg(short, long)]
    format: String,

    #[arg(short, long)]
    output: PathBuf,
}

fn main() {
    let args = Args::parse();
    println!(
        "📦 aros-romtool: Building {} image -> {}",
        args.format,
        args.output.display()
    );
}
