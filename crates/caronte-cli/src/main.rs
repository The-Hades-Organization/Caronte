//! `caronte`: argumentos, impressão e códigos de saída; a regra mora no `caronte-core` (`src/commands/caronte.ts`).

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use hades_core::Env;
use hades_core::output::warn;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let tty = std::io::stdin().is_terminal();
    match caronte_cli::run(&args, &Env::from_process(), &cwd, tty) {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            warn(&format!("hades: {}", error.message));
            ExitCode::from(error.code as u8)
        }
    }
}
