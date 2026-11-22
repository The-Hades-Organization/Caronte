//! Passos do `caronte pack` e do `brief` que não imprimem nada: plano, raiz, saída, KDF e senha
//! (`src/commands/caronte.ts`, CC).

use std::path::{Path, PathBuf};

use hades_core::Env;
use hades_core::config::config_value;
use hades_core::fsx::{is_file, resolve};
use hades_core::output::{HadesError, die, exit};
use hades_core::paths::hades_home;
use hades_core::proc::{RunOptions, run};
use hades_core::work::{WorkEnv, WorkOptions, load_work_env};

use crate::format::{CaronteError, CaronteErrorCode, DEFAULT_LOG2N};
use crate::manifest::Repo;

/// CC:23 (`PASSWORD_ENV`).
pub const PASSWORD_ENV: &str = "HADES_CARONTE_PASSWORD";

/// CC:36-43 (`resolvePlan`): `<arg>`, `<hades_home>/plans/<arg>` e `<hades_home>/plans/<arg>.md`, o primeiro que for
/// arquivo, em caminho absoluto.
pub fn resolve_plan(env: &Env, arg: Option<&str>) -> Result<PathBuf, HadesError> {
    let Some(given) = arg else {
        return die("faltou o plano", exit::USAGE, None);
    };
    let plans = hades_home(env).join("plans");
    let candidates = [
        PathBuf::from(given),
        plans.join(given),
        plans.join(format!("{given}.md")),
    ];
    for candidate in candidates {
        if is_file(&candidate) {
            return Ok(resolve(&candidate));
        }
    }
    die(format!("plano não encontrado: {given}"), exit::USAGE, None)
}

/// CC:45-48 (`workOf`): `_skill` para os planos de `<hades_home>/plans`, a do projeto do diretório para os demais.
pub fn work_of(env: &Env, cwd: &Path, plan_file: &Path) -> Result<WorkEnv, HadesError> {
    let plans = format!("{}/plans/", hades_home(env).to_string_lossy());
    let options = WorkOptions {
        skill_ok: true,
        skill_force: plan_file.to_string_lossy().starts_with(&plans),
        cwd: Some(cwd.to_path_buf()),
        ..WorkOptions::default()
    };
    load_work_env(&options, env)
}

/// CC:50-53 (`repoOf`).
pub fn repo_of(work: &WorkEnv) -> Repo {
    let clone = work.main_clone.to_string_lossy().into_owned();
    let result = run(
        "git",
        &["-C", &clone, "config", "--get", "remote.origin.url"],
        &RunOptions::default(),
    );
    Repo {
        remote: result.stdout.trim().to_owned(),
        clone,
    }
}

/// CC:35 (`utcStamp`): `AAAAMMDDTHHMMSSZ` a partir de `AAAA-MM-DDTHH:MM:SSZ`.
pub fn utc_stamp(iso: &str) -> String {
    let compact: String = iso.chars().filter(|c| *c != '-' && *c != ':').collect();
    format!("{}Z", compact.chars().take(15).collect::<String>())
}

/// CC:101-107 (`log2nOf`): `HADES_CARONTE_SCRYPT_LOG2N`, inteiro de 14 a 20.
pub fn log2n_of(env: &Env, cwd: &Path) -> Result<u8, HadesError> {
    let raw = config_value(
        env,
        cwd,
        "HADES_CARONTE_SCRYPT_LOG2N",
        &DEFAULT_LOG2N.to_string(),
    );
    let parsed = raw.trim().parse::<f64>().ok();
    match parsed {
        Some(value) if value.fract() == 0.0 && (14.0..=20.0).contains(&value) => Ok(value as u8),
        _ => die(
            format!("HADES_CARONTE_SCRYPT_LOG2N={raw}: esperava um inteiro de 14 a 20"),
            exit::USAGE,
            None,
        ),
    }
}

/// CC:109-113 (`outputPath`): `--out`, ou `HADES_CARONTE_OUT_DIR`, ou `<hades_home>/caronte`, com o nome pelo carimbo.
pub fn output_path(
    env: &Env,
    cwd: &Path,
    requested: Option<&str>,
    plan_file: &Path,
    iso_now: &str,
) -> PathBuf {
    if let Some(requested) = requested.filter(|r| !r.is_empty()) {
        return resolve(Path::new(requested));
    }
    let configured = config_value(env, cwd, "HADES_CARONTE_OUT_DIR", "");
    let dir = if configured.is_empty() {
        hades_home(env).join("caronte")
    } else {
        PathBuf::from(configured)
    };
    dir.join(format!(
        "{}-{}.caronte",
        plan_stem(plan_file),
        utc_stamp(iso_now)
    ))
}

/// `planFile.replace(/^.*\//, "").replace(/\.md$/, "")` (CC:152).
pub fn plan_stem(plan_file: &Path) -> String {
    let text = plan_file.to_string_lossy();
    let base = text.rsplit('/').next().unwrap_or("");
    base.strip_suffix(".md").unwrap_or(base).to_owned()
}

/// CC:115-119 (`insideRepo`): o pacote não pode ficar dentro de um repo.
pub fn inside_repo(file: &Path) -> bool {
    let mut dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    while !dir.exists() {
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    let dir = dir.to_string_lossy().into_owned();
    run(
        "git",
        &["-C", &dir, "rev-parse", "--is-inside-work-tree"],
        &RunOptions::default(),
    )
    .stdout
    .trim()
        == "true"
}

/// CC:91-99 (`askPassword`, só a parte do ambiente): `HADES_CARONTE_PASSWORD`; sem ela e sem tty não há pacote. A
/// leitura pelo tty mora no `caronte-cli` (`password::ask_password`), que só chega aqui sem tty ou com a variável.
pub fn password_from_env(env: &Env, stdin_is_tty: bool) -> Result<String, HadesError> {
    if let Some(password) = env.get(PASSWORD_ENV) {
        return Ok(password.to_owned());
    }
    if !stdin_is_tty {
        return die(
            format!("sem senha: sem tty e sem {PASSWORD_ENV}, sem pacote"),
            exit::USAGE,
            None,
        );
    }
    die(
        format!("a leitura da senha pelo tty é do caronte-cli: use {PASSWORD_ENV}"),
        exit::USAGE,
        None,
    )
}

/// Erro de formato vira saída do CLI: senha vazia é `die` sem código (2); o resto, 1 (CC:274).
pub fn caronte_failure(error: &CaronteError) -> HadesError {
    let code = if error.code == CaronteErrorCode::EmptyPassword {
        exit::USAGE
    } else {
        exit::FAIL
    };
    HadesError::with_code(error.message.clone(), code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carimbo_do_nome_do_pacote() {
        assert_eq!(utc_stamp("2026-11-20T12:00:00Z"), "20261120T120000Z");
    }

    #[test]
    fn stem_tira_pasta_e_md() {
        assert_eq!(plan_stem(Path::new("/a/b/demo.md")), "demo");
        assert_eq!(plan_stem(Path::new("/a/b/demo.txt")), "demo.txt");
    }

    #[test]
    fn log2n_aceita_14_a_20() {
        let cwd = Path::new("/");
        for (raw, ok) in [
            ("14", true),
            ("20", true),
            ("13", false),
            ("21", false),
            ("x", false),
            ("15.5", false),
        ] {
            let env = Env::empty()
                .with("HOME", "/nao-existe")
                .with("HADES_CARONTE_SCRYPT_LOG2N", raw);
            assert_eq!(log2n_of(&env, cwd).is_ok(), ok, "{raw}");
        }
    }

    #[test]
    fn senha_sem_env_e_sem_tty_e_uso() {
        let env = Env::empty();
        let error = password_from_env(&env, false).unwrap_err();
        assert_eq!(error.code, 2);
        assert!(error.message.contains(PASSWORD_ENV));
        assert_eq!(
            password_from_env(&Env::empty().with(PASSWORD_ENV, "x"), false).unwrap(),
            "x"
        );
        assert!(password_from_env(&Env::empty().with(PASSWORD_ENV, ""), false).is_err());
    }
}
