//! Despacho e impressão do `caronte` (CC:289-299).

mod brief;
mod pack;
mod unpack;

use std::path::Path;

use hades_core::Env;
use hades_core::output::{HadesError, die, exit, out};

/// CC:25-33 (`USAGE`).
pub const USAGE: &str = "uso: hades caronte pack <plano> [--out <arq>] [--with-journal] [--allow <caminho>]… [--dry-run]
     hades caronte unpack <arq> [--work-root <d>] [--plans <d>] [--worktrees <d>] [--clone <d>] [--dry-run]
     hades caronte brief <plano>
  pack   empacota o plano, os registros, relatórios e worktrees dos agentes num arquivo cifrado (AES-256-GCM, senha pelo
         tty ou HADES_CARONTE_PASSWORD). Recusa se algum agente não está estacionado ou encerrado (hades agent park <nome>).
         ATENÇÃO: a senha não é guardada em lugar nenhum; senha esquecida perde o pacote.
  unpack abre o pacote (senha pelo tty, uma vez, ou HADES_CARONTE_PASSWORD) e refaz plano, registros, relatórios e worktrees; recusa
         sem tocar em nada se algo já existe ou falta (clone, base, bundle); nunca clona sozinho. --dry-run só mostra o mapa.
  brief  imprime o prompt de retomada do orquestrador numa outra sessão da mesma máquina (só leitura)";

/// CC:289-299 (`caronteCommand`). `stdin_is_tty` decide se a senha poderia vir do tty.
pub fn run(args: &[String], env: &Env, cwd: &Path, stdin_is_tty: bool) -> Result<i32, HadesError> {
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("pack") => pack::run(rest, env, cwd, stdin_is_tty),
        Some("brief") => brief::run(rest, env, cwd),
        Some("unpack") => unpack::run(rest, env, cwd, stdin_is_tty),
        sub => {
            out(USAGE);
            match sub {
                None | Some("-h" | "--help") => Ok(exit::USAGE),
                Some(other) => die(
                    format!("subcomando desconhecido: {other}"),
                    exit::USAGE,
                    None,
                ),
            }
        }
    }
}
