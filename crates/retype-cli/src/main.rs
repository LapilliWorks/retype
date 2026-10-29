use clap::Parser;

/// Automatic file format conversion on rename.
#[derive(Parser)]
#[command(name = "retype", version, about, long_about = None)]
struct Cli {}

fn main() {
	let _ = Cli::parse();
}
