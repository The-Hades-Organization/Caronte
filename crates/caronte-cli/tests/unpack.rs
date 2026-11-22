//! `caronte unpack` de ponta a ponta: ida e volta pack → unpack (os dois do binário Rust) entre duas raízes de
//! trabalho, dois diretórios de planos e dois de worktrees, as recusas (nada é tocado), o dry-run e o desfazer depois
//! de uma falha no meio. Porta de TU:99-360.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use caronte_core::format::{Part, decrypt_package, encrypt_package, sha256};
use caronte_core::manifest::{Manifest, manifest_part, verify_parts};
use tempfile::TempDir;

const PASSWORD: &str = "senha de teste 123";
const LOG2N: u8 = 14;
const REPORTS: &str = ".design-sync-reports";
const BINARY: [u8; 8] = [0, 1, 2, 3, 255, 254, 0, 9];

struct Rig {
    _dir: TempDir,
    root: PathBuf,
}

impl Rig {
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn work(&self) -> PathBuf {
        self.root.join("work/_skill")
    }
    fn work2(&self) -> PathBuf {
        self.root.join("work2")
    }
    fn plan(&self) -> PathBuf {
        self.home().join(".claude/hades/plans/demo.md")
    }
    fn plans2(&self) -> PathBuf {
        self.root.join("plans2")
    }
    fn tree(&self) -> PathBuf {
        self.root.join("trees/fake-a")
    }
    fn trees2(&self) -> PathBuf {
        self.root.join("trees2")
    }
    fn dest_tree(&self) -> PathBuf {
        self.trees2().join("fake-a")
    }
    fn dest_clone(&self) -> PathBuf {
        self.root.join("dest-clone")
    }
    fn file(&self) -> PathBuf {
        self.root.join("out/demo.caronte")
    }

    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_caronte"));
        cmd.args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", std::env::var("PATH").unwrap())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HADES_WORK_ROOT", self.root.join("work"))
            .env("HADES_SKILL_DIR", self.root.join("skill"))
            .env("HADES_NOW", "2026-11-20T12:00:00Z")
            .env("HADES_CARONTE_SCRYPT_LOG2N", LOG2N.to_string())
            .env("HADES_CARONTE_PASSWORD", PASSWORD);
        for (key, value) in extra {
            cmd.env(key, value);
        }
        cmd.output().unwrap()
    }

    fn unpack_args(&self, file: &Path, extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = ["unpack", file.to_str().unwrap()]
            .into_iter()
            .map(str::to_owned)
            .collect();
        for (flag, dir) in [
            ("--work-root", self.work2()),
            ("--plans", self.plans2()),
            ("--worktrees", self.trees2()),
            ("--clone", self.dest_clone()),
        ] {
            args.push(flag.to_owned());
            args.push(dir.to_string_lossy().into_owned());
        }
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        args
    }

    fn unpack(&self, extra: &[&str], env: &[(&str, &str)]) -> Output {
        let args = self.unpack_args(&self.file(), extra);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs, env)
    }

    fn repack(&self, mutate: impl FnOnce(&mut Vec<Part>, &mut Manifest)) {
        let parts = decrypt_package(&fs::read(self.file()).unwrap(), PASSWORD).unwrap();
        let mut manifest = verify_parts(&parts).unwrap();
        let mut rest: Vec<Part> = parts.into_iter().skip(1).collect();
        mutate(&mut rest, &mut manifest);
        let mut all = vec![manifest_part(&manifest)];
        all.extend(rest);
        fs::remove_file(self.file()).unwrap();
        fs::write(self.file(), encrypt_package(&all, PASSWORD, LOG2N).unwrap()).unwrap();
    }

    fn untouched(&self) {
        assert!(!self.work2().exists(), "work2 criado");
        assert!(!self.plans2().exists(), "plans2 criado");
        assert!(!self.dest_tree().exists(), "worktree criada");
        assert_eq!(sh(&self.dest_clone(), &["branch", "--list", "feat/x"]), "");
    }
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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(file: &Path, data: impl AsRef<[u8]>) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, data).unwrap();
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(file: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
}

/// Origem: clone com a base, worktree com commit próprio, alteração não commitada, arquivo não rastreado e
/// binário; destino: clone da base.
fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let rig = Rig { _dir: dir, root };
    let src = rig.root.join("src-clone");
    fs::create_dir_all(&src).unwrap();
    sh(&src, &["init", "-q", "-b", "main"]);
    write(&src.join("a.txt"), "um\n");
    write(
        &src.join(".gitignore"),
        format!("ignorado.log\n{REPORTS}/\n"),
    );
    sh(&src, &["add", "-A"]);
    sh(&src, &["commit", "-q", "-m", "base"]);
    sh(
        &rig.root,
        &[
            "clone",
            "-q",
            src.to_str().unwrap(),
            rig.dest_clone().to_str().unwrap(),
        ],
    );
    let tree = rig.tree();
    fs::create_dir_all(tree.parent().unwrap()).unwrap();
    sh(
        &src,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            tree.to_str().unwrap(),
            "main",
        ],
    );
    write(&tree.join("novo.txt"), "do commit\n");
    sh(&tree, &["add", "-A"]);
    sh(&tree, &["commit", "-q", "-m", "feat: novo"]);
    write(&tree.join("a.txt"), "um\ndois\n");
    write(&tree.join("solto.txt"), "não rastreado\n");
    write(&tree.join("bin.dat"), BINARY);
    write(&tree.join("ignorado.log"), "ruído\n");
    write(&tree.join(REPORTS).join("brief.md"), "# brief\n");
    write(
        &tree.join(REPORTS).join("handoff.md"),
        "STATUS: PARKED — pausa\nfeito: tudo\n",
    );
    write(&rig.root.join("skill/VERSION"), "1.2.3\n");
    write(
        &rig.plan(),
        "# Plano demo\n\n## Estado atual\n- fake-a estacionado\n\n## Unidades\n| fake-a | impl |\n",
    );
    write(&rig.plan().with_file_name("demo.log.md"), "histórico\n");
    let plan = rig.plan();
    write(
        &rig.work().join("orchestration/plan.json"),
        serde_json::json!({"path": plan, "sha": "0".repeat(64), "approvedAt": "2026-10-29T10:00:00Z"}).to_string(),
    );
    write(
        &rig.work().join("orchestration/agents/fake-a.json"),
        serde_json::json!({
            "name": "fake-a", "mode": "skill", "role": "impl", "branch": "feat/x", "base": "main",
            "worktree": tree, "pane": "p1", "state": "parked",
        })
        .to_string(),
    );
    write(
        &rig.work().join("orchestration/watch/fake-a.ctx"),
        format!("{}\n", tree.display()),
    );
    let file = rig.file();
    let packed = rig.run(
        &[
            "pack",
            plan.to_str().unwrap(),
            "--out",
            file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&packed), 0, "pack falhou: {}", stderr(&packed));
    rig
}

fn patch_part(parts: &mut [Part], manifest: &mut Manifest, data: &str) {
    let path = "worktrees/fake-a/uncommitted.patch";
    let part = parts
        .iter_mut()
        .find(|p| p.path == path)
        .expect("patch ausente");
    part.data = data.as_bytes().to_vec();
    let entry = manifest.parts.iter_mut().find(|p| p.path == path).unwrap();
    entry.sha256 = sha256(&part.data);
}

#[test]
fn ida_e_volta_confere_head_diff_nao_rastreado_e_relatorios() {
    let r = rig();
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let dest = r.dest_tree();
    assert_eq!(
        sh(&dest, &["rev-parse", "HEAD"]),
        sh(&r.tree(), &["rev-parse", "HEAD"])
    );
    assert_eq!(sh(&dest, &["branch", "--show-current"]), "feat/x");
    assert_eq!(sh(&dest, &["diff"]), sh(&r.tree(), &["diff"]));
    assert_eq!(
        fs::read_to_string(dest.join("solto.txt")).unwrap(),
        "não rastreado\n"
    );
    assert_eq!(fs::read(dest.join("bin.dat")).unwrap(), BINARY);
    assert!(!dest.join("ignorado.log").exists());
    assert!(
        fs::read_to_string(dest.join(REPORTS).join("handoff.md"))
            .unwrap()
            .contains("STATUS: PARKED")
    );
    assert_eq!(
        fs::read_to_string(dest.join(REPORTS).join("brief.md")).unwrap(),
        "# brief\n"
    );
    assert_eq!(
        fs::read_to_string(r.plans2().join("demo.md")).unwrap(),
        fs::read_to_string(r.plan()).unwrap()
    );
    assert!(r.plans2().join("demo.log.md").exists());

    let agent = r.work2().join("_skill/orchestration/agents/fake-a.json");
    let record = json(&agent);
    assert_eq!(record["state"], "parked");
    assert_eq!(record["worktree"], dest.to_str().unwrap());
    assert!(record.get("pane").is_none());
    let keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "name", "mode", "role", "branch", "base", "worktree", "state"
        ]
    );
    assert_eq!(
        fs::read_to_string(r.work2().join("_skill/orchestration/watch/fake-a.ctx")).unwrap(),
        format!("{}\n", dest.display())
    );
    let approval = json(&r.work2().join("_skill/orchestration/plan.json"));
    assert_eq!(
        approval["path"],
        r.plans2().join("demo.md").to_str().unwrap()
    );
    assert!(!r.work2().join("_skill/journal").exists());
}

#[test]
fn aprovacao_que_nao_confere_avisa_do_plan_approve() {
    let r = rig();
    assert!(stderr(&r.unpack(&[], &[])).contains("plan-approve"));
}

#[test]
fn fim_do_unpack_traz_start_adopt_resume_e_prompt() {
    let r = rig();
    let out = stdout(&r.unpack(&[], &[]));
    assert!(out.contains("herdr agent start fake-a"), "{out}");
    assert!(out.contains("hades agent adopt fake-a && hades agent resume fake-a"));
    assert!(out.contains("hades watch --next"));
    assert!(out.contains(r.plans2().join("demo.md").to_str().unwrap()));
    assert!(out.contains("unpack concluído: 1 worktree(s), 1 agente(s), plano "));
}

#[test]
fn worktree_limpa_e_sem_commit_nasce_na_base() {
    let r = rig();
    let tree = r.tree();
    sh(&tree, &["checkout", "-q", "--", "a.txt"]);
    fs::remove_file(tree.join("solto.txt")).unwrap();
    fs::remove_file(tree.join("bin.dat")).unwrap();
    sh(&tree, &["reset", "-q", "--hard", "main"]);
    let clean = r.root.join("out/clean.caronte");
    let plan = r.plan();
    let packed = r.run(
        &[
            "pack",
            plan.to_str().unwrap(),
            "--out",
            clean.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&packed), 0, "{}", stderr(&packed));
    let args = r.unpack_args(&clean, &[]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = r.run(&refs, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        sh(&r.dest_tree(), &["rev-parse", "HEAD"]),
        sh(&r.dest_clone(), &["rev-parse", "main"])
    );
    assert_eq!(sh(&r.dest_tree(), &["status", "--porcelain"]), "");
}

#[test]
fn dry_run_mostra_o_mapa_e_nao_escreve() {
    let r = rig();
    let out = r.unpack(&["--dry-run"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(&format!(
        "{}  →  {}",
        r.tree().display(),
        r.dest_tree().display()
    )));
    assert!(text.contains(&format!(
        "{}  →  {}",
        r.work().display(),
        r.work2().join("_skill").display()
    )));
    assert!(text.contains("nada foi gravado"));
    r.untouched();
}

#[test]
fn dry_run_sem_senha_e_sem_tty_pede_a_senha_e_para() {
    let r = rig();
    let out = r.unpack(&["--dry-run"], &[("HADES_CARONTE_PASSWORD", "")]);
    assert_ne!(code(&out), 0);
    assert!(stderr(&out).contains("HADES_CARONTE_PASSWORD"));
}

#[test]
fn pacote_inexistente_sai_2() {
    let r = rig();
    let out = r.run(&["unpack", "nao-existe.caronte"], &[]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("pacote não encontrado:"));
}

#[test]
fn senha_errada_recusa() {
    let r = rig();
    let out = r.unpack(&[], &[("HADES_CARONTE_PASSWORD", "outra senha")]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("senha errada ou pacote adulterado"));
    r.untouched();
}

#[test]
fn parte_adulterada_recusa_nomeando_a_parte() {
    let r = rig();
    r.repack(|parts, _| {
        parts
            .iter_mut()
            .find(|p| p.path == "plan/demo.md")
            .unwrap()
            .data = b"outro texto".to_vec();
    });
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("sha256 não confere: plan/demo.md"));
    r.untouched();
}

#[test]
fn format_maior_que_o_do_leitor_recusa() {
    let r = rig();
    r.repack(|_, manifest| manifest.format = 2);
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("mais novo"));
    r.untouched();
}

#[test]
fn skill_version_de_outro_major_recusa() {
    let r = rig();
    r.repack(|_, manifest| manifest.skill_version = "99.0.0".into());
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("major diferente"));
    r.untouched();
}

#[test]
fn skill_version_com_outro_minor_avisa_e_segue() {
    let r = rig();
    r.repack(|_, manifest| manifest.skill_version = "1.99.0".into());
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).contains("minor diferente"));
}

#[test]
fn plano_existente_com_outro_sha_recusa() {
    let r = rig();
    write(&r.plans2().join("demo.md"), "# outro plano\n");
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("já existe com outro conteúdo"));
    assert_eq!(
        fs::read_to_string(r.plans2().join("demo.md")).unwrap(),
        "# outro plano\n"
    );
    assert!(!r.dest_tree().exists());
    assert!(!r.work2().exists());
}

#[test]
fn plano_existente_com_o_mesmo_sha_segue_sem_sobrescrever() {
    let r = rig();
    let edited = fs::read_to_string(r.plan())
        .unwrap()
        .replace("- fake-a estacionado", "- estado só deste lado");
    write(&r.plans2().join("demo.md"), &edited);
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(r.plans2().join("demo.md")).unwrap(),
        edited
    );
}

#[test]
fn worktree_de_destino_existente_recusa_sem_criar_mais_nada() {
    let r = rig();
    fs::create_dir_all(r.dest_tree()).unwrap();
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("já existe"));
    assert!(!r.work2().exists());
    assert!(!r.plans2().exists());
    assert_eq!(fs::read_dir(r.dest_tree()).unwrap().count(), 0);
}

#[test]
fn branch_existente_no_clone_recusa() {
    let r = rig();
    sh(&r.dest_clone(), &["branch", "feat/x"]);
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("a branch feat/x já existe"));
    assert!(!r.work2().exists());
    assert!(!r.dest_tree().exists());
}

#[test]
fn registro_de_agente_existente_recusa_sem_criar_a_worktree() {
    let r = rig();
    write(
        &r.work2().join("_skill/orchestration/agents/fake-a.json"),
        "{}",
    );
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("já existe no destino"));
    assert!(!r.dest_tree().exists());
    assert!(!r.plans2().exists());
}

#[test]
fn clone_ausente_sem_clone_recusa_com_o_git_clone_do_remote() {
    let r = rig();
    let gone = r.root.join("nao-existe");
    r.repack(|_, manifest| {
        manifest.repo.remote = "https://example.test/org/projeto.git".into();
        manifest.repo.clone = gone.to_string_lossy().into_owned();
    });
    let file = r.file();
    let out = r.run(
        &[
            "unpack",
            file.to_str().unwrap(),
            "--work-root",
            r.work2().to_str().unwrap(),
            "--plans",
            r.plans2().to_str().unwrap(),
            "--worktrees",
            r.trees2().to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("git clone https://example.test/org/projeto.git"));
    r.untouched();
}

#[test]
fn base_fora_do_clone_e_sem_origin_recusa_com_o_comando_de_fetch() {
    let r = rig();
    fs::remove_dir_all(r.dest_clone()).unwrap();
    fs::create_dir_all(r.dest_clone()).unwrap();
    sh(&r.dest_clone(), &["init", "-q", "-b", "main"]);
    sh(
        &r.dest_clone(),
        &["commit", "-q", "--allow-empty", "-m", "outro"],
    );
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains(&format!(
        "git -C {} fetch origin main",
        r.dest_clone().display()
    )));
    assert!(!r.dest_tree().exists());
    assert!(!r.work2().exists());
    assert_eq!(sh(&r.dest_clone(), &["branch", "--list", "feat/x"]), "");
}

#[test]
fn clone_que_nao_e_repositorio_recusa() {
    let r = rig();
    let file = r.file();
    let not_repo = r.root.join("out");
    let out = r.run(
        &[
            "unpack",
            file.to_str().unwrap(),
            "--clone",
            not_repo.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("não é um repositório git"));
}

fn field(r: &Rig, name: &str, value: &str) {
    r.repack(|_, manifest| {
        let entry = &mut manifest.worktrees[0];
        match name {
            "baseSha" => entry.base_sha = value.into(),
            "head" => entry.head = value.into(),
            _ => entry.base_ref = value.into(),
        }
    });
}

#[test]
fn campos_do_manifesto_que_viram_argumento_do_git_recusam_nomeando_o_agente() {
    for (name, value, message) in [
        ("baseSha", "--upload-pack=x", "fake-a: baseSha inválido"),
        ("head", "main", "fake-a: head inválido"),
        ("baseRef", "--upload-pack=x", "fake-a: baseRef inválido"),
        ("baseRef", "a..b", "fake-a: baseRef inválido"),
    ] {
        let r = rig();
        field(&r, name, value);
        let out = r.unpack(&[], &[]);
        assert_eq!(code(&out), 1, "{name}={value}");
        assert!(
            stderr(&out).contains(message),
            "{name}={value}: {}",
            stderr(&out)
        );
        r.untouched();
    }
}

#[test]
fn patch_que_nao_aplica_desfaz_plano_worktree_e_branch() {
    let r = rig();
    r.repack(|parts, manifest| {
        patch_part(
            parts,
            manifest,
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -9,1 +9,1 @@\n-nada\n+nunca\n",
        );
    });
    let out = r.unpack(&[], &[]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("aplicar uncommitted.patch"));
    r.untouched();
    assert!(!sh(&r.dest_clone(), &["worktree", "list"]).contains("trees2"));
}

#[test]
fn falha_no_meio_com_plano_preexistente_deixa_plano_e_clone_como_estavam() {
    let r = rig();
    let same = fs::read_to_string(r.plan()).unwrap();
    write(&r.plans2().join("demo.md"), &same);
    r.repack(|parts, manifest| patch_part(parts, manifest, "lixo\n"));
    assert_eq!(code(&r.unpack(&[], &[])), 1);
    assert_eq!(
        fs::read_to_string(r.plans2().join("demo.md")).unwrap(),
        same
    );
    assert!(!r.dest_tree().exists());
    assert_eq!(sh(&r.dest_clone(), &["branch", "--list", "feat/x"]), "");
}
