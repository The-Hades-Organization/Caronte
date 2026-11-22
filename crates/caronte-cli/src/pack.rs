//! `caronte pack <plano> [--out <arq>] [--with-journal] [--allow <caminho>]… [--dry-run]` (CC:121-196).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use caronte_core::format::encrypt_package;
use caronte_core::gitx::SystemGit;
use caronte_core::guard::{Finding, Rule, apply_allow};
use caronte_core::manifest::{ManifestInput, PlanRef, Scope, build_manifest, manifest_part};
use caronte_core::pack::{
    caronte_failure, inside_repo, log2n_of, output_path, password_from_env, plan_stem, repo_of,
    resolve_plan, work_of,
};
use caronte_core::plan::{GatherOptions, gather};
use hades_core::Env;
use hades_core::fsx::read_text;
use hades_core::output::{HadesError, die, exit, out, warn};
use hades_core::paths::skill_root;
use hades_core::registry::iso_now_env;

const PACKAGE_MODE: u32 = 0o600;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackArgs {
    pub plan: String,
    pub out: Option<String>,
    pub with_journal: bool,
    pub allow: Vec<String>,
    pub dry_run: bool,
}

/// CC:121-138 (`parsePack`).
pub fn parse(args: &[String]) -> Result<PackArgs, HadesError> {
    let mut parsed = PackArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--with-journal" => parsed.with_journal = true,
            "--dry-run" => parsed.dry_run = true,
            "--out" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    return die("--out pede o arquivo", exit::USAGE, None);
                };
                parsed.out = Some(value.clone());
            }
            "--allow" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    return die("--allow pede o caminho", exit::USAGE, None);
                };
                parsed.allow.push(value.clone());
            }
            _ if arg.starts_with('-') => {
                return die(format!("opção desconhecida: {arg}"), exit::USAGE, None);
            }
            _ if parsed.plan.is_empty() => parsed.plan = arg.to_owned(),
            _ => return die(format!("argumento a mais: {arg}"), exit::USAGE, None),
        }
        i += 1;
    }
    if parsed.plan.is_empty() {
        return die("faltou o plano", exit::USAGE, None);
    }
    Ok(parsed)
}

/// CC:140-142 (`nameFindings`).
fn name_findings(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| {
            let why = if f.rule == Rule::Nome {
                "nome de credencial"
            } else {
                "padrão de segredo no conteúdo"
            };
            format!("  {} ({why})", f.path)
        })
        .collect()
}

fn write_package(file: &Path, sealed: &[u8]) -> Result<(), HadesError> {
    let fail =
        |e: std::io::Error| HadesError::with_code(format!("{}: {e}", file.display()), exit::FAIL);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = format!("{}.tmp", file.display());
    let mut handle = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PACKAGE_MODE)
        .open(&tmp)
        .map_err(fail)?;
    handle.write_all(sealed).map_err(fail)?;
    drop(handle);
    fs::set_permissions(&tmp, fs::Permissions::from_mode(PACKAGE_MODE)).map_err(fail)?;
    fs::rename(&tmp, file).map_err(fail)
}

/// CC:149-196 (`pack`).
pub fn run(args: &[String], env: &Env, cwd: &Path, stdin_is_tty: bool) -> Result<i32, HadesError> {
    let options = parse(args)?;
    let plan_file = resolve_plan(env, Some(&options.plan))?;
    let work = work_of(env, cwd, &plan_file)?;
    let gathered = gather(
        &SystemGit,
        &GatherOptions {
            work: &work,
            plan_file: &plan_file,
            with_journal: options.with_journal,
        },
    )?;
    let (blocked, allowed) = apply_allow(gathered.plan.findings.clone(), &options.allow);
    let now = iso_now_env(env).map_err(|e| HadesError::with_code(e.to_string(), exit::FAIL))?;
    let plan_name = plan_stem(&plan_file);
    let manifest = build_manifest(
        ManifestInput {
            skill_version: read_text(&skill_root(env).join("VERSION"))
                .unwrap_or_default()
                .trim()
                .to_owned(),
            work_root: work.work.to_string_lossy().into_owned(),
            scope: if work.skill_scope {
                Scope::Skill
            } else {
                Scope::Project
            },
            repo: repo_of(&work),
            plan: PlanRef {
                path: plan_file.to_string_lossy().into_owned(),
                name: plan_name,
                approved_sha: gathered.approved_sha.clone(),
                sha: gathered.plan_sha.clone(),
            },
            agents: gathered.agents.clone(),
            worktrees: gathered.plan.worktrees.clone(),
            with_journal: options.with_journal,
            allowed: allowed.clone(),
        },
        &gathered.plan.parts,
        &now,
    );
    if options.dry_run {
        out(&format!(
            "caronte pack --dry-run: {} (raiz {})",
            plan_file.display(),
            work.work.display()
        ));
        for part in &manifest.parts {
            out(&format!("  {:>10}  {}", part.size, part.path));
        }
        let agents = if gathered.agents.is_empty() {
            "(nenhum)".to_owned()
        } else {
            gathered.agents.join(", ")
        };
        out(&format!("  agentes: {agents}"));
        for line in name_findings(&blocked) {
            out(&format!("  BARRADO {}", line.trim()));
        }
        for path in &allowed {
            out(&format!("  liberado por --allow: {path}"));
        }
        out(if blocked.is_empty() {
            "o pack seguiria (nada foi gravado, nenhuma senha foi pedida)"
        } else {
            "o pack recusaria: use --allow <caminho> para liberar de forma explícita"
        });
        return Ok(if blocked.is_empty() {
            exit::OK
        } else {
            exit::FAIL
        });
    }
    if !blocked.is_empty() {
        warn("hades: o pack recusou, arquivos com credencial (o valor não é mostrado):");
        for line in name_findings(&blocked) {
            warn(&line);
        }
        warn("tire o arquivo da orquestração ou libere de forma explícita com --allow <caminho>");
        return Ok(exit::FAIL);
    }
    let file = output_path(env, cwd, options.out.as_deref(), &plan_file, &now);
    if file.exists() {
        return die(
            format!("{} já existe: escolha outro --out", file.display()),
            exit::USAGE,
            None,
        );
    }
    if inside_repo(&file) {
        return die(
            format!(
                "{} está dentro de um repositório: o pacote tem de ficar fora de qualquer repo (--out)",
                file.display()
            ),
            exit::USAGE,
            None,
        );
    }
    let password = password_from_env(env, stdin_is_tty)?;
    let log2n = log2n_of(env, cwd)?;
    let mut parts = vec![manifest_part(&manifest)];
    parts.extend(gathered.plan.parts.iter().cloned());
    let sealed = encrypt_package(&parts, &password, log2n).map_err(|e| caronte_failure(&e))?;
    write_package(&file, &sealed)?;
    out(&format!(
        "pacote gravado: {} ({} bytes, {} partes, modo 600)",
        file.display(),
        sealed.len(),
        manifest.parts.len()
    ));
    out("a senha não é guardada em lugar nenhum: senha esquecida perde o pacote");
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
            "demo",
            "--out",
            "x.caronte",
            "--allow",
            "a",
            "--allow",
            "b",
            "--with-journal",
            "--dry-run",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            PackArgs {
                plan: "demo".into(),
                out: Some("x.caronte".into()),
                with_journal: true,
                allow: vec!["a".into(), "b".into()],
                dry_run: true,
            }
        );
    }

    #[test]
    fn parse_recusas_saem_2() {
        for bad in [
            vec![],
            vec!["demo", "--out"],
            vec!["demo", "--allow"],
            vec!["demo", "--x"],
            vec!["demo", "outro"],
        ] {
            assert_eq!(parse(&args(&bad)).unwrap_err().code, 2, "{bad:?}");
        }
    }

    #[test]
    fn out_aceita_valor_com_hifen() {
        assert_eq!(
            parse(&args(&["p", "--out", "-x"])).unwrap().out.as_deref(),
            Some("-x")
        );
    }
}
