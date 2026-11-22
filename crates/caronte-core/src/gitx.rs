//! `git` do Caronte: patch do não commitado e bundle do não publicado, atrás de uma trait de executor para os
//! testes. Porta de LC:256-321.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use hades_core::output::{HadesError, die};
use serde_json::Value;

use crate::format::Part;
use crate::guard::{Finding, scan_diff};
use crate::manifest::WorktreeInfo;

/// LC:30 (`GIT_MAX_BUFFER`): teto da saída capturada.
pub const GIT_MAX_BUFFER: usize = 1 << 30;
const STDERR_KEEP: usize = 1 << 20;

/// LC:256-260: o que o `git` devolveu. Falha ao iniciar e estouro do teto dão `status` 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOut {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Executor de `git -C <dir> <args>`. `env` acrescenta variáveis às herdadas; `input` vai para o stdin.
pub trait GitExec {
    fn git(&self, dir: &Path, args: &[&str], env: &[(&str, &str)], input: Option<&[u8]>) -> GitOut;
}

/// Executor real: o `git` do PATH.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemGit;

impl GitExec for SystemGit {
    fn git(&self, dir: &Path, args: &[&str], env: &[(&str, &str)], input: Option<&[u8]>) -> GitOut {
        let failed = |stderr: String| GitOut {
            status: 1,
            stdout: Vec::new(),
            stderr,
        };
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(dir)
            .args(args)
            .envs(env.iter().copied())
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return failed(e.to_string()),
        };
        let writer = child
            .stdin
            .take()
            .zip(input.map(<[u8]>::to_vec))
            .map(|(mut pipe, data)| {
                thread::spawn(move || {
                    let _ = pipe.write_all(&data);
                })
            });
        let mut stdout_pipe = child.stdout.take().expect("stdout piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr piped");
        let errs = thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stderr_pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let room = STDERR_KEEP.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
            kept
        });
        let mut stdout = Vec::new();
        let read = (&mut stdout_pipe)
            .take(GIT_MAX_BUFFER as u64 + 1)
            .read_to_end(&mut stdout);
        let exceeded = stdout.len() > GIT_MAX_BUFFER;
        if exceeded || read.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().ok().and_then(|s| s.code());
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        let stderr = errs.join().unwrap_or_default();
        GitOut {
            status: if exceeded { 1 } else { status.unwrap_or(1) },
            stdout: if exceeded { Vec::new() } else { stdout },
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }
}

/// LC:262 (`gitText`): stdout aparado; vazio em qualquer falha.
pub fn git_text(exec: &dyn GitExec, dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&exec.git(dir, args, &[], None).stdout)
        .trim()
        .to_owned()
}

/// LC:264-286 (`uncommittedPatch`): tracked mais não rastreados, sem ignorados e sem a pasta de relatórios, num
/// índice temporário (o índice da worktree não é tocado).
pub fn uncommitted_patch(
    exec: &dyn GitExec,
    worktree: &Path,
    reports: &str,
) -> Result<Vec<u8>, HadesError> {
    let shown = worktree.display();
    let scratch = tempfile::Builder::new()
        .prefix("caronte-index-")
        .tempdir()
        .map_err(|e| HadesError::with_code(format!("não criei o índice temporário: {e}"), 1))?;
    let index = scratch.path().join("index");
    let index = index.to_string_lossy();
    let env = [("GIT_INDEX_FILE", index.as_ref())];
    if exec
        .git(worktree, &["read-tree", "HEAD"], &env, None)
        .status
        != 0
    {
        return die(format!("git read-tree falhou em {shown}"), 1, None);
    }
    let folder = format!("{reports}/");
    let ignored = exec
        .git(worktree, &["check-ignore", "-q", "--", &folder], &[], None)
        .status
        == 0;
    let exclude = format!(":(exclude){reports}");
    let mut add = vec!["add", "-A", "--", "."];
    if !ignored {
        add.push(&exclude);
    }
    if exec.git(worktree, &add, &env, None).status != 0 {
        return die(format!("git add -A falhou em {shown}"), 1, None);
    }
    let diff = exec.git(
        worktree,
        &["diff", "--cached", "--binary", "--no-color", "HEAD"],
        &env,
        None,
    );
    if diff.status != 0 {
        return die(
            format!("git diff falhou em {shown}: {}", diff.stderr.trim()),
            1,
            None,
        );
    }
    Ok(diff.stdout)
}

/// LC:288-292 (`Captured`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub info: WorktreeInfo,
    pub parts: Vec<Part>,
    pub findings: Vec<Finding>,
}

/// `String(valor ?? "")` do JS para os campos do registro.
pub fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(other) => other.to_string(),
    }
}

/// LC:288-321 (`captureWorktree`): bundle de `base..HEAD` (o não publicado) e patch do não commitado. Limpa e sem
/// commit leva só a referência da base.
pub fn capture_worktree(
    exec: &dyn GitExec,
    agent: &str,
    record: &serde_json::Map<String, Value>,
    reports: &str,
) -> Result<Captured, HadesError> {
    let path_text = js_string(record.get("worktree"));
    let path = Path::new(&path_text);
    if path_text.is_empty()
        || exec
            .git(path, &["rev-parse", "--git-dir"], &[], None)
            .status
            != 0
    {
        return die(
            format!("worktree de {agent} não existe: {path_text}"),
            1,
            None,
        );
    }
    let base = js_string(record.get("base"));
    let base_ref = if base.is_empty() { "main" } else { &base }.to_owned();
    let head = git_text(exec, path, &["rev-parse", "HEAD"]);
    let base_sha = git_text(exec, path, &["merge-base", &base_ref, "HEAD"]);
    if head.is_empty() || base_sha.is_empty() {
        return die(
            format!("{agent}: não achei a base {base_ref} na worktree {path_text}"),
            1,
            None,
        );
    }
    let branch = git_text(exec, path, &["branch", "--show-current"]);
    let mut parts = Vec::new();
    let mut findings = Vec::new();
    let prefix = format!("worktrees/{agent}");
    let mut bundle = None;
    if head != base_sha {
        let scratch = tempfile::Builder::new()
            .prefix("caronte-bundle-")
            .tempdir()
            .map_err(|e| HadesError::with_code(format!("não criei a pasta do bundle: {e}"), 1))?;
        let file = scratch.path().join("commits.bundle");
        let file_text = file.to_string_lossy();
        let range = format!("{base_sha}..HEAD");
        let made = exec.git(path, &["bundle", "create", &file_text, &range], &[], None);
        if made.status != 0 {
            return die(
                format!("git bundle falhou em {path_text}: {}", made.stderr.trim()),
                1,
                None,
            );
        }
        let logical = format!("{prefix}/commits.bundle");
        let data = std::fs::read(&file).map_err(|e| {
            HadesError::with_code(format!("não li o bundle de {path_text}: {e}"), 1)
        })?;
        parts.push(Part::new(logical.clone(), data));
        bundle = Some(logical);
        let committed = exec.git(path, &["diff", "--binary", "--no-color", &range], &[], None);
        findings.extend(scan_diff(&prefix, &committed.stdout));
    }
    let diff = uncommitted_patch(exec, path, reports)?;
    let mut patch = None;
    if !diff.is_empty() {
        let logical = format!("{prefix}/uncommitted.patch");
        findings.extend(scan_diff(&prefix, &diff));
        parts.push(Part::new(logical.clone(), diff));
        patch = Some(logical);
    }
    Ok(Captured {
        info: WorktreeInfo {
            agent: agent.to_owned(),
            path: path_text,
            branch,
            head,
            base_ref,
            base_sha,
            bundle,
            patch,
        },
        parts,
        findings,
    })
}

#[cfg(test)]
pub(crate) mod testing {
    use std::cell::RefCell;

    use super::*;

    /// Executor de teste: responde por prefixo de argumentos e guarda o que foi chamado.
    pub struct FakeGit {
        pub replies: Vec<(Vec<&'static str>, GitOut)>,
        pub calls: RefCell<Vec<Vec<String>>>,
    }

    pub fn reply(status: i32, stdout: &str) -> GitOut {
        GitOut {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: String::new(),
        }
    }

    impl GitExec for FakeGit {
        fn git(&self, _: &Path, args: &[&str], _: &[(&str, &str)], _: Option<&[u8]>) -> GitOut {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            self.replies
                .iter()
                .find(|(prefix, _)| args.starts_with(prefix))
                .map(|(_, out)| out.clone())
                .unwrap_or_else(|| reply(0, ""))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::testing::{FakeGit, reply};
    use super::*;

    fn record(worktree: &str, base: &str) -> serde_json::Map<String, Value> {
        let value = serde_json::json!({"worktree": worktree, "base": base});
        value.as_object().cloned().unwrap()
    }

    fn fake(replies: Vec<(Vec<&'static str>, GitOut)>) -> FakeGit {
        FakeGit {
            replies,
            calls: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn js_string_como_string_do_js() {
        assert_eq!(js_string(None), "");
        assert_eq!(js_string(Some(&Value::Null)), "");
        assert_eq!(js_string(Some(&serde_json::json!("x"))), "x");
        assert_eq!(js_string(Some(&serde_json::json!(5))), "5");
        assert_eq!(js_string(Some(&serde_json::json!(true))), "true");
        assert_eq!(
            js_string(Some(&serde_json::json!({"a": 1}))),
            "[object Object]"
        );
        assert_eq!(js_string(Some(&serde_json::json!([1, null, "b"]))), "1,,b");
    }

    #[test]
    fn worktree_ausente_recusa_com_codigo_1() {
        let git = fake(vec![(vec!["rev-parse", "--git-dir"], reply(128, ""))]);
        let err = capture_worktree(&git, "fake-a", &record("/nada", "main"), ".r").unwrap_err();
        assert_eq!(err.code, 1);
        assert_eq!(err.message, "worktree de fake-a não existe: /nada");
    }

    #[test]
    fn worktree_vazia_no_registro_recusa() {
        let git = fake(vec![]);
        let err = capture_worktree(&git, "fake-a", &record("", "main"), ".r").unwrap_err();
        assert_eq!(err.message, "worktree de fake-a não existe: ");
        assert!(git.calls.borrow().is_empty());
    }

    #[test]
    fn base_nao_achada_recusa_nomeando_a_base() {
        let git = fake(vec![
            (vec!["rev-parse", "HEAD"], reply(0, "abc\n")),
            (vec!["merge-base"], reply(1, "")),
        ]);
        let err = capture_worktree(&git, "fake-a", &record("/w", ""), ".r").unwrap_err();
        assert_eq!(err.code, 1);
        assert_eq!(err.message, "fake-a: não achei a base main na worktree /w");
    }

    #[test]
    fn falha_do_read_tree_recusa_sem_vazar_saida() {
        let git = fake(vec![(vec!["read-tree"], reply(1, ""))]);
        let err = uncommitted_patch(&git, Path::new("/w"), ".r").unwrap_err();
        assert_eq!(err.message, "git read-tree falhou em /w");
        assert_eq!(err.code, 1);
    }

    #[test]
    fn falha_do_add_e_do_diff_tem_mensagem_propria() {
        let git = fake(vec![(vec!["add"], reply(1, ""))]);
        let err = uncommitted_patch(&git, Path::new("/w"), ".r").unwrap_err();
        assert_eq!(err.message, "git add -A falhou em /w");
        let git = fake(vec![(
            vec!["diff"],
            GitOut {
                status: 1,
                stdout: Vec::new(),
                stderr: " boom \n".into(),
            },
        )]);
        let err = uncommitted_patch(&git, Path::new("/w"), ".r").unwrap_err();
        assert_eq!(err.message, "git diff falhou em /w: boom");
    }

    #[test]
    fn pasta_ignorada_nao_vira_pathspec_de_exclusao() {
        let git = fake(vec![(vec!["check-ignore"], reply(0, ""))]);
        uncommitted_patch(&git, Path::new("/w"), ".r").unwrap();
        let add = git
            .calls
            .borrow()
            .iter()
            .find(|c| c[0] == "add")
            .cloned()
            .unwrap();
        assert_eq!(add, ["add", "-A", "--", "."]);
        let git = fake(vec![(vec!["check-ignore"], reply(1, ""))]);
        uncommitted_patch(&git, Path::new("/w"), ".r").unwrap();
        let add = git
            .calls
            .borrow()
            .iter()
            .find(|c| c[0] == "add")
            .cloned()
            .unwrap();
        assert_eq!(add, ["add", "-A", "--", ".", ":(exclude).r"]);
    }

    #[test]
    fn worktree_limpa_leva_so_a_referencia_da_base() {
        let git = fake(vec![
            (vec!["rev-parse", "HEAD"], reply(0, "aaa\n")),
            (vec!["merge-base"], reply(0, "aaa\n")),
            (vec!["branch"], reply(0, "feat/x\n")),
        ]);
        let got = capture_worktree(&git, "fake-a", &record("/w", "main"), ".r").unwrap();
        assert!(got.parts.is_empty());
        assert_eq!(got.info.bundle, None);
        assert_eq!(got.info.patch, None);
        assert_eq!(got.info.branch, "feat/x");
        assert_eq!(got.info.base_sha, got.info.head);
    }

    #[test]
    fn executor_real_captura_saida_status_e_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let sys = SystemGit;
        assert_eq!(
            sys.git(dir.path(), &["init", "-q", "-b", "main"], &[], None)
                .status,
            0
        );
        let hash = sys.git(dir.path(), &["hash-object", "--stdin"], &[], Some(b"oi\n"));
        assert_eq!(hash.status, 0);
        assert_eq!(hash.stdout.len(), 41);
        let bad = sys.git(dir.path(), &["rev-parse", "nao-existe"], &[], None);
        assert_ne!(bad.status, 0);
        assert!(!bad.stderr.is_empty());
        let missing = sys.git(&dir.path().join("x/y"), &["status"], &[], None);
        assert_ne!(missing.status, 0);
    }
}
