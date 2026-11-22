//! Senha do pacote (CC:56-99, `readHidden` e `askPassword`): o ambiente vence; sem ele e sem tty não há pacote; com tty
//! a senha é lida sem eco, em modo cru, e nunca passa por argumento nem por arquivo.

use std::io::Write;

use caronte_core::pack::password_from_env;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use hades_core::Env;
use hades_core::output::{HadesError, die, exit};

const PASSWORD_ENV: &str = "HADES_CARONTE_PASSWORD";

struct RawMode;

impl RawMode {
    fn enter() -> Result<Self, HadesError> {
        enable_raw_mode()
            .map_err(|e| HadesError::new(format!("não consegui ler a senha pelo tty: {e}")))?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Aplica uma tecla ao texto digitado. `Some(true)` fecha a leitura, `Some(false)` interrompe (Ctrl-C).
fn apply_key(typed: &mut String, code: KeyCode, modifiers: KeyModifiers) -> Option<bool> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Enter => Some(true),
        // Em modo cru o crossterm entrega o `\n` (0x0A) como Ctrl-J; CC:67 o trata como Enter.
        KeyCode::Char('d' | 'j') if ctrl => Some(true),
        KeyCode::Char('c') if ctrl => Some(false),
        KeyCode::Backspace => {
            typed.pop();
            None
        }
        KeyCode::Char(c) if !ctrl => {
            typed.push(c);
            None
        }
        _ => None,
    }
}

/// CC:56-89 (`readHidden`): prompt no stderr, sem eco, Enter ou Ctrl-D fecham, Ctrl-C interrompe.
fn read_hidden(prompt: &str) -> Result<String, HadesError> {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{prompt}");
    let _ = stderr.flush();
    let mut typed = String::new();
    let finished = {
        let _raw = RawMode::enter()?;
        loop {
            match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    if let Some(done) = apply_key(&mut typed, key.code, key.modifiers) {
                        break done;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = writeln!(stderr);
                    return Err(HadesError::new(format!(
                        "não consegui ler a senha pelo tty: {e}"
                    )));
                }
            }
        }
    };
    let _ = writeln!(stderr);
    if finished {
        Ok(typed)
    } else {
        Err(HadesError::new("interrompido"))
    }
}

/// CC:91-99 (`askPassword`): `HADES_CARONTE_PASSWORD`; senão o tty (com confirmação no `pack`); senão uso (2).
pub fn ask_password(env: &Env, stdin_is_tty: bool, confirm: bool) -> Result<String, HadesError> {
    if env.get(PASSWORD_ENV).is_some() || !stdin_is_tty {
        return password_from_env(env, stdin_is_tty);
    }
    let first = read_hidden(if confirm {
        "Senha do pacote (esquecida, o pacote se perde): "
    } else {
        "Senha do pacote: "
    })?;
    if first.is_empty() {
        return die("senha vazia: sem senha, sem pacote", exit::USAGE, None);
    }
    if confirm && first != read_hidden("Repita a senha: ")? {
        return die("as senhas não conferem", exit::USAGE, None);
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(keys: &[(KeyCode, KeyModifiers)]) -> (String, Option<bool>) {
        let mut typed = String::new();
        for (code, mods) in keys {
            if let Some(done) = apply_key(&mut typed, *code, *mods) {
                return (typed, Some(done));
            }
        }
        (typed, None)
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn digita_apaga_e_fecha_no_enter() {
        let keys = [
            (KeyCode::Char('a'), NONE),
            (KeyCode::Char('b'), NONE),
            (KeyCode::Backspace, NONE),
            (KeyCode::Char('ç'), NONE),
            (KeyCode::Enter, NONE),
        ];
        assert_eq!(feed(&keys), ("aç".to_owned(), Some(true)));
    }

    #[test]
    fn ctrl_d_fecha_e_ctrl_c_interrompe() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            feed(&[(KeyCode::Char('x'), NONE), (KeyCode::Char('d'), ctrl)]),
            ("x".to_owned(), Some(true))
        );
        assert_eq!(
            feed(&[(KeyCode::Char('c'), ctrl)]),
            (String::new(), Some(false))
        );
    }

    #[test]
    fn sem_tty_e_sem_env_e_uso() {
        let error = ask_password(&Env::empty(), false, true).unwrap_err();
        assert_eq!(error.code, 2);
    }

    #[test]
    fn env_vence_o_tty() {
        let env = Env::empty().with(PASSWORD_ENV, "x");
        assert_eq!(ask_password(&env, true, true).unwrap(), "x");
    }
}
