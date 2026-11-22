//! O plano e seus agentes: quem está ligado, quem está assentado, o que entra no pacote, a prévia e o prompt de
//! retomada. Porta de LC:323-482.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use hades_core::fsx::{is_file, list_dir, read_text};
use hades_core::journal::parse_iso;
use hades_core::output::{HadesError, die};
use hades_core::registry::plan_sha;
use hades_core::work::{Json, WorkEnv, read_json};
use regex::Regex;

use crate::format::Part;
use crate::gitx::{GitExec, capture_worktree, js_string};
use crate::guard::{Finding, apply_allow, basename, scan_text};
use crate::manifest::WorktreeInfo;

/// LC:31 (`REPORT_FILES`).
pub const REPORT_FILES: [&str; 5] = [
    "brief.md",
    "handoff.md",
    "report.md",
    "questions.md",
    "quality.md",
];

/// LC:32 (`AGENT_NAME`).
pub fn is_agent_name(name: &str) -> bool {
    static NAME: OnceLock<Regex> = OnceLock::new();
    NAME.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]*$").expect("padrão válido"))
        .is_match(name)
}

/// LC:324-327 (`AgentRecord`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRecord {
    pub name: String,
    pub record: Json,
}

/// LC:330-340 (`linkedAgents`): agentes do registro da raiz cujo nome aparece como palavra no texto do plano, na
/// ordem de byte dos arquivos.
pub fn linked_agents(work: &WorkEnv, plan_text: &str) -> Vec<AgentRecord> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"[A-Za-z0-9_-]+").expect("padrão válido"));
    let words: HashSet<&str> = word.find_iter(plan_text).map(|m| m.as_str()).collect();
    let dir = work.orch.join("agents");
    let mut agents = Vec::new();
    for file in list_dir(&dir).into_iter().filter(|n| n.ends_with(".json")) {
        let Some(record) = read_json(&dir.join(&file)) else {
            continue;
        };
        let name = record
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        if is_agent_name(&name) && words.contains(name.as_str()) {
            agents.push(AgentRecord { name, record });
        }
    }
    agents
}

/// LC:343 (retorno de `settled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Parked,
    Ended,
}

/// LC:343-351 (`settled`): `done`/`archived` encerram, `parked` estaciona; senão vale o handoff `STATUS: PARKED` da
/// worktree mais novo que o último prompt.
pub fn settled(record: &Json, reports: &str) -> Option<Settled> {
    match record.get("state").and_then(|v| v.as_str()) {
        Some("done" | "archived") => return Some(Settled::Ended),
        Some("parked") => return Some(Settled::Parked),
        _ => {}
    }
    let handoff = Path::new(&js_string(record.get("worktree")))
        .join(reports)
        .join("handoff.md");
    let text = read_text(&handoff).unwrap_or_default();
    let head = text.split('\n').next().unwrap_or("");
    let prompted = record
        .get("promptedAt")
        .and_then(|v| v.as_str())
        .and_then(parse_iso)
        .map_or(0.0, |ms| ms as f64);
    let stamp = std::fs::metadata(&handoff)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    (head.starts_with("STATUS: PARKED") && stamp > prompted).then_some(Settled::Parked)
}

/// LC:353 (`Readiness`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Parked,
    Ended,
    Working,
}

/// LC:355-362 (`PlanAgent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanAgent {
    pub name: String,
    /// `state` do registro (`"?"` se ausente)
    pub state: String,
    pub readiness: Readiness,
    pub worktree: String,
}

/// LC:364-371 (`planAgents`): prontidão de cada agente ligado, pela mesma regra do `gather`.
pub fn plan_agents(work: &WorkEnv, plan_text: &str) -> Vec<PlanAgent> {
    linked_agents(work, plan_text)
        .into_iter()
        .map(|AgentRecord { name, record }| PlanAgent {
            state: record
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_owned(),
            readiness: match settled(&record, &work.reports) {
                Some(Settled::Parked) => Readiness::Parked,
                Some(Settled::Ended) => Readiness::Ended,
                None => Readiness::Working,
            },
            worktree: js_string(record.get("worktree")),
            name,
        })
        .collect()
}

/// LC:46 (`Plan`): partes, worktrees e achados da guarda.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub parts: Vec<Part>,
    pub worktrees: Vec<WorktreeInfo>,
    pub findings: Vec<Finding>,
}

/// LC:373-381 (`addFile`): arquivo que não existe fica de fora; o que existe entra e passa pela guarda.
fn add_file(
    parts: &mut Vec<Part>,
    findings: &mut Vec<Finding>,
    logical: &str,
    file: &Path,
) -> Result<(), HadesError> {
    if !is_file(file) {
        return Ok(());
    }
    let data = std::fs::read(file)
        .map_err(|e| HadesError::with_code(format!("não li {}: {e}", file.display()), 1))?;
    findings.extend(scan_text(logical, &data));
    parts.push(Part::new(logical, data));
    Ok(())
}

/// LC:383-386 (`GatherOptions`).
#[derive(Debug, Clone)]
pub struct GatherOptions<'a> {
    pub work: &'a WorkEnv,
    pub plan_file: &'a Path,
    pub with_journal: bool,
}

/// LC:388 (retorno de `gather`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gathered {
    pub plan: Plan,
    pub agents: Vec<String>,
    pub approved_sha: Option<String>,
    pub plan_sha: String,
}

/// LC:387-426 (`gather`): junta o que entra no pacote, sem o manifesto. Recusa (código 1), nomeando o agente, se
/// algum não está estacionado ou encerrado.
pub fn gather(exec: &dyn GitExec, options: &GatherOptions) -> Result<Gathered, HadesError> {
    let GatherOptions {
        work,
        plan_file,
        with_journal,
    } = options;
    let plan_name = plan_file.to_string_lossy().into_owned();
    let Some(plan_text) = read_text(plan_file) else {
        return die(format!("plano não encontrado: {plan_name}"), 2, None);
    };
    let linked = linked_agents(work, &plan_text);
    for AgentRecord { name, record } in &linked {
        if settled(record, &work.reports).is_none() {
            let state = match record.get("state") {
                None | Some(serde_json::Value::Null) => "?".to_owned(),
                other => js_string(other),
            };
            return die(
                format!(
                    "{name} ainda não está estacionado (estado {state}): rode hades agent park {name} e espere o handoff; o pack nunca estaciona sozinho"
                ),
                1,
                None,
            );
        }
    }
    let mut parts = Vec::new();
    let mut findings = Vec::new();
    let mut worktrees = Vec::new();
    let stem = basename(&plan_name)
        .strip_suffix(".md")
        .unwrap_or(basename(&plan_name))
        .to_owned();
    add_file(
        &mut parts,
        &mut findings,
        &format!("plan/{}", basename(&plan_name)),
        plan_file,
    )?;
    let log_dir = &plan_name[..plan_name.rfind('/').map_or(0, |i| i + 1)];
    add_file(
        &mut parts,
        &mut findings,
        &format!("plan/{stem}.log.md"),
        Path::new(&format!("{log_dir}{stem}.log.md")),
    )?;
    let approval = read_json(&work.orch.join("plan.json"));
    add_file(
        &mut parts,
        &mut findings,
        "orchestration/plan.json",
        &work.orch.join("plan.json"),
    )?;
    let names: Vec<String> = linked.iter().map(|a| a.name.clone()).collect();
    for name in &names {
        add_file(
            &mut parts,
            &mut findings,
            &format!("orchestration/agents/{name}.json"),
            &work.orch.join("agents").join(format!("{name}.json")),
        )?;
    }
    for dir in ["watch", "consolidations"] {
        for file in list_dir(&work.orch.join(dir)) {
            if names.iter().any(|n| file.starts_with(&format!("{n}."))) {
                add_file(
                    &mut parts,
                    &mut findings,
                    &format!("orchestration/{dir}/{file}"),
                    &work.orch.join(dir).join(&file),
                )?;
            }
        }
    }
    for AgentRecord { name, record } in &linked {
        let worktree = js_string(record.get("worktree"));
        for file in REPORT_FILES {
            add_file(
                &mut parts,
                &mut findings,
                &format!("reports/{name}/{file}"),
                &Path::new(&worktree).join(&work.reports).join(file),
            )?;
        }
        if *with_journal {
            add_file(
                &mut parts,
                &mut findings,
                &format!("journal/{name}.jsonl"),
                &work.work.join("journal").join(format!("{name}.jsonl")),
            )?;
        }
        if record.get("mode").and_then(|v| v.as_str()) == Some("review") || worktree.is_empty() {
            continue;
        }
        let captured = capture_worktree(exec, name, record, &work.reports)?;
        parts.extend(captured.parts);
        findings.extend(captured.findings);
        worktrees.push(captured.info);
    }
    let approved_sha = approval
        .as_ref()
        .filter(|a| a.get("path").and_then(|v| v.as_str()) == Some(plan_name.as_str()))
        .and_then(|a| a.get("sha"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Ok(Gathered {
        plan: Plan {
            parts,
            worktrees,
            findings,
        },
        agents: names,
        approved_sha,
        plan_sha: plan_sha(&plan_text),
    })
}

/// LC:444-456 (`resumePrompt`): prompt de retomada do orquestrador em outra sessão da mesma máquina.
pub fn resume_prompt(plan_file: &str, parked: &[String]) -> String {
    let resumes = if parked.is_empty() {
        "   (nenhum agente estacionado: confira com hades agent list)".to_owned()
    } else {
        parked
            .iter()
            .map(|name| format!("   hades agent resume {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    [
        format!("Você é o orquestrador da Hades retomando a orquestração do plano {plan_file}."),
        "1. Leia só a seção \"Estado atual\" do plano (não o plano inteiro) e a mantenha em dia."
            .to_owned(),
        "2. Rearme o plantão em segundo plano: hades watch --next (com run_in_background)."
            .to_owned(),
        "3. Reabra cada agente estacionado, um por vez, na ordem do plano:".to_owned(),
        resumes,
        "4. Siga o plano: trate cada evento do watch como de costume.".to_owned(),
    ]
    .join("\n")
}

/// LC:458-468 (`PackPreview`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackPreview {
    pub parts: Vec<(String, usize)>,
    /// soma dos tamanhos das partes, antes do gzip e da cifra
    pub raw_bytes: usize,
    pub agents: Vec<String>,
    pub worktrees: Vec<WorktreeInfo>,
    /// o que a guarda barraria: caminho lógico e regra, nunca o valor
    pub blocked: Vec<Finding>,
    pub allowed: Vec<String>,
}

/// LC:470-482 (`previewPack`): o que `caronte pack --dry-run` mostra, como dados. Recusa como o pack se algum agente
/// não está estacionado.
pub fn preview_pack(
    exec: &dyn GitExec,
    options: &GatherOptions,
    allow: &[String],
) -> Result<PackPreview, HadesError> {
    let gathered = gather(exec, options)?;
    let (blocked, allowed) = apply_allow(gathered.plan.findings, allow);
    let parts: Vec<(String, usize)> = gathered
        .plan
        .parts
        .iter()
        .map(|p| (p.path.clone(), p.data.len()))
        .collect();
    Ok(PackPreview {
        raw_bytes: parts.iter().map(|(_, size)| size).sum(),
        parts,
        agents: gathered.agents,
        worktrees: gathered.plan.worktrees,
        blocked,
        allowed,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use serde_json::json;

    use super::*;
    use crate::gitx::SystemGit;
    use crate::guard::Rule;

    const REPORTS: &str = ".design-sync-reports";

    fn secret_value() -> String {
        ["Hds", "!", "abcdef1234"].join("")
    }

    fn sh(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn write(file: &Path, text: &str) {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    struct Rig {
        _root: tempfile::TempDir,
        tree: PathBuf,
        plan: PathBuf,
        work: WorkEnv,
    }

    impl Rig {
        fn agent(&self, record: serde_json::Value) {
            let name = record["name"].as_str().unwrap().to_owned();
            write(
                &self.work.orch.join("agents").join(format!("{name}.json")),
                &record.to_string(),
            );
        }
    }

    fn rig(state: &str, with_handoff: bool) -> Rig {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path().join("trees/fake-a");
        fs::create_dir_all(&tree).unwrap();
        sh(&tree, &["init", "-q", "-b", "main"]);
        write(&tree.join("a.txt"), "um\n");
        write(&tree.join(".gitignore"), "ignorado.log\n");
        sh(&tree, &["add", "-A"]);
        sh(&tree, &["commit", "-q", "-m", "base"]);
        write(&tree.join(REPORTS).join("brief.md"), "# brief\n");
        if with_handoff {
            write(
                &tree.join(REPORTS).join("handoff.md"),
                "STATUS: PARKED — pausa\nfeito: tudo\n",
            );
        }
        let plan = root.path().join("home/.claude/hades/plans/demo.md");
        write(
            &plan,
            "# Plano demo\n\n## Estado atual\n- fake-a trabalhando\n\n## Unidades\n| fake-a | impl |\n",
        );
        write(&plan.with_file_name("demo.log.md"), "histórico\n");
        let work_dir = root.path().join("work/_skill");
        let orch = work_dir.join("orchestration");
        let work = WorkEnv {
            skill_scope: true,
            repo_top: root.path().to_path_buf(),
            main_clone: root.path().to_path_buf(),
            slug: "_skill".into(),
            repo_name: "_skill".into(),
            work: work_dir.clone(),
            locks: work_dir.join("locks"),
            state: work_dir.join("state"),
            orch: orch.clone(),
            skill_orch: orch.clone(),
            reports: REPORTS.into(),
        };
        write(
            &orch.join("plan.json"),
            &json!({"path": plan.to_string_lossy(), "sha": "0".repeat(64)}).to_string(),
        );
        write(&orch.join("watch/fake-a.ctx"), "1");
        write(&orch.join("watch/outro.ctx"), "1");
        write(&work_dir.join("journal/fake-a.jsonl"), "{}\n");
        let rig = Rig {
            _root: root,
            tree: tree.clone(),
            plan,
            work,
        };
        rig.agent(json!({
            "name": "fake-a", "mode": "skill", "role": "impl", "branch": "main", "base": "main",
            "worktree": tree.to_string_lossy(), "state": state, "promptedAt": "2020-01-01T00:00:00Z"
        }));
        rig.agent(json!({"name": "outro", "worktree": "/nada/outro", "state": "working"}));
        rig
    }

    fn opts(rig: &Rig, with_journal: bool) -> GatherOptions<'_> {
        GatherOptions {
            work: &rig.work,
            plan_file: &rig.plan,
            with_journal,
        }
    }

    fn paths(g: &Gathered) -> Vec<&str> {
        g.plan.parts.iter().map(|p| p.path.as_str()).collect()
    }

    #[test]
    fn linked_so_agente_citado_como_palavra() {
        let r = rig("parked", true);
        let names: Vec<_> = linked_agents(&r.work, "fake-a e fake-ab")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["fake-a"]);
        let names: Vec<_> = linked_agents(&r.work, "| fake-a | impl |\n| outro | impl |")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["fake-a", "outro"]);
        assert!(linked_agents(&r.work, "nada").is_empty());
    }

    #[test]
    fn linked_ignora_registro_ilegivel_e_nome_invalido() {
        let r = rig("parked", true);
        write(&r.work.orch.join("agents/quebrado.json"), "{ nao e json");
        write(
            &r.work.orch.join("agents/sem-nome.json"),
            "{\"state\":\"x\"}",
        );
        write(
            &r.work.orch.join("agents/-ruim.json"),
            "{\"name\":\"-ruim\"}",
        );
        write(&r.work.orch.join("agents/nota.txt"), "fake-a");
        let names: Vec<_> = linked_agents(&r.work, "fake-a quebrado sem-nome -ruim")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["fake-a"]);
    }

    #[test]
    fn settled_por_estado() {
        let rec = |state: &str| json!({"state": state}).as_object().cloned().unwrap();
        assert_eq!(settled(&rec("done"), REPORTS), Some(Settled::Ended));
        assert_eq!(settled(&rec("archived"), REPORTS), Some(Settled::Ended));
        assert_eq!(settled(&rec("parked"), REPORTS), Some(Settled::Parked));
        assert_eq!(settled(&rec("working"), "/nada"), None);
    }

    #[test]
    fn settled_handoff_parked_mais_novo_que_o_prompt() {
        let r = rig("working", true);
        let record = |prompted: serde_json::Value| {
            json!({"state": "working", "worktree": r.tree.to_string_lossy(), "promptedAt": prompted})
                .as_object()
                .cloned()
                .unwrap()
        };
        assert_eq!(
            settled(&record(json!("2020-01-01T00:00:00Z")), REPORTS),
            Some(Settled::Parked)
        );
        assert_eq!(
            settled(&record(json!("2999-01-01T00:00:00Z")), REPORTS),
            None
        );
        assert_eq!(
            settled(&record(json!("lixo")), REPORTS),
            Some(Settled::Parked)
        );
        assert_eq!(settled(&record(json!(5)), REPORTS), Some(Settled::Parked));
        write(
            &r.tree.join(REPORTS).join("handoff.md"),
            "STATUS: WORKING\n",
        );
        assert_eq!(
            settled(&record(json!("2020-01-01T00:00:00Z")), REPORTS),
            None
        );
    }

    #[test]
    fn settled_sem_handoff_nao_assenta() {
        let r = rig("working", false);
        let record = fs::read_to_string(r.work.orch.join("agents/fake-a.json")).unwrap();
        let record: Json = serde_json::from_str(&record).unwrap();
        assert_eq!(settled(&record, REPORTS), None);
    }

    #[test]
    fn plan_agents_da_a_prontidao_de_cada_um() {
        let r = rig("working", true);
        let text = fs::read_to_string(&r.plan).unwrap() + "| outro | impl |\n";
        let got = plan_agents(&r.work, &text);
        let listed: Vec<_> = got
            .iter()
            .map(|a| (a.name.as_str(), a.state.as_str(), a.readiness))
            .collect();
        assert_eq!(
            listed,
            [
                ("fake-a", "working", Readiness::Parked),
                ("outro", "working", Readiness::Working)
            ]
        );
        assert_eq!(got[0].worktree, r.tree.to_string_lossy());
    }

    #[test]
    fn gather_recusa_agente_trabalhando_nomeando_o_comando() {
        let r = rig("working", false);
        let err = gather(&SystemGit, &opts(&r, false)).unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.message.contains("fake-a"));
        assert!(err.message.contains("(estado working)"));
        assert!(err.message.contains("hades agent park fake-a"));
    }

    #[test]
    fn gather_recusa_estado_ausente_com_interrogacao() {
        let r = rig("parked", true);
        r.agent(json!({"name": "fake-a", "worktree": "/nada"}));
        let err = gather(&SystemGit, &opts(&r, false)).unwrap_err();
        assert!(err.message.contains("(estado ?)"));
    }

    #[test]
    fn gather_plano_inexistente_sai_2() {
        let r = rig("parked", true);
        let missing = r.plan.with_file_name("nada.md");
        let err = gather(
            &SystemGit,
            &GatherOptions {
                work: &r.work,
                plan_file: &missing,
                with_journal: false,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, 2);
        assert!(err.message.starts_with("plano não encontrado: "));
    }

    #[test]
    fn gather_agente_fora_do_plano_nao_barra_e_encerrado_segue() {
        let r = rig("parked", true);
        assert!(gather(&SystemGit, &opts(&r, false)).is_ok());
        let r = rig("done", false);
        assert!(gather(&SystemGit, &opts(&r, false)).is_ok());
        let r = rig("working", true);
        assert!(gather(&SystemGit, &opts(&r, false)).is_ok());
    }

    #[test]
    fn gather_pacote_real_na_ordem_do_ts() {
        let r = rig("parked", true);
        let tree = &r.tree;
        write(&tree.join("novo.txt"), "do commit\n");
        sh(tree, &["checkout", "-q", "-b", "feat/x"]);
        sh(tree, &["add", "-A"]);
        sh(tree, &["commit", "-q", "-m", "feat: novo"]);
        write(&tree.join("a.txt"), "um\ndois\n");
        write(&tree.join("solto.txt"), "não rastreado\n");
        write(&tree.join("ignorado.log"), "ruído\n");
        r.agent(json!({
            "name": "fake-a", "mode": "skill", "role": "impl", "branch": "feat/x", "base": "main",
            "worktree": tree.to_string_lossy(), "state": "parked"
        }));
        let got = gather(&SystemGit, &opts(&r, true)).unwrap();
        assert_eq!(
            paths(&got),
            [
                "plan/demo.md",
                "plan/demo.log.md",
                "orchestration/plan.json",
                "orchestration/agents/fake-a.json",
                "orchestration/watch/fake-a.ctx",
                "reports/fake-a/brief.md",
                "reports/fake-a/handoff.md",
                "journal/fake-a.jsonl",
                "worktrees/fake-a/commits.bundle",
                "worktrees/fake-a/uncommitted.patch",
            ]
        );
        let patch = got
            .plan
            .parts
            .iter()
            .find(|p| p.path.ends_with("uncommitted.patch"))
            .unwrap();
        let patch = String::from_utf8_lossy(&patch.data);
        assert!(patch.contains("solto.txt"));
        assert!(patch.contains("+dois"));
        assert!(!patch.contains("ignorado.log"));
        assert!(!patch.contains(REPORTS));
        assert_eq!(got.agents, ["fake-a"]);
        let info = &got.plan.worktrees[0];
        assert_eq!(
            (info.branch.as_str(), info.base_ref.as_str()),
            ("feat/x", "main")
        );
        assert_eq!(info.head, sh(tree, &["rev-parse", "HEAD"]));
        assert_eq!(got.approved_sha.as_deref(), Some("0".repeat(64).as_str()));
        assert_eq!(got.plan_sha.len(), 64);
        assert!(sh(tree, &["status", "--porcelain"]).contains("?? solto.txt"));
        let bundle = r._root.path().join("check.bundle");
        let data = &got
            .plan
            .parts
            .iter()
            .find(|p| p.path.ends_with("commits.bundle"))
            .unwrap()
            .data;
        fs::write(&bundle, data).unwrap();
        assert!(sh(tree, &["bundle", "verify", bundle.to_str().unwrap()]).contains(&info.head));
    }

    #[test]
    fn gather_sem_journal_por_padrao() {
        let r = rig("parked", true);
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        assert!(!paths(&got).iter().any(|p| p.starts_with("journal/")));
    }

    #[test]
    fn gather_aprovacao_de_outro_plano_nao_vale() {
        let r = rig("parked", true);
        write(
            &r.work.orch.join("plan.json"),
            &json!({"path": "/outro.md", "sha": "abc"}).to_string(),
        );
        assert_eq!(
            gather(&SystemGit, &opts(&r, false)).unwrap().approved_sha,
            None
        );
        write(
            &r.work.orch.join("plan.json"),
            &json!({"path": r.plan.to_string_lossy(), "sha": 7}).to_string(),
        );
        assert_eq!(
            gather(&SystemGit, &opts(&r, false)).unwrap().approved_sha,
            None
        );
        fs::remove_file(r.work.orch.join("plan.json")).unwrap();
        assert_eq!(
            gather(&SystemGit, &opts(&r, false)).unwrap().approved_sha,
            None
        );
    }

    #[test]
    fn gather_agente_review_fica_sem_worktree() {
        let r = rig("parked", true);
        r.agent(json!({
            "name": "fake-a", "mode": "review", "base": "main",
            "worktree": r.tree.to_string_lossy(), "state": "parked"
        }));
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        assert!(got.plan.worktrees.is_empty());
        assert!(!paths(&got).iter().any(|p| p.starts_with("worktrees/")));
        assert!(paths(&got).contains(&"reports/fake-a/brief.md"));
    }

    #[test]
    fn worktree_limpa_so_com_commits_e_so_com_nao_commitado() {
        let r = rig("parked", true);
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        let info = &got.plan.worktrees[0];
        assert_eq!((&info.bundle, &info.patch), (&None, &None));
        assert_eq!(info.base_sha, info.head);
        assert!(!paths(&got).iter().any(|p| p.starts_with("worktrees/")));

        write(&r.tree.join("solto.txt"), "novo\n");
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        let info = &got.plan.worktrees[0];
        assert_eq!(info.bundle, None);
        assert_eq!(
            info.patch.as_deref(),
            Some("worktrees/fake-a/uncommitted.patch")
        );
        fs::remove_file(r.tree.join("solto.txt")).unwrap();

        write(&r.tree.join("b.txt"), "dois\n");
        sh(&r.tree, &["checkout", "-q", "-b", "feat/y"]);
        sh(&r.tree, &["add", "-A"]);
        sh(&r.tree, &["commit", "-q", "-m", "feat: b"]);
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        let info = &got.plan.worktrees[0];
        assert_eq!(
            info.bundle.as_deref(),
            Some("worktrees/fake-a/commits.bundle")
        );
        assert_eq!(info.patch, None);
    }

    #[test]
    fn pasta_de_relatorios_no_gitignore_sai_do_patch_sem_falhar() {
        let r = rig("parked", true);
        write(
            &r.tree.join(".gitignore"),
            &format!("ignorado.log\n{REPORTS}/\n"),
        );
        sh(&r.tree, &["add", ".gitignore"]);
        sh(&r.tree, &["commit", "-q", "-m", "chore: ignora relatórios"]);
        write(&r.tree.join("solto.txt"), "novo\n");
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        let patch = got
            .plan
            .parts
            .iter()
            .find(|p| p.path.ends_with("uncommitted.patch"))
            .unwrap();
        let patch = String::from_utf8_lossy(&patch.data);
        assert!(patch.contains("solto.txt"));
        assert!(!patch.contains(REPORTS));
        assert!(paths(&got).contains(&"reports/fake-a/brief.md"));
    }

    #[test]
    fn guarda_barra_env_e_credenciais_no_nao_commitado() {
        let r = rig("parked", true);
        write(&r.tree.join(".env"), &format!("TOKEN={}\n", secret_value()));
        write(&r.tree.join("docs/credenciais.md"), "usuario: x\n");
        write(&r.tree.join(".env.example"), "TOKEN=<token>\n");
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        let (blocked, allowed) = apply_allow(got.plan.findings, &[]);
        let names: Vec<_> = blocked.iter().map(|f| f.path.as_str()).collect();
        assert!(names.contains(&"worktrees/fake-a/.env"));
        assert!(names.contains(&"worktrees/fake-a/docs/credenciais.md"));
        assert!(!names.iter().any(|n| n.contains(".env.example")));
        assert!(allowed.is_empty());
        assert!(!format!("{blocked:?}").contains(&secret_value()));
    }

    #[test]
    fn guarda_barra_segredo_no_conteudo_de_um_commit() {
        let r = rig("parked", true);
        write(
            &r.tree.join("config.txt"),
            &format!("senha={}\n", secret_value()),
        );
        sh(&r.tree, &["checkout", "-q", "-b", "feat/z"]);
        sh(&r.tree, &["add", "-A"]);
        sh(&r.tree, &["commit", "-q", "-m", "x"]);
        let got = gather(&SystemGit, &opts(&r, false)).unwrap();
        assert!(got.plan.findings.contains(&Finding {
            path: "worktrees/fake-a/config.txt".into(),
            rule: Rule::Conteudo
        }));
    }

    #[test]
    fn preview_lista_barrado_no_handoff_e_libera_com_allow() {
        let r = rig("parked", true);
        write(
            &r.tree.join(REPORTS).join("handoff.md"),
            &format!("STATUS: PARKED — x\n{}\n", secret_value()),
        );
        let preview = preview_pack(&SystemGit, &opts(&r, false), &[]).unwrap();
        assert_eq!(
            preview.blocked,
            [Finding {
                path: "reports/fake-a/handoff.md".into(),
                rule: Rule::Conteudo
            }]
        );
        assert!(preview.allowed.is_empty());
        assert!(!format!("{preview:?}").contains(&secret_value()));
        let allow = ["reports/fake-a/handoff.md".to_owned()];
        let preview = preview_pack(&SystemGit, &opts(&r, false), &allow).unwrap();
        assert!(preview.blocked.is_empty());
        assert_eq!(preview.allowed, allow);
        assert_eq!(
            preview.raw_bytes,
            preview.parts.iter().map(|(_, s)| s).sum::<usize>()
        );
        assert_eq!(preview.agents, ["fake-a"]);
    }

    #[test]
    fn preview_recusa_como_o_pack() {
        let r = rig("working", false);
        assert_eq!(
            preview_pack(&SystemGit, &opts(&r, false), &[])
                .unwrap_err()
                .code,
            1
        );
    }

    #[test]
    fn resume_prompt_lista_so_os_estacionados() {
        let text = resume_prompt("/p/demo.md", &["fake-a".to_owned(), "b".to_owned()]);
        let lines: Vec<_> = text.split('\n').collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(
            lines[0],
            "Você é o orquestrador da Hades retomando a orquestração do plano /p/demo.md."
        );
        assert_eq!(lines[4], "   hades agent resume fake-a");
        assert_eq!(lines[5], "   hades agent resume b");
        assert!(text.contains("hades watch --next"));
        assert!(text.contains("\"Estado atual\""));
        let empty = resume_prompt("/p", &[]);
        assert!(empty.contains("(nenhum agente estacionado: confira com hades agent list)"));
        assert!(!empty.contains("hades agent resume"));
    }
}
