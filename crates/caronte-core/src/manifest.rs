//! `manifest.json` do pacote: o contrato com o unpack. Porta de LC:47-91, 202-215 e 428-441: campo novo só é
//! acrescentado, e `format` sobe só em quebra. A ordem dos campos dos structs é a de chaves do TS.

use hades_core::jsonfmt::json_indent;
use serde::{Deserialize, Serialize};

use crate::format::{CaronteError, CaronteErrorCode, FORMAT_VERSION, Part, sha256};

/// LC:58-71 (`WorktreeInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub agent: String,
    pub path: String,
    pub branch: String,
    pub head: String,
    pub base_ref: String,
    pub base_sha: String,
    pub bundle: Option<String>,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub remote: String,
    pub clone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRef {
    pub path: String,
    pub name: String,
    pub approved_sha: Option<String>,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Skill,
    Project,
}

/// LC:73-87 (`Manifest`). Ordem de chaves: `format, skillVersion, workRoot, scope, repo, plan, agents, worktrees,
/// withJournal, allowed, createdAt, parts` (LC:428-441).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: i64,
    pub skill_version: String,
    pub work_root: String,
    pub scope: Scope,
    pub repo: Repo,
    pub plan: PlanRef,
    pub agents: Vec<String>,
    pub worktrees: Vec<WorktreeInfo>,
    pub with_journal: bool,
    pub allowed: Vec<String>,
    pub created_at: String,
    pub parts: Vec<PartEntry>,
}

/// Campos de entrada de `build_manifest`: tudo menos `format`, `createdAt` e `parts` (LC:428).
#[derive(Debug, Clone)]
pub struct ManifestInput {
    pub skill_version: String,
    pub work_root: String,
    pub scope: Scope,
    pub repo: Repo,
    pub plan: PlanRef,
    pub agents: Vec<String>,
    pub worktrees: Vec<WorktreeInfo>,
    pub with_journal: bool,
    pub allowed: Vec<String>,
}

/// LC:428-441. `created_at` já vem como ISO até o segundo mais `Z` (`2026-11-20T12:00:00Z`).
pub fn build_manifest(input: ManifestInput, parts: &[Part], created_at: &str) -> Manifest {
    Manifest {
        format: FORMAT_VERSION,
        skill_version: input.skill_version,
        work_root: input.work_root,
        scope: input.scope,
        repo: input.repo,
        plan: input.plan,
        agents: input.agents,
        worktrees: input.worktrees,
        with_journal: input.with_journal,
        allowed: input.allowed,
        created_at: created_at.to_owned(),
        parts: parts
            .iter()
            .map(|p| PartEntry {
                path: p.path.clone(),
                size: p.data.len() as u64,
                sha256: sha256(&p.data),
            })
            .collect(),
    }
}

impl Manifest {
    /// `JSON.stringify(_, null, 2) + "\n"` (LC:443-445).
    pub fn to_json(&self) -> String {
        let value = serde_json::to_value(self).expect("manifesto serializa");
        format!("{}\n", json_indent(&value, 2))
    }
}

/// `manifest.json` como primeira parte do pacote (LC:443-445).
pub fn manifest_part(manifest: &Manifest) -> Part {
    Part::new("manifest.json", manifest.to_json())
}

/// Confere cada parte contra o `manifest.json` e devolve o manifesto lido (LC:202-215). Manifesto que não é JSON
/// vira `tampered` (o TS lança exceção crua).
pub fn verify_parts(parts: &[Part]) -> Result<Manifest, CaronteError> {
    let tampered = |msg: String| CaronteError::new(CaronteErrorCode::Tampered, msg);
    let first = parts
        .first()
        .filter(|p| p.path == "manifest.json")
        .ok_or_else(|| tampered("manifest.json ausente do pacote".into()))?;
    let manifest: Manifest = serde_json::from_slice(&first.data)
        .map_err(|_| tampered("manifest.json sem a lista de partes".into()))?;
    for entry in &manifest.parts {
        let part = parts
            .iter()
            .find(|p| p.path == entry.path)
            .ok_or_else(|| tampered(format!("parte ausente: {}", entry.path)))?;
        if sha256(&part.data) != entry.sha256 {
            return Err(tampered(format!("sha256 não confere: {}", entry.path)));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{decrypt_package, encrypt_package};

    const PASSWORD: &str = "senha de teste 123";

    fn input() -> ManifestInput {
        ManifestInput {
            skill_version: "6.56.0".into(),
            work_root: "/w/_skill".into(),
            scope: Scope::Skill,
            repo: Repo {
                remote: "https://github.com/acme/demo.git".into(),
                clone: "/c".into(),
            },
            plan: PlanRef {
                path: "/p/demo.md".into(),
                name: "demo".into(),
                approved_sha: None,
                sha: "ab".into(),
            },
            agents: vec!["fake-a".into()],
            worktrees: vec![],
            with_journal: false,
            allowed: vec![],
        }
    }

    #[test]
    fn manifesto_sai_com_a_ordem_de_chaves_do_ts_e_newline_final() {
        let parts = vec![Part::new("plan/demo.md", "# x\n")];
        let json = build_manifest(input(), &parts, "2026-11-20T12:00:00Z").to_json();
        let keys: Vec<&str> = json
            .lines()
            .filter(|l| l.starts_with("  \""))
            .map(|l| l.split('"').nth(1).unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "format",
                "skillVersion",
                "workRoot",
                "scope",
                "repo",
                "plan",
                "agents",
                "worktrees",
                "withJournal",
                "allowed",
                "createdAt",
                "parts"
            ]
        );
        assert!(json.ends_with("}\n"));
        assert!(json.contains("\"approvedSha\": null"));
        assert!(json.contains("\"worktrees\": []"));
    }

    #[test]
    fn roundtrip_com_manifesto_e_partes_verifica() {
        let body = vec![
            Part::new("plan/demo.md", "# x\n"),
            Part::new("reports/a/brief.md", "b"),
        ];
        let manifest = build_manifest(input(), &body, "2026-11-20T12:00:00Z");
        let mut all = vec![manifest_part(&manifest)];
        all.extend(body);
        let back =
            decrypt_package(&encrypt_package(&all, PASSWORD, 14).unwrap(), PASSWORD).unwrap();
        assert_eq!(verify_parts(&back).unwrap(), manifest);
    }

    #[test]
    fn parte_adulterada_ou_ausente_ou_sem_manifesto_vira_tampered() {
        let body = vec![Part::new("a", "1")];
        let manifest = build_manifest(input(), &body, "2026-11-20T12:00:00Z");
        let mut all = vec![manifest_part(&manifest), Part::new("a", "2")];
        assert!(
            verify_parts(&all)
                .unwrap_err()
                .message
                .contains("sha256 não confere: a")
        );
        all.pop();
        assert!(
            verify_parts(&all)
                .unwrap_err()
                .message
                .contains("parte ausente: a")
        );
        assert!(
            verify_parts(&all[1..])
                .unwrap_err()
                .message
                .contains("manifest.json ausente")
        );
        assert!(verify_parts(&[Part::new("manifest.json", "nao json")]).is_err());
    }

    #[test]
    fn abre_o_pacote_gerado_pelo_ts_embutido() {
        // Fixture `tests/fixtures/ts.caronte`: gerada pelo `hades caronte pack` do TS (Hades-Skill f831727, bun)
        // numa cópia da skill em /tmp com remoto fictício acme/skill e `new Date()` do pack fixo em
        // 2026-11-20T12:00:00Z (o `HADES_NOW` do caronte ainda não existe). HOME de sandbox com dados fictícios:
        // plano `demo`, agente `fake-a` estacionado, worktree com um commit `base`. HADES_CARONTE_SCRYPT_LOG2N=14
        // e a senha de teste "senha de teste 123".
        let file = include_bytes!("../tests/fixtures/ts.caronte");
        let parts = decrypt_package(file, PASSWORD).unwrap();
        let manifest = verify_parts(&parts).unwrap();
        assert_eq!(manifest.format, 1);
        assert_eq!(manifest.created_at, "2026-11-20T12:00:00Z");
        assert_eq!(manifest.plan.name, "demo");
        assert_eq!(manifest.agents, ["fake-a"]);
        assert_eq!(manifest.scope, Scope::Skill);
        assert_eq!(manifest.repo.remote, "https://github.com/acme/skill.git");
        let names: Vec<&str> = parts.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(names[0], "manifest.json");
        assert!(names.contains(&"plan/demo.md"));
        assert!(names.contains(&"reports/fake-a/handoff.md"));
        assert_eq!(
            parts[0].data,
            manifest.to_json().into_bytes(),
            "o to_json reproduz o manifesto do TS byte a byte"
        );
    }
}
