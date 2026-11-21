use clap::Parser;

/// Caronte: ferramenta do Hades, reescrita em Rust.
#[derive(Parser)]
#[command(name = "caronte", version, about)]
struct Cli {}

fn main() {
    Cli::parse();
}
