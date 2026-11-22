//! `caronte unpack <arq> [--work-root <d>] [--plans <d>] [--worktrees <d>] [--clone <d>] [--dry-run]` (CC:201-287).

use std::fs;
use std::path::Path;

use caronte_core::format::decrypt_package;
use caronte_core::gitx::SystemGit;
use caronte_core::manifest::verify_parts;
use caronte_core::pack::{caronte_failure, password_from_env};
use caronte_core::plan::resume_prompt;
use caronte_core::start::start_command;
use caronte_core::unpack::{Prepared, UnpackOptions, apply, prepare};
use hades_core::Env;
use hades_core::fsx::{is_file, read_text, resolve};
use hades_core::output::{HadesError, die, exit, out, warn};
use hades_core::paths::skill_root;

const DEFAULT_REPORTS: &str = ".design-sync-reports";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UnpackArgs {
    pub file: String,
    pub work_root: Option<String>,
    pub plans: Option<String>,
    pub worktrees: Option<String>,
    pub clone: Option<String>,
    pub dry_run: bool,
}

fn dir_value(args: &[String], i: usize, flag: &str) -> Result<String, HadesError> {
    match args.get(i) {
        Some(value) => Ok(resolve(Path::new(value)).to_string_lossy().into_owned()),
        None => die(format!("{flag} pede o diretório"), exit::USAGE, None),
    }
}

/// CC:207-223 (`parseUnpack`).
pub fn parse(args: &[String]) -> Result<UnpackArgs, HadesError> {
    let mut parsed = UnpackArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--dry-run" => parsed.dry_run = true,
            "--work-root" | "--plans" | "--worktrees" | "--clone" => {
                i += 1;
                let value = Some(dir_value(args, i, arg)?);
                match arg {
                    "--work-root" => parsed.work_root = value,
                    "--plans" => parsed.plans = value,
                    "--worktrees" => parsed.worktrees = value,
                    _ => parsed.clone = value,
                }
            }
            _ if arg.starts_with('-') => {
                return die(format!("opção desconhecida: {arg}"), exit::USAGE, None);
            }
            _ if parsed.file.is_empty() => parsed.file = arg.to_owned(),
            _ => return die(format!("argumento a mais: {arg}"), exit::USAGE, None),
        }
        i += 1;
    }
    if parsed.file.is_empty() {
        return die("faltou o arquivo do pacote", exit::USAGE, None);
    }
    Ok(parsed)
}

/// CC:225-235 (`describePlan`).
fn describe_plan(prepared: &Prepared, dry_run: bool) {
    out(&format!(
        "caronte unpack{}: mapa origem → destino",
        if dry_run { " --dry-run" } else { "" }
    ));
    for (from, to) in &prepared.path_map {
        out(&format!("  {from}  →  {to}"));
    }
    out(&format!("  clone: {}", prepared.clone));
    if dry_run {
        out("seria criado:");
        for file in prepared.plan_writes.iter().chain(&prepared.files) {
            out(&format!("  {}", file.path));
        }
        for entry in &prepared.worktrees {
            out(&format!(
                "  worktree {} (branch {}, head {})",
                entry.dest,
                entry.info.branch,
                entry.info.head.chars().take(12).collect::<String>()
            ));
        }
    }
}

/// CC:236-249 (`resumeLines`).
fn resume_lines(prepared: &Prepared, env: &Env, cwd: &Path) {
    let resumable: Vec<_> = prepared
        .agents
        .iter()
        .filter(|a| a.state == "parked")
        .collect();
    out("");
    out("para retomar, por agente (o dono inicia cada um):");
    for agent in &resumable {
        out(&format!("  {}:", agent.name));
        out(&format!(
            "    herdr workspace create --cwd {} --label hd-{} --no-focus",
            agent.worktree, agent.name
        ));
        out(&format!(
            "    {}",
            start_command(env, cwd, &agent.name, "", &agent.role)
        ));
        out(&format!(
            "    hades agent adopt {0} && hades agent resume {0}",
            agent.name
        ));
    }
    if resumable.is_empty() {
        out("  (nenhum agente estacionado no pacote)");
    }
    out("");
    let names: Vec<String> = resumable.iter().map(|a| a.name.clone()).collect();
    out(&resume_prompt(&prepared.plan_file, &names));
}

/// CC:251-287 (`unpack`).
pub fn run(args: &[String], env: &Env, cwd: &Path, stdin_is_tty: bool) -> Result<i32, HadesError> {
    let options = parse(args)?;
    let file = resolve(Path::new(&options.file));
    if !is_file(&file) {
        return die(
            format!("pacote não encontrado: {}", file.display()),
            exit::USAGE,
            None,
        );
    }
    let password = password_from_env(env, stdin_is_tty)?;
    let bytes = fs::read(&file)
        .map_err(|e| HadesError::with_code(format!("{}: {e}", file.display()), exit::FAIL))?;
    let parts = decrypt_package(&bytes, &password).map_err(|e| caronte_failure(&e))?;
    let manifest = verify_parts(&parts).map_err(|e| caronte_failure(&e))?;
    let reports = env
        .get("HADES_ORCH_REPORTS_DIR")
        .unwrap_or(DEFAULT_REPORTS)
        .to_owned();
    let skill_version = read_text(&skill_root(env).join("VERSION"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    let unpack_failure =
        |e: caronte_core::unpack::UnpackError| HadesError::with_code(e.0, exit::FAIL);
    let prepared = prepare(
        &SystemGit,
        &parts,
        &manifest,
        &UnpackOptions {
            work_root: options.work_root.clone(),
            plans: options.plans.clone(),
            worktrees: options.worktrees.clone(),
            clone: options.clone.clone(),
            reports,
            skill_version,
        },
        env,
    )
    .map_err(unpack_failure)?;
    for line in &prepared.warnings {
        warn(&format!("hades: aviso: {line}"));
    }
    describe_plan(&prepared, options.dry_run);
    if options.dry_run {
        out("o unpack seguiria (nada foi gravado)");
        return Ok(exit::OK);
    }
    apply(&SystemGit, &prepared).map_err(unpack_failure)?;
    out(&format!(
        "unpack concluído: {} worktree(s), {} agente(s), plano {}",
        prepared.worktrees.len(),
        prepared.agents.len(),
        prepared.plan_file
    ));
    resume_lines(&prepared, env, cwd);
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_completo() {
        let parsed = parse(&args(&[
            "p.caronte",
            "--work-root",
            "/w",
            "--plans",
            "/p",
            "--worktrees",
            "/t",
            "--clone",
            "/c",
            "--dry-run",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            UnpackArgs {
                file: "p.caronte".into(),
                work_root: Some("/w".into()),
                plans: Some("/p".into()),
                worktrees: Some("/t".into()),
                clone: Some("/c".into()),
                dry_run: true,
            }
        );
    }

    #[test]
    fn parse_recusas_saem_2() {
        for bad in [
            vec![],
            vec!["f", "--work-root"],
            vec!["f", "--clone"],
            vec!["f", "--x"],
            vec!["f", "g"],
        ] {
            assert_eq!(parse(&args(&bad)).unwrap_err().code, 2, "{bad:?}");
        }
    }

    #[test]
    fn diretorios_sao_resolvidos() {
        let parsed = parse(&args(&["f", "--plans", "/a/./b/../c"])).unwrap();
        assert_eq!(parsed.plans.as_deref(), Some("/a/c"));
    }
}
