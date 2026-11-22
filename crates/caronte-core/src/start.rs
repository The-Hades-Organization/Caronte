//! Linha de start do herdr que o `unpack` imprime por agente estacionado. Porta de AR:55-72 (`roleModelDefaults`,
//! `startCommand`) e dos trechos de AP:340, 411-428, 549-551, 598-610, 746-765, 955-967 que ela usa. É só a string:
//! nada é iniciado nem registrado.

use std::path::Path;

use hades_core::Env;
use hades_core::config::config_value;
use hades_core::proc::{RunOptions, run, which};
use serde_json::Value;

/// Binário do Hades; ausente, procura `hades` no PATH.
pub const HADES_BIN_ENV: &str = "CARONTE_HADES_BIN";

const PROVIDER_KEY: &str = "HADES_AGENT_PROVIDER";
const AGY_DEFAULT_OPUS_MODEL: &str = "gemini-4-argon";
const AGY_DEFAULT_SONNET_MODEL: &str = "gemini-4-argon";
const AGY_DEFAULT_HAIKU_MODEL: &str = "gemini-3-flash";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Agy,
}

impl Provider {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "agy" => Some(Self::Agy),
            _ => None,
        }
    }

    fn herdr_kind(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Agy => "agy",
        }
    }
}

/// `readCfg` de AR:67: chave do ambiente ou das Configurações; vazio vale como ausente.
fn read_cfg(env: &Env, cwd: &Path, key: &str) -> String {
    config_value(env, cwd, key, "")
}

/// AP:955-964 (`activeProviderId`): a do papel, depois a global, senão Claude.
fn active_provider(env: &Env, cwd: &Path, role: &str) -> Provider {
    let role = role.trim();
    if !role.is_empty() {
        let key = format!("{PROVIDER_KEY}_{}", role.to_uppercase());
        if let Some(provider) = Provider::parse(read_cfg(env, cwd, &key).trim()) {
            return provider;
        }
    }
    Provider::parse(read_cfg(env, cwd, PROVIDER_KEY).trim()).unwrap_or(Provider::Claude)
}

/// AP:598-610 (`resolveAgyModelString`).
fn agy_model(env: &Env, cwd: &Path, raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    let configured = |key: &str, default: &str| {
        let value = read_cfg(env, cwd, key);
        let value = value.trim();
        if value.is_empty() {
            default.to_owned()
        } else {
            value.to_owned()
        }
    };
    if lower.is_empty()
        || lower == "opus"
        || lower.starts_with("claude-opus")
        || lower.starts_with("claude-fable")
    {
        return configured("HADES_AGY_MODEL_OPUS", AGY_DEFAULT_OPUS_MODEL);
    }
    if lower == "sonnet" || lower.starts_with("claude-sonnet") {
        return configured("HADES_AGY_MODEL_SONNET", AGY_DEFAULT_SONNET_MODEL);
    }
    if lower == "haiku" || lower.starts_with("claude-haiku") {
        return configured("HADES_AGY_MODEL_HAIKU", AGY_DEFAULT_HAIKU_MODEL);
    }
    raw.trim().to_owned()
}

/// AP:411-428 (Claude) e AP:746-765 (agy) com `permissionMode: "auto"`, sem binário nem sessão.
fn interactive_argv(
    env: &Env,
    cwd: &Path,
    provider: Provider,
    model: &str,
    effort: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    match provider {
        Provider::Claude => {
            argv.extend(["--permission-mode".to_owned(), "auto".to_owned()]);
            if !model.is_empty() {
                argv.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        Provider::Agy => {
            argv.extend(["--mode".to_owned(), "auto".to_owned()]);
            if read_cfg(env, cwd, "HADES_AGY_SKIP_PERMISSIONS").trim() == "1" {
                argv.push("--dangerously-skip-permissions".to_owned());
            }
            if !model.is_empty() {
                argv.extend(["--model".to_owned(), agy_model(env, cwd, model)]);
            }
        }
    }
    if !effort.is_empty() {
        argv.extend(["--effort".to_owned(), effort.to_owned()]);
    }
    argv
}

/// AR:66-72 (`startCommand`).
pub fn start_command(env: &Env, cwd: &Path, name: &str, pane: &str, role: &str) -> String {
    let provider = active_provider(env, cwd, role);
    let key = role.to_uppercase();
    let model = read_cfg(env, cwd, &format!("HADES_MODEL_{key}"));
    let effort = read_cfg(env, cwd, &format!("HADES_EFFORT_{key}"));
    let tail = interactive_argv(env, cwd, provider, &model, &effort).join(" ");
    let pane = if pane.is_empty() { "<pane>" } else { pane };
    format!(
        "! herdr agent start {name} --kind {} --pane {pane} -- {tail}",
        provider.herdr_kind()
    )
}

/// Saída de `hades provider start-argv`: `{"kind","args"}` (`startArgv`, `src/lib/agent-registry.ts`).
#[derive(Debug, PartialEq, Eq)]
pub struct StartArgv {
    pub kind: String,
    pub args: Vec<String>,
}

pub fn parse_start_argv(stdout: &str) -> Result<StartArgv, String> {
    let value: Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("JSON inválido: {e}"))?;
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty())
        .ok_or("faltou \"kind\"")?
        .to_owned();
    let args = value
        .get("args")
        .and_then(Value::as_array)
        .ok_or("faltou \"args\"")?
        .iter()
        .map(|a| {
            a.as_str()
                .map(str::to_owned)
                .ok_or("\"args\" só aceita texto")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StartArgv { kind, args })
}

/// Quem informa o argv de start do papel; `None` manda o chamador para o `start_command` local.
pub trait StartArgvSource {
    fn start_argv(&self, role: &str) -> Option<StartArgv>;
}

/// Pede o argv ao `hades provider start-argv --role R`; sem `hades` ou com erro, não informa nada.
pub struct HadesStartArgv {
    hades: Option<String>,
}

impl HadesStartArgv {
    pub fn new(env: &Env) -> Self {
        let hades = match env.get(HADES_BIN_ENV).filter(|v| !v.is_empty()) {
            Some(path) => Some(path.to_owned()),
            None => which("hades", env.get("PATH").unwrap_or(""))
                .map(|p| p.to_string_lossy().into_owned()),
        };
        Self { hades }
    }
}

impl StartArgvSource for HadesStartArgv {
    fn start_argv(&self, role: &str) -> Option<StartArgv> {
        let hades = self.hades.as_deref()?;
        let result = run(
            hades,
            &["provider", "start-argv", "--role", role],
            &RunOptions::default(),
        );
        if result.status != 0 {
            return None;
        }
        parse_start_argv(&result.stdout).ok()
    }
}

/// AR:66-72 (`startCommand`) com o argv vindo do Hades (`startArgv`); sem ele, a porta local.
pub fn start_command_via(
    source: &dyn StartArgvSource,
    env: &Env,
    cwd: &Path,
    name: &str,
    pane: &str,
    role: &str,
) -> String {
    match source.start_argv(role) {
        Some(StartArgv { kind, args }) => {
            let pane = if pane.is_empty() { "<pane>" } else { pane };
            format!(
                "! herdr agent start {name} --kind {kind} --pane {pane} -- {}",
                args.join(" ")
            )
        }
        None => start_command(env, cwd, name, pane, role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::empty().with("HOME", "/nao-existe")
    }

    #[test]
    fn claude_sem_modelo_nem_esforco() {
        let line = start_command(&env(), Path::new("/"), "fake-a", "", "impl");
        assert_eq!(
            line,
            "! herdr agent start fake-a --kind claude --pane <pane> -- --permission-mode auto"
        );
    }

    #[test]
    fn claude_com_modelo_e_esforco_do_papel() {
        let env = env()
            .with("HADES_MODEL_IMPL", "sonnet")
            .with("HADES_EFFORT_IMPL", "low");
        let line = start_command(&env, Path::new("/"), "n", "p1", "impl");
        assert_eq!(
            line,
            "! herdr agent start n --kind claude --pane p1 -- --permission-mode auto --model sonnet --effort low"
        );
    }

    #[test]
    fn provedor_do_papel_vence_o_global() {
        let env = env()
            .with("HADES_AGENT_PROVIDER", "claude")
            .with("HADES_AGENT_PROVIDER_IMPL", "agy")
            .with("HADES_MODEL_IMPL", "claude-sonnet-5");
        let line = start_command(&env, Path::new("/"), "n", "", "impl");
        assert_eq!(
            line,
            "! herdr agent start n --kind agy --pane <pane> -- --mode auto --model gemini-4-argon"
        );
    }

    #[test]
    fn provedor_invalido_cai_no_claude() {
        let env = env().with("HADES_AGENT_PROVIDER", "outro");
        assert!(start_command(&env, Path::new("/"), "n", "", "impl").contains("--kind claude"));
    }

    #[test]
    fn agy_pula_permissoes_com_a_flag() {
        let env = env()
            .with("HADES_AGENT_PROVIDER", "agy")
            .with("HADES_AGY_SKIP_PERMISSIONS", " 1 ");
        assert!(
            start_command(&env, Path::new("/"), "n", "", "impl")
                .ends_with("--mode auto --dangerously-skip-permissions")
        );
    }

    struct Fixed(Option<StartArgv>);

    impl StartArgvSource for Fixed {
        fn start_argv(&self, _role: &str) -> Option<StartArgv> {
            self.0.as_ref().map(|a| StartArgv {
                kind: a.kind.clone(),
                args: a.args.clone(),
            })
        }
    }

    #[test]
    fn le_o_json_do_hades() {
        let parsed =
            parse_start_argv("{\"kind\":\"claude\",\"args\":[\"--permission-mode\",\"auto\"]}\n")
                .unwrap();
        assert_eq!(parsed.kind, "claude");
        assert_eq!(parsed.args, ["--permission-mode", "auto"]);
    }

    #[test]
    fn recusa_saida_incompleta_ou_torta() {
        for bad in [
            "",
            "nope",
            "{}",
            "{\"kind\":\"\",\"args\":[]}",
            "{\"kind\":\"c\"}",
            "{\"kind\":\"c\",\"args\":[1]}",
        ] {
            assert!(parse_start_argv(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn linha_usa_o_argv_do_hades() {
        let source = Fixed(Some(StartArgv {
            kind: "agy".into(),
            args: vec!["--mode".into(), "auto".into()],
        }));
        assert_eq!(
            start_command_via(&source, &env(), Path::new("/"), "n", "p2", "impl"),
            "! herdr agent start n --kind agy --pane p2 -- --mode auto"
        );
    }

    #[test]
    fn sem_argv_do_hades_cai_no_start_command_local() {
        assert_eq!(
            start_command_via(&Fixed(None), &env(), Path::new("/"), "fake-a", "", "impl"),
            start_command(&env(), Path::new("/"), "fake-a", "", "impl")
        );
    }

    #[test]
    fn sem_hades_no_path_nao_informa_argv() {
        let env = Env::empty().with("PATH", "/nao-existe");
        assert!(HadesStartArgv::new(&env).start_argv("impl").is_none());
    }
}
