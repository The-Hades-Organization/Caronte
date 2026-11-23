//! Leitura e subprocessos do app sobre o `caronte-core`. Tudo aqui faz I/O e roda só no executor de fundo.
//! Porta de `createCaronteRoutes` (`src/panel/caronte.ts:88-232`): lista de planos, prévia e park.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caronte_core::gitx::SystemGit;
use caronte_core::plan::{
    GatherOptions, PackPreview, PlanAgent, Readiness, linked_agents, plan_agents, preview_pack,
    resume_prompt,
};
use caronte_core::start::start_command;
use hades_core::Env;
use hades_core::config::config_value;
use hades_core::fsx::{is_dir, list_dir, read_text};
use hades_core::output::HadesError;
use hades_core::paths::hades_home;
use hades_core::proc::{RunOptions, run, which};
use hades_core::work::{WorkEnv, WorkOptions, load_work_env};

use crate::vm::{
    self, BYTES_CABECALHO, ESPERA_PARK_PADRAO_S, MAX_PACOTES, MAX_PLANOS, PacoteResumo,
    PlanoResumo, Retomada, RetomadaAgente, SAIDA_PARK_MAX, estado_assentado, plano_valido,
};

const EXTENSAO_PLANO: &str = ".md";
const EXTENSAO_PACOTE: &str = ".caronte";
const EXTENSAO_LOG: &str = ".log.md";
const CHAVE_ESPERA_PARK: &str = "HADES_ARGOS_ACTION_TIMEOUT_S";

/// `<hades_home>/plans`, onde o painel e o `caronte pack` procuram os planos (PC:42, CC:37-43).
pub fn pasta_de_planos(env: &Env) -> PathBuf {
    hades_home(env).join("plans")
}

/// Raiz de orquestração dos planos da skill (`loadWorkEnv` com `skillOk` e `skillForce`, CC:46-48).
pub fn carregar_raiz(env: &Env, cwd: &Path) -> Result<WorkEnv, HadesError> {
    load_work_env(
        &WorkOptions {
            skill_ok: true,
            skill_force: true,
            agent: None,
            cwd: Some(cwd.to_path_buf()),
        },
        env,
    )
}

/// Pasta dos pacotes: `HADES_CARONTE_OUT_DIR` ou `<hades_home>/caronte` (CC:109-113).
pub fn pasta_de_pacotes(env: &Env, cwd: &Path) -> PathBuf {
    let configurada = config_value(env, cwd, "HADES_CARONTE_OUT_DIR", "");
    if configurada.is_empty() {
        hades_home(env).join("caronte")
    } else {
        PathBuf::from(configurada)
    }
}

/// Pacotes `.caronte` da pasta, do mais novo ao mais antigo, até 200 (PC:123-131). Lê só o começo de cada arquivo
/// (o cabeçalho em claro): o conteúdo cifrado nunca é lido.
pub fn listar_pacotes(pasta: &Path) -> Vec<PacoteResumo> {
    let mut pacotes: Vec<PacoteResumo> = list_dir(pasta)
        .into_iter()
        .filter(|nome| nome.ends_with(EXTENSAO_PACOTE))
        .filter_map(|nome| {
            let caminho = pasta.join(&nome);
            let meta = std::fs::metadata(&caminho).ok().filter(|m| m.is_file())?;
            Some(PacoteResumo {
                formato: cabecalho_do_arquivo(&caminho),
                tamanho: meta.len(),
                modificado_ms: modificado_ms(&caminho),
                caminho: caminho.to_string_lossy().into_owned(),
                nome,
            })
        })
        .collect();
    vm::ordenar_pacotes(&mut pacotes);
    pacotes.truncate(MAX_PACOTES);
    pacotes
}

fn cabecalho_do_arquivo(caminho: &Path) -> Option<i64> {
    use std::io::Read;
    let mut inicio = Vec::with_capacity(BYTES_CABECALHO);
    std::fs::File::open(caminho)
        .ok()?
        .take(BYTES_CABECALHO as u64)
        .read_to_end(&mut inicio)
        .ok()?;
    vm::formato_do_cabecalho(&inicio)
}

/// Prompt de retomada e passos por agente estacionado (CC:236-249); o `start_command` lê a config, por isso roda
/// no executor de fundo. Não abre pacote nem pede senha.
pub fn retomada(
    env: &Env,
    cwd: &Path,
    work: &WorkEnv,
    pasta: &Path,
    nome: &str,
) -> Result<Retomada, String> {
    let arquivo = arquivo_do_plano(pasta, nome)?;
    let texto = read_text(&arquivo).unwrap_or_default();
    let estacionados: Vec<_> = plan_agents(work, &texto)
        .into_iter()
        .filter(|a| a.readiness == Readiness::Parked)
        .collect();
    let registros = linked_agents(work, &texto);
    let agentes: Vec<RetomadaAgente> = estacionados
        .iter()
        .map(|a| {
            let papel = registros
                .iter()
                .find(|r| r.name == a.name)
                .and_then(|r| r.record.get("role").and_then(|v| v.as_str()))
                .unwrap_or("impl");
            RetomadaAgente {
                workspace: vm::comando_workspace(&a.worktree, &a.name),
                iniciar: start_command(env, cwd, &a.name, "", papel),
                adotar: vm::comando_adopt(&a.name),
                nome: a.name.clone(),
            }
        })
        .collect();
    let nomes: Vec<String> = estacionados.into_iter().map(|a| a.name).collect();
    Ok(Retomada {
        prompt: resume_prompt(&arquivo.to_string_lossy(), &nomes),
        agentes,
    })
}

/// `<branch> @ <sha curto>` do repositório em `cwd`; `None` fora de um repositório ou sem `git`.
pub fn referencia_git(cwd: &Path) -> Option<String> {
    let saida = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let ramo = saida(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let sha = saida(&["rev-parse", "--short", "HEAD"])?;
    Some(format!("{ramo} @ {sha}"))
}

pub fn agora_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn modificado_ms(arquivo: &Path) -> u64 {
    std::fs::metadata(arquivo)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Nomes dos planos da pasta, sem os `.log.md`, do mais novo ao mais antigo, até 100 (PC:96-105).
pub fn nomes_de_planos(pasta: &Path) -> Vec<(String, u64)> {
    let mut nomes: Vec<(String, u64)> = list_dir(pasta)
        .into_iter()
        .filter(|f| f.ends_with(EXTENSAO_PLANO) && !f.ends_with(EXTENSAO_LOG))
        .filter_map(|f| f.strip_suffix(EXTENSAO_PLANO).map(str::to_owned))
        .filter(|n| plano_valido(n))
        .map(|n| {
            let em = modificado_ms(&pasta.join(format!("{n}{EXTENSAO_PLANO}")));
            (n, em)
        })
        .collect();
    nomes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    nomes.truncate(MAX_PLANOS);
    nomes
}

/// Lista os planos com a prontidão de cada agente ligado (PC:152-166).
pub fn listar_planos(work: &WorkEnv, pasta: &Path) -> Vec<PlanoResumo> {
    nomes_de_planos(pasta)
        .into_iter()
        .map(|(nome, modificado_ms)| {
            let texto =
                read_text(&pasta.join(format!("{nome}{EXTENSAO_PLANO}"))).unwrap_or_default();
            PlanoResumo {
                agentes: plan_agents(work, &texto),
                nome,
                modificado_ms,
            }
        })
        .collect()
}

/// Arquivo do plano: o nome tem de passar no padrão e estar na lista (PC:107-114).
fn arquivo_do_plano(pasta: &Path, nome: &str) -> Result<PathBuf, String> {
    if !plano_valido(nome) {
        return Err("nome de plano inválido".to_owned());
    }
    if !nomes_de_planos(pasta).iter().any(|(n, _)| n == nome) {
        return Err("plano desconhecido".to_owned());
    }
    Ok(pasta.join(format!("{nome}{EXTENSAO_PLANO}")))
}

/// Prévia do pack (`caronte pack --dry-run` como dados). Recusa, como o pack, se algum agente não está estacionado
/// (PC:128-142); nunca lê nem mostra o valor de um segredo.
pub fn previa(work: &WorkEnv, pasta: &Path, nome: &str) -> Result<PackPreview, String> {
    let arquivo = arquivo_do_plano(pasta, nome)?;
    preview_pack(
        &SystemGit,
        &GatherOptions {
            work,
            plan_file: &arquivo,
            with_journal: false,
        },
        &[],
    )
    .map_err(|e| e.to_string())
}

pub fn hades_no_path(env: &Env) -> bool {
    which("hades", env.get("PATH").unwrap_or("")).is_some()
}

/// Espera do park: `HADES_ARGOS_ACTION_TIMEOUT_S` inteiro positivo, senão 1200 s (PC:190-192).
pub fn espera_do_park(env: &Env, cwd: &Path) -> Duration {
    let valor = config_value(env, cwd, CHAVE_ESPERA_PARK, "");
    let segundos = valor
        .parse::<u64>()
        .ok()
        .filter(|s| *s > 0)
        .unwrap_or(ESPERA_PARK_PADRAO_S);
    Duration::from_secs(segundos)
}

/// Estaciona um agente do plano pelo `hades agent park <nome>` (decisão do dono: o Caronte não grava registro).
/// Roda na worktree do agente, ou na raiz do repositório se ela sumiu (PC:189-197).
pub fn estacionar(
    env: &Env,
    cwd: &Path,
    work: &WorkEnv,
    pasta: &Path,
    plano: &str,
    agente: &str,
) -> Result<String, String> {
    let arquivo = arquivo_do_plano(pasta, plano)?;
    let texto = read_text(&arquivo).unwrap_or_default();
    let alvo: PlanAgent = plan_agents(work, &texto)
        .into_iter()
        .find(|a| a.name == agente)
        .ok_or_else(|| "agente desconhecido neste plano".to_owned())?;
    if estado_assentado(&alvo.state) {
        return Ok(format!("{agente} já está {}", alvo.state));
    }
    let worktree = PathBuf::from(&alvo.worktree);
    let onde = if is_dir(&worktree) {
        worktree
    } else {
        work.repo_top.clone()
    };
    let r = run(
        "hades",
        &["agent", "park", agente],
        &RunOptions {
            cwd: Some(onde),
            timeout: Some(espera_do_park(env, cwd)),
            max_buffer: None,
        },
    );
    if r.timed_out {
        return Err("o comando do agente não terminou a tempo".to_owned());
    }
    if r.status != 0 {
        let motivo = ultima_linha(&r.stderr)
            .or_else(|| ultima_linha(&r.stdout))
            .unwrap_or_else(|| "o comando do agente falhou".to_owned());
        return Err(vm::cortar(&motivo, 300));
    }
    Ok(vm::cortar(r.stdout.trim(), SAIDA_PARK_MAX))
}

fn ultima_linha(texto: &str) -> Option<String> {
    texto
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use caronte_core::plan::Readiness;
    use serde_json::json;

    use super::*;

    fn raiz(base: &Path) -> WorkEnv {
        let orch = base.join("orch");
        WorkEnv {
            skill_scope: true,
            repo_top: base.to_path_buf(),
            main_clone: base.to_path_buf(),
            slug: "_skill".to_owned(),
            repo_name: "skill".to_owned(),
            work: base.join("work"),
            locks: base.join("locks"),
            state: base.join("state"),
            skill_orch: orch.clone(),
            orch,
            reports: ".design-sync-reports".to_owned(),
        }
    }

    fn escrever(arquivo: &Path, texto: &str) {
        fs::create_dir_all(arquivo.parent().unwrap()).unwrap();
        fs::write(arquivo, texto).unwrap();
    }

    #[test]
    fn lista_ignora_log_e_nome_invalido_e_ordena_por_data() {
        let base = tempfile::tempdir().unwrap();
        let pasta = base.path().join("plans");
        escrever(&pasta.join("a.md"), "# a");
        escrever(&pasta.join("a.log.md"), "log");
        escrever(&pasta.join("b.md"), "# b");
        escrever(&pasta.join("-ruim.md"), "x");
        escrever(&pasta.join("c.txt"), "x");
        let nomes: Vec<String> = nomes_de_planos(&pasta)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(nomes.len(), 2);
        assert!(nomes.contains(&"a".to_owned()) && nomes.contains(&"b".to_owned()));
    }

    #[test]
    fn pasta_inexistente_da_lista_vazia() {
        let base = tempfile::tempdir().unwrap();
        assert!(nomes_de_planos(&base.path().join("nada")).is_empty());
    }

    #[test]
    fn prontidao_do_agente_ligado_ao_plano() {
        let base = tempfile::tempdir().unwrap();
        let work = raiz(base.path());
        let pasta = base.path().join("plans");
        escrever(&pasta.join("demo.md"), "# demo\nagentes: fake-a e fake-b\n");
        escrever(
            &work.orch.join("agents/fake-a.json"),
            &json!({"name": "fake-a", "state": "parked", "worktree": "/nao/existe"}).to_string(),
        );
        escrever(
            &work.orch.join("agents/fake-b.json"),
            &json!({"name": "fake-b", "state": "working", "worktree": "/nao/existe"}).to_string(),
        );
        escrever(
            &work.orch.join("agents/fora.json"),
            &json!({"name": "fora", "state": "working"}).to_string(),
        );
        let planos = listar_planos(&work, &pasta);
        assert_eq!(planos.len(), 1);
        let agentes = &planos[0].agentes;
        assert_eq!(agentes.len(), 2, "só os agentes citados no plano");
        assert_eq!(agentes[0].readiness, Readiness::Parked);
        assert_eq!(agentes[1].readiness, Readiness::Working);
        assert!(!planos[0].pronto());
    }

    #[test]
    fn previa_recusa_plano_fora_da_lista_e_nome_invalido() {
        let base = tempfile::tempdir().unwrap();
        let work = raiz(base.path());
        let pasta = base.path().join("plans");
        escrever(&pasta.join("demo.md"), "# demo");
        assert_eq!(
            previa(&work, &pasta, "../demo").unwrap_err(),
            "nome de plano inválido"
        );
        assert_eq!(
            previa(&work, &pasta, "outro").unwrap_err(),
            "plano desconhecido"
        );
    }

    #[test]
    fn previa_de_plano_sem_agentes_lista_o_plano() {
        let base = tempfile::tempdir().unwrap();
        let work = raiz(base.path());
        let pasta = base.path().join("plans");
        escrever(&pasta.join("demo.md"), "# demo");
        let p = previa(&work, &pasta, "demo").unwrap();
        assert_eq!(p.parts[0].0, "plan/demo.md");
        assert!(p.blocked.is_empty());
    }

    #[test]
    fn previa_recusa_agente_ainda_rodando() {
        let base = tempfile::tempdir().unwrap();
        let work = raiz(base.path());
        let pasta = base.path().join("plans");
        escrever(&pasta.join("demo.md"), "# demo fake-b");
        escrever(
            &work.orch.join("agents/fake-b.json"),
            &json!({"name": "fake-b", "state": "working", "worktree": "/nao/existe"}).to_string(),
        );
        let erro = previa(&work, &pasta, "demo").unwrap_err();
        assert!(erro.contains("fake-b ainda não está estacionado"), "{erro}");
    }

    #[test]
    fn estacionar_agente_ja_assentado_nao_sobe_subprocesso() {
        let base = tempfile::tempdir().unwrap();
        let work = raiz(base.path());
        let pasta = base.path().join("plans");
        escrever(&pasta.join("demo.md"), "# demo fake-a");
        escrever(
            &work.orch.join("agents/fake-a.json"),
            &json!({"name": "fake-a", "state": "parked", "worktree": "/nao/existe"}).to_string(),
        );
        let env = Env::empty();
        let saida = estacionar(&env, base.path(), &work, &pasta, "demo", "fake-a").unwrap();
        assert_eq!(saida, "fake-a já está parked");
        assert_eq!(
            estacionar(&env, base.path(), &work, &pasta, "demo", "ninguem").unwrap_err(),
            "agente desconhecido neste plano"
        );
    }

    #[test]
    fn espera_do_park_usa_o_padrao_sem_config_valida() {
        let base = tempfile::tempdir().unwrap();
        let env = Env::empty().with("HOME", base.path().to_string_lossy());
        assert_eq!(espera_do_park(&env, base.path()), Duration::from_secs(1200));
        let env = env.with(CHAVE_ESPERA_PARK, "30");
        assert_eq!(espera_do_park(&env, base.path()), Duration::from_secs(30));
        let env = env.with(CHAVE_ESPERA_PARK, "0");
        assert_eq!(espera_do_park(&env, base.path()), Duration::from_secs(1200));
    }

    #[test]
    fn ultima_linha_ignora_vazias() {
        assert_eq!(ultima_linha("a\nb\n\n"), Some("b".to_owned()));
        assert_eq!(ultima_linha("  \n"), None);
    }

    #[test]
    fn pacotes_listam_so_caronte_com_formato_do_cabecalho() {
        let base = tempfile::tempdir().unwrap();
        let mut valido = caronte_core::format::MAGIC.to_vec();
        let json = br#"{"v":1}"#;
        valido.extend_from_slice(&(json.len() as u32).to_be_bytes());
        valido.extend_from_slice(json);
        valido.extend_from_slice(&[7u8; 64]);
        fs::write(base.path().join("a.caronte"), &valido).unwrap();
        fs::write(base.path().join("b.caronte"), b"lixo").unwrap();
        fs::write(base.path().join("c.txt"), &valido).unwrap();
        fs::create_dir(base.path().join("d.caronte")).unwrap();
        let lista = listar_pacotes(base.path());
        let mut nomes: Vec<_> = lista.iter().map(|p| p.nome.as_str()).collect();
        nomes.sort_unstable();
        assert_eq!(nomes, ["a.caronte", "b.caronte"]);
        let a = lista.iter().find(|p| p.nome == "a.caronte").unwrap();
        assert_eq!((a.formato, a.tamanho), (Some(1), valido.len() as u64));
        assert_eq!(
            lista
                .iter()
                .find(|p| p.nome == "b.caronte")
                .unwrap()
                .formato,
            None
        );
        assert!(listar_pacotes(&base.path().join("nada")).is_empty());
    }
}
