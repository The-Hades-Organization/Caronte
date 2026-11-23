//! `caronte`: argumentos, impressão e códigos de saída; a regra mora no `caronte-core` (`src/commands/caronte.ts`).

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use hades_core::Env;
use hades_core::errors::render_error;
use hades_core::output::warn;

/// `renderCli` do `cli.ts:146`: o TS imprime com o prefixo `hades`, e o binário o substitui no corte.
const ERROR_PREFIX: &str = "hades";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let tty = std::io::stdin().is_terminal();
    match caronte_cli::run(&args, &Env::from_process(), &cwd, tty) {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            warn(&render_error(&error, ERROR_PREFIX));
            ExitCode::from(error.code as u8)
        }
    }
}
