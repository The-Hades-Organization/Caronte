//! Caronte, parte 2: abre o pacote e refaz a orquestração nesta máquina. Porta de LU:1-338. Duas fases: `prepare`
//! valida tudo e não escreve nada (é o que o `--dry-run` mostra); `apply` escreve e, se uma escrita falhar, desfaz
//! só o que criou.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hades_core::Env;
use hades_core::fsx::{is_dir, read_text};
use hades_core::jsonfmt::json_indent;
use hades_core::paths::{hades_home, work_root};
use hades_core::registry::plan_sha;
use hades_core::work::{Json, read_json};
use regex::Regex;
use serde_json::Value;

use crate::format::{FORMAT_VERSION, Part};
use crate::gitx::{GitExec, git_text};
use crate::guard::basename;
use crate::manifest::{Manifest, WorktreeInfo};
use crate::plan::{REPORT_FILES, is_agent_name};

/// LU:12-17 (`UnpackError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackError(pub String);

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UnpackError {}

fn fail<T>(message: impl Into<String>) -> Result<T, UnpackError> {
    Err(UnpackError(message.into()))
}

/// LU:19-26 (`UnpackOptions`). Os diretórios já vêm resolvidos (absolutos).
#[derive(Debug, Clone, Default)]
pub struct UnpackOptions {
    pub work_root: Option<String>,
    pub plans: Option<String>,
    pub worktrees: Option<String>,
    pub clone: Option<String>,
    pub reports: String,
    pub skill_version: String,
}

/// LU:28-34 (`PlannedWorktree`).
#[derive(Debug, Clone)]
pub struct PlannedWorktree {
    pub info: WorktreeInfo,
    pub dest: String,
    pub bundle: Option<Part>,
    pub patch: Option<Part>,
    pub needs_fetch: bool,
}

/// LU:36-39 (`FileWrite`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: String,
    pub data: Vec<u8>,
}

/// LU:41-52 (`Prepared.agents`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedAgent {
    pub name: String,
    pub state: String,
    pub role: String,
    pub worktree: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Valida,
    Refazer,
    Ausente,
}

/// LU:41-52 (`Prepared`).
#[derive(Debug, Clone)]
pub struct Prepared {
    pub clone: String,
    pub work_dir: String,
    pub plan_file: String,
    pub path_map: Vec<(String, String)>,
    pub worktrees: Vec<PlannedWorktree>,
    pub files: Vec<FileWrite>,
    pub plan_writes: Vec<FileWrite>,
    pub agents: Vec<PreparedAgent>,
    pub warnings: Vec<String>,
    pub approval: Approval,
}

const ENDED: [&str; 2] = ["done", "archived"];

fn safe_file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_][A-Za-z0-9._-]*$").expect("padrão válido"))
}

fn plan_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*\.md$").expect("padrão válido"))
}

fn commit_sha_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9a-f]{40}([0-9a-f]{24})?$").expect("padrão válido"))
}

/* ---------- caminhos (Node `path`) ---------- */

/// `path.join` do Node: junta e normaliza de forma léxica, sem tocar o disco.
pub fn node_join(a: &str, b: &str) -> String {
    let joined = match (a.is_empty(), b.is_empty()) {
        (true, true) => return ".".to_owned(),
        (true, false) => b.to_owned(),
        (false, true) => a.to_owned(),
        (false, false) => format!("{a}/{b}"),
    };
    let absolute = joined.starts_with('/');
    let trailing = joined.ends_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|last| *last != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    let mut body = stack.join("/");
    if body.is_empty() && !absolute {
        body = ".".to_owned();
    }
    if trailing && !body.is_empty() && body != "." {
        body.push('/');
    }
    if absolute { format!("/{body}") } else { body }
}

/// `path.dirname` do Node.
pub fn node_dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    match trimmed.rfind('/') {
        None => ".".to_owned(),
        Some(i) => {
            let dir = trimmed[..i].trim_end_matches('/');
            if dir.is_empty() {
                "/".to_owned()
            } else {
                dir.to_owned()
            }
        }
    }
}

/* ---------- versões ---------- */

fn major_minor(version: &str) -> (&str, &str) {
    let mut parts = version.split('.');
    (parts.next().unwrap_or(""), parts.next().unwrap_or(""))
}

/// LU:68-82 (`checkVersions`): recusa formato mais novo e skillVersion de outro major; outro minor vira aviso.
pub fn check_versions(
    manifest: &Manifest,
    local_version: &str,
) -> Result<Vec<String>, UnpackError> {
    if manifest.format > FORMAT_VERSION {
        return fail(format!(
            "formato {} do pacote é mais novo que o desta skill ({FORMAT_VERSION}): atualize a Hades",
            manifest.format
        ));
    }
    let (their_major, their_minor) = major_minor(&manifest.skill_version);
    let (my_major, my_minor) = major_minor(local_version);
    if their_major != my_major {
        return fail(format!(
            "o pacote é da Hades {} e esta é a {local_version}: major diferente, atualize ou use a mesma versão",
            manifest.skill_version
        ));
    }
    Ok(if their_minor == my_minor {
        Vec::new()
    } else {
        vec![format!(
            "o pacote é da Hades {} e esta é a {local_version} (minor diferente)",
            manifest.skill_version
        )]
    })
}

/* ---------- reescrita de caminhos ---------- */

/// LU:88-96 (`pathRewriter`): prefixos de origem para destino numa passada só, o mais longo primeiro (um destino
/// nunca é reescrito de novo).
pub fn path_rewriter(map: &[(String, String)]) -> impl Fn(&str) -> String + use<> {
    let mut pairs: Vec<&(String, String)> = map
        .iter()
        .filter(|(from, to)| !from.is_empty() && from != to)
        .collect();
    pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.encode_utf16().count()));
    let lookup: HashMap<String, String> =
        pairs.iter().map(|(f, t)| (f.clone(), t.clone())).collect();
    let re = if pairs.is_empty() {
        None
    } else {
        let alternation = pairs
            .iter()
            .map(|(from, _)| regex::escape(from))
            .collect::<Vec<_>>()
            .join("|");
        Some(Regex::new(&alternation).expect("alternativas escapadas"))
    };
    move |text: &str| match &re {
        None => text.to_owned(),
        Some(re) => re
            .replace_all(text, |caps: &regex::Captures| {
                let hit = &caps[0];
                lookup.get(hit).cloned().unwrap_or_else(|| hit.to_owned())
            })
            .into_owned(),
    }
}

/// LU:98-105 (`rewriteJson`): só os valores string.
pub fn rewrite_json(value: Value, rewrite: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(rewrite(&text)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| rewrite_json(v, rewrite))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, rewrite_json(v, rewrite)))
                .collect(),
        ),
        other => other,
    }
}

/// LU:108-110 (`localWorkRoot`): `HADES_WORK_ROOT`, `work-root`, `~/.claude/hades/work`.
fn local_work_root(env: &Env) -> String {
    work_root(env).to_string_lossy().into_owned()
}

/// LU:113-116 (`plansDirOf`): plano que veio de `~/.claude/hades/plans` volta para lá; qualquer outro, para a pasta
/// de origem.
fn plans_dir_of(env: &Env, origin: &str, requested: Option<&str>) -> String {
    if let Some(dir) = requested.filter(|d| !d.is_empty()) {
        return dir.to_owned();
    }
    let dir = node_dirname(origin);
    if dir.ends_with("/hades/plans") {
        hades_home(env).join("plans").to_string_lossy().into_owned()
    } else {
        dir
    }
}

/// LU:118-121 (`safeName`).
pub fn safe_name(value: &str, what: &str) -> Result<String, UnpackError> {
    if !safe_file_re().is_match(value) || value == "." || value == ".." {
        return fail(format!("nome inseguro no pacote ({what}): {value}"));
    }
    Ok(value.to_owned())
}

/* ---------- validação ---------- */

fn text_of(part: &Part) -> String {
    String::from_utf8_lossy(&part.data).into_owned()
}

/// LU:127-130 (`asRecord`).
fn as_record(value: Value, what: &str) -> Result<Json, UnpackError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => fail(format!("{what} do pacote não é um objeto JSON")),
    }
}

/// LU:132-138 (`parseRecord`).
fn parse_record(part: &Part) -> Result<Json, UnpackError> {
    match serde_json::from_str::<Value>(&text_of(part)) {
        Ok(value) => as_record(value, &part.path),
        Err(_) => fail(format!("{} do pacote não é JSON válido", part.path)),
    }
}

fn is_repo(exec: &dyn GitExec, dir: &str) -> bool {
    is_dir(Path::new(dir))
        && exec
            .git(Path::new(dir), &["rev-parse", "--git-dir"], &[], None)
            .status
            == 0
}

/// LU:140-155 (`findClone`).
fn find_clone(
    exec: &dyn GitExec,
    manifest: &Manifest,
    requested: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<String, UnpackError> {
    let remote = manifest.repo.remote.as_str();
    let hint = if remote.is_empty() {
        "(o pacote não registra o remote: informe um clone com --clone)".to_owned()
    } else {
        format!("git clone {remote} <destino>")
    };
    let url_of = |dir: &str| {
        git_text(
            exec,
            Path::new(dir),
            &["config", "--get", "remote.origin.url"],
        )
    };
    if let Some(requested) = requested.filter(|r| !r.is_empty()) {
        if !is_repo(exec, requested) {
            return fail(format!(
                "--clone {requested} não é um repositório git; clone o projeto antes: {hint}"
            ));
        }
        let url = url_of(requested);
        if url != remote {
            warnings.push(format!(
                "o remote.origin.url de {requested} ({}) difere do do pacote ({})",
                if url.is_empty() { "vazio" } else { &url },
                if remote.is_empty() { "vazio" } else { remote }
            ));
        }
        return Ok(requested.to_owned());
    }
    let origin = manifest.repo.clone.as_str();
    if !origin.is_empty() && is_repo(exec, origin) && url_of(origin) == remote {
        return Ok(origin.to_owned());
    }
    fail(format!(
        "não há clone deste projeto aqui (o do pacote: {}); o unpack nunca clona sozinho. Clone e aponte com --clone: {hint}",
        if origin.is_empty() { "?" } else { origin }
    ))
}

fn has_commit(exec: &dyn GitExec, clone: &str, sha: &str) -> bool {
    exec.git(
        Path::new(clone),
        &["cat-file", "-e", &format!("{sha}^{{commit}}")],
        &[],
        None,
    )
    .status
        == 0
}

fn git_ok(exec: &dyn GitExec, dir: &str, args: &[&str]) -> bool {
    exec.git(Path::new(dir), args, &[], None).status == 0
}

/// `String(value ?? fallback)` do JS para valor de registro.
fn js_str(value: Option<&Value>, fallback: &str) -> String {
    match value {
        None | Some(Value::Null) => fallback.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// LU:157-263 (`prepare`): valida tudo e monta o que seria escrito. Não escreve nada.
pub fn prepare(
    exec: &dyn GitExec,
    parts: &[Part],
    manifest: &Manifest,
    options: &UnpackOptions,
    env: &Env,
) -> Result<Prepared, UnpackError> {
    let mut warnings = check_versions(manifest, &options.skill_version)?;
    let listed: Vec<&str> = manifest.parts.iter().map(|p| p.path.as_str()).collect();
    let mut by_path: Vec<(&str, &Part)> = Vec::new();
    for part in parts.iter().filter(|p| listed.contains(&p.path.as_str())) {
        match by_path.iter_mut().find(|(path, _)| *path == part.path) {
            Some(slot) => slot.1 = part,
            None => by_path.push((part.path.as_str(), part)),
        }
    }
    let find = |path: &str| {
        by_path
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, part)| *part)
    };
    let clone = find_clone(exec, manifest, options.clone.as_deref(), &mut warnings)?;

    let work_base = options
        .work_root
        .clone()
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| local_work_root(env));
    let work_dir = node_join(
        &work_base,
        &safe_name(basename(&manifest.work_root), "raiz de trabalho")?,
    );
    let orch = node_join(&work_dir, "orchestration");
    let plan_name = safe_name(&manifest.plan.name, "plano")?;
    let plans_dir = plans_dir_of(env, &manifest.plan.path, options.plans.as_deref());
    let plan_file = node_join(&plans_dir, &format!("{plan_name}.md"));
    let plan_log = node_join(&plans_dir, &format!("{plan_name}.log.md"));

    let plan_part = find(&format!("plan/{plan_name}.md"));
    let Some(plan_part) = plan_part.filter(|_| plan_name_re().is_match(&format!("{plan_name}.md")))
    else {
        return fail("o pacote não traz o plano");
    };
    let restored_sha = plan_sha(&text_of(plan_part));
    if restored_sha != manifest.plan.sha {
        return fail("o plano do pacote não confere com o sha do manifesto");
    }

    let mut plan_writes: Vec<FileWrite> = Vec::new();
    if Path::new(&plan_file).exists() {
        let existing = plan_sha(&read_text(Path::new(&plan_file)).unwrap_or_default());
        if existing != manifest.plan.sha {
            return fail(format!(
                "o plano {plan_file} já existe com outro conteúdo (sha {}, o do pacote {}): escolha outro --plans ou resolva à mão",
                head(&existing, 12),
                head(&manifest.plan.sha, 12)
            ));
        }
    } else {
        plan_writes.push(FileWrite {
            path: plan_file.clone(),
            data: plan_part.data.clone(),
        });
    }
    if let Some(log) = find(&format!("plan/{plan_name}.log.md"))
        && !Path::new(&plan_log).exists()
        && !Path::new(&plan_file).exists()
    {
        plan_writes.push(FileWrite {
            path: plan_log,
            data: log.data.clone(),
        });
    }

    let mut path_map: Vec<(String, String)> = vec![
        (manifest.work_root.clone(), work_dir.clone()),
        (manifest.plan.path.clone(), plan_file.clone()),
    ];
    let mut worktrees: Vec<PlannedWorktree> = Vec::new();
    for info in &manifest.worktrees {
        let agent = &info.agent;
        if !is_agent_name(agent) {
            return fail(format!("agente inválido no manifesto: {agent}"));
        }
        let base = options
            .worktrees
            .clone()
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| node_dirname(&info.path));
        let dest = node_join(
            &base,
            &safe_name(basename(&info.path), &format!("worktree de {agent}"))?,
        );
        path_map.push((info.path.clone(), dest.clone()));
        worktrees.push(PlannedWorktree {
            info: info.clone(),
            dest,
            bundle: info.bundle.as_deref().and_then(find).cloned(),
            patch: info.patch.as_deref().and_then(find).cloned(),
            needs_fetch: false,
        });
    }

    for entry in &mut worktrees {
        validate_worktree(exec, entry, &clone, &mut warnings)?;
    }

    let rewrite = path_rewriter(&path_map);
    let mut files: Vec<FileWrite> = Vec::new();
    let mut agents: Vec<PreparedAgent> = Vec::new();
    let dest_of: HashMap<&str, &str> = worktrees
        .iter()
        .map(|w| (w.info.agent.as_str(), w.dest.as_str()))
        .collect();
    let mut approval = Approval::Ausente;

    for (path, part) in &by_path {
        let mut segments = path.split('/');
        let area = segments.next().unwrap_or("");
        let rest: Vec<&str> = segments.collect();
        if *path == "orchestration/plan.json" {
            let dest = node_join(&orch, "plan.json");
            let current: Option<Json> = if Path::new(&dest).exists() {
                Some(read_json(Path::new(&dest)).unwrap_or_default())
            } else {
                None
            };
            if let Some(current) = &current {
                let same = matches!(current.get("path"), Some(Value::String(p)) if *p == plan_file);
                if !same {
                    return fail(format!(
                        "{dest} já guarda a aprovação de outro plano ({}): não é tocado, resolva à mão",
                        js_str(current.get("path"), "undefined")
                    ));
                }
            }
            approval = if manifest.plan.approved_sha.as_deref() == Some(restored_sha.as_str()) {
                Approval::Valida
            } else {
                Approval::Refazer
            };
            if current.is_none() {
                let rewritten = rewrite_json(Value::Object(parse_record(part)?), &rewrite);
                files.push(FileWrite {
                    path: dest,
                    data: json_file(&rewritten),
                });
            }
        } else if area == "orchestration" && rest.len() == 2 && rest[0] == "agents" {
            let name = safe_name(rest[1].strip_suffix(".json").unwrap_or(rest[1]), "agente")?;
            if !is_agent_name(&name) || !manifest.agents.contains(&name) {
                continue;
            }
            let record = as_record(
                rewrite_json(Value::Object(parse_record(part)?), &rewrite),
                &part.path,
            )?;
            let ended = matches!(record.get("state"), Some(Value::String(s)) if ENDED.contains(&s.as_str()));
            let mut restored = record.clone();
            if !ended {
                restored.insert("state".to_owned(), Value::String("parked".to_owned()));
            }
            restored.shift_remove("pane");
            files.push(FileWrite {
                path: node_join(&node_join(&orch, "agents"), &format!("{name}.json")),
                data: json_file(&Value::Object(restored.clone())),
            });
            agents.push(PreparedAgent {
                name,
                state: js_str(restored.get("state"), "undefined"),
                role: js_str(record.get("role"), "impl"),
                worktree: js_str(restored.get("worktree"), ""),
            });
        } else if area == "orchestration"
            && rest.len() == 2
            && (rest[0] == "watch" || rest[0] == "consolidations")
        {
            files.push(FileWrite {
                path: node_join(
                    &node_join(&orch, rest[0]),
                    &safe_name(rest[1], "arquivo do registro")?,
                ),
                data: rewrite(&text_of(part)).into_bytes(),
            });
        } else if area == "reports" && rest.len() == 2 {
            if let Some(dest) = dest_of.get(rest[0])
                && REPORT_FILES.contains(&rest[1])
            {
                files.push(FileWrite {
                    path: node_join(&node_join(dest, &options.reports), rest[1]),
                    data: part.data.clone(),
                });
            }
        } else if area == "journal" && rest.len() == 1 {
            let name = rest[0].strip_suffix(".jsonl").unwrap_or(rest[0]);
            if is_agent_name(name) {
                files.push(FileWrite {
                    path: node_join(&node_join(&work_dir, "journal"), &format!("{name}.jsonl")),
                    data: part.data.clone(),
                });
            }
        }
    }

    let taken: Vec<&str> = files
        .iter()
        .filter(|f| Path::new(&f.path).exists())
        .map(|f| f.path.as_str())
        .collect();
    if !taken.is_empty() {
        return fail(format!(
            "já existe no destino (nada foi tocado): {}",
            taken.join(", ")
        ));
    }
    if approval == Approval::Refazer {
        warnings.push(format!(
            "a aprovação do plano não confere com o plano restaurado: rode hades agent plan-approve {plan_file}"
        ));
    }
    Ok(Prepared {
        clone,
        work_dir,
        plan_file,
        path_map,
        worktrees,
        files,
        plan_writes,
        agents,
        warnings,
        approval,
    })
}

/// `${JSON.stringify(value, null, 2)}\n`.
fn json_file(value: &Value) -> Vec<u8> {
    format!("{}\n", json_indent(value, 2)).into_bytes()
}

/// `slice(0, n)` do JS por caractere.
fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// LU:265-283 (`validateWorktree`).
fn validate_worktree(
    exec: &dyn GitExec,
    entry: &mut PlannedWorktree,
    clone: &str,
    warnings: &mut Vec<String>,
) -> Result<(), UnpackError> {
    let info = &entry.info;
    let dest = entry.dest.as_str();
    if !commit_sha_re().is_match(&info.base_sha) {
        return fail(format!("{}: baseSha inválido no manifesto", info.agent));
    }
    if !commit_sha_re().is_match(&info.head) {
        return fail(format!("{}: head inválido no manifesto", info.agent));
    }
    let base_ref = info.base_ref.as_str();
    if base_ref.starts_with('-')
        || !git_ok(exec, clone, &["check-ref-format", "--branch", base_ref])
    {
        return fail(format!("{}: baseRef inválido no manifesto", info.agent));
    }
    if info.branch.is_empty() {
        return fail(format!(
            "{}: a worktree de origem estava em HEAD destacado, o unpack só refaz worktrees de branch",
            info.agent
        ));
    }
    if !git_ok(exec, clone, &["check-ref-format", "--branch", &info.branch]) {
        return fail(format!(
            "{}: nome de branch inválido: {}",
            info.agent, info.branch
        ));
    }
    if Path::new(dest).exists() {
        return fail(format!(
            "{}: {dest} já existe (nada foi tocado): escolha outro --worktrees",
            info.agent
        ));
    }
    if git_ok(
        exec,
        clone,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", info.branch),
        ],
    ) {
        return fail(format!(
            "{}: a branch {} já existe no clone {clone} (nada foi tocado)",
            info.agent, info.branch
        ));
    }
    if !has_commit(exec, clone, &info.base_sha) {
        entry.needs_fetch = true;
        warnings.push(format!(
            "{}: a base {} ({}) não está no clone; o unpack faz git fetch origin {}",
            info.agent,
            info.base_ref,
            head(&info.base_sha, 12),
            info.base_ref
        ));
    }
    if info.bundle.is_some() && entry.bundle.is_none() {
        return fail(format!(
            "{}: o manifesto cita {}, que não está no pacote",
            info.agent,
            info.bundle.as_deref().unwrap_or("")
        ));
    }
    if info.patch.is_some() && entry.patch.is_none() {
        return fail(format!(
            "{}: o manifesto cita {}, que não está no pacote",
            info.agent,
            info.patch.as_deref().unwrap_or("")
        ));
    }
    Ok(())
}

/* ---------- aplicação ---------- */

/// Um passo de desfazer, executado em ordem inversa.
enum Undo {
    RemoveTree(PathBuf),
    RemoveFile(PathBuf),
    DeleteBranch { clone: String, branch: String },
    RemoveWorktree { clone: String, dest: String },
}

fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) if path.is_file() || path.is_symlink() => remove_file(path),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Undo {
    fn run(self, exec: &dyn GitExec) -> Result<(), String> {
        match self {
            Self::RemoveTree(path) => remove_tree(&path),
            Self::RemoveFile(path) => remove_file(&path),
            Self::DeleteBranch { clone, branch } => {
                exec.git(Path::new(&clone), &["branch", "-D", &branch], &[], None);
                Ok(())
            }
            Self::RemoveWorktree { clone, dest } => {
                exec.git(
                    Path::new(&clone),
                    &["worktree", "remove", "--force", &dest],
                    &[],
                    None,
                );
                let removed = remove_tree(Path::new(&dest));
                exec.git(Path::new(&clone), &["worktree", "prune"], &[], None);
                removed
            }
        }
    }
}

fn io_fail(path: &str, error: &std::io::Error) -> UnpackError {
    UnpackError(format!("{path}: {error}"))
}

/// LU:289-292 (`ensureDir`): `mkdir -p` que registra a pasta mais alta que ele mesmo criou.
fn ensure_dir(dir: &str, undo: &mut Vec<Undo>) -> Result<(), UnpackError> {
    let path = Path::new(dir);
    let first_new = path
        .ancestors()
        .take_while(|a| !a.exists())
        .last()
        .map(Path::to_path_buf);
    fs::create_dir_all(path).map_err(|e| io_fail(dir, &e))?;
    if let Some(first) = first_new {
        undo.push(Undo::RemoveTree(first));
    }
    Ok(())
}

/// LU:294-298 (`writeNew`): `wx`, nunca sobrescreve.
fn write_new(file: &FileWrite, undo: &mut Vec<Undo>) -> Result<(), UnpackError> {
    ensure_dir(&node_dirname(&file.path), undo)?;
    let mut handle = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file.path)
        .map_err(|e| io_fail(&file.path, &e))?;
    undo.push(Undo::RemoveFile(PathBuf::from(&file.path)));
    handle
        .write_all(&file.data)
        .map_err(|e| io_fail(&file.path, &e))
}

/// LU:300-302 (`must`).
fn must(out: &crate::gitx::GitOut, what: &str) -> Result<(), UnpackError> {
    if out.status != 0 {
        return fail(format!("{what}: {}", out.stderr.trim()));
    }
    Ok(())
}

/// LU:304-336 (`restoreWorktree`): cria a branch (do bundle, ou na base), a worktree, confere o HEAD e aplica o
/// patch.
fn restore_worktree(
    exec: &dyn GitExec,
    entry: &PlannedWorktree,
    clone: &str,
    undo: &mut Vec<Undo>,
) -> Result<(), UnpackError> {
    let info = &entry.info;
    let dest = entry.dest.as_str();
    let clone_dir = Path::new(clone);
    if entry.needs_fetch && !has_commit(exec, clone, &info.base_sha) {
        let origin_ref = info
            .base_ref
            .strip_prefix("origin/")
            .unwrap_or(&info.base_ref);
        let fetched = exec.git(clone_dir, &["fetch", "origin", origin_ref], &[], None);
        if fetched.status != 0 || !has_commit(exec, clone, &info.base_sha) {
            return fail(format!(
                "{}: a base {} ({}) não está no clone e o fetch falhou; rode: git -C {clone} fetch origin {}",
                info.agent, info.base_ref, info.base_sha, info.base_ref
            ));
        }
    }
    let scratch = tempfile::Builder::new()
        .prefix("caronte-unpack-")
        .tempdir()
        .map_err(|e| UnpackError(format!("{}: {e}", info.agent)))?;
    if let Some(bundle) = &entry.bundle {
        let file = scratch.path().join("commits.bundle");
        fs::write(&file, &bundle.data).map_err(|e| io_fail(&file.to_string_lossy(), &e))?;
        let file = file.to_string_lossy().into_owned();
        must(
            &exec.git(clone_dir, &["bundle", "verify", &file], &[], None),
            &format!("{}: git bundle verify", info.agent),
        )?;
        must(
            &exec.git(
                clone_dir,
                &[
                    "fetch",
                    "--no-tags",
                    &file,
                    &format!("HEAD:refs/heads/{}", info.branch),
                ],
                &[],
                None,
            ),
            &format!("{}: aplicar o bundle", info.agent),
        )?;
    } else {
        must(
            &exec.git(
                clone_dir,
                &["branch", &info.branch, &info.base_sha],
                &[],
                None,
            ),
            &format!("{}: criar a branch {}", info.agent, info.branch),
        )?;
    }
    undo.push(Undo::DeleteBranch {
        clone: clone.to_owned(),
        branch: info.branch.clone(),
    });
    ensure_dir(&node_dirname(dest), undo)?;
    must(
        &exec.git(
            clone_dir,
            &["worktree", "add", dest, &info.branch],
            &[],
            None,
        ),
        &format!("{}: git worktree add", info.agent),
    )?;
    undo.push(Undo::RemoveWorktree {
        clone: clone.to_owned(),
        dest: dest.to_owned(),
    });
    let head_now = git_text(exec, Path::new(dest), &["rev-parse", "HEAD"]);
    if head_now != info.head {
        return fail(format!(
            "{}: o HEAD restaurado ({head_now}) difere do do manifesto ({})",
            info.agent, info.head
        ));
    }
    if let Some(patch) = &entry.patch {
        must(
            &exec.git(
                Path::new(dest),
                &["apply", "--binary", "-"],
                &[],
                Some(&patch.data),
            ),
            &format!("{}: aplicar uncommitted.patch", info.agent),
        )?;
    }
    Ok(())
}

/// LU:318-337 (`apply`): escreve tudo; se algo falha no meio, desfaz em ordem inversa o que este unpack criou e
/// devolve o erro.
pub fn apply(exec: &dyn GitExec, prepared: &Prepared) -> Result<(), UnpackError> {
    let mut undo: Vec<Undo> = Vec::new();
    let run = |undo: &mut Vec<Undo>| -> Result<(), UnpackError> {
        for file in &prepared.plan_writes {
            write_new(file, undo)?;
        }
        for entry in &prepared.worktrees {
            restore_worktree(exec, entry, &prepared.clone, undo)?;
        }
        for file in &prepared.files {
            write_new(file, undo)?;
        }
        Ok(())
    };
    let Err(error) = run(&mut undo) else {
        return Ok(());
    };
    let failed: Vec<String> = undo
        .into_iter()
        .rev()
        .filter_map(|step| step.run(exec).err())
        .collect();
    if failed.is_empty() {
        return Err(error);
    }
    fail(format!(
        "{} (o desfazer também falhou, confira à mão: {})",
        error.0,
        failed.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PartEntry, PlanRef, Repo, Scope};

    fn manifest(format: i64, version: &str) -> Manifest {
        Manifest {
            format,
            skill_version: version.into(),
            work_root: "/w".into(),
            scope: Scope::Skill,
            repo: Repo {
                remote: String::new(),
                clone: String::new(),
            },
            plan: PlanRef {
                path: "/p/demo.md".into(),
                name: "demo".into(),
                approved_sha: None,
                sha: String::new(),
            },
            agents: Vec::new(),
            worktrees: Vec::new(),
            with_journal: false,
            allowed: Vec::new(),
            created_at: String::new(),
            parts: Vec::<PartEntry>::new(),
        }
    }

    fn pair(from: &str, to: &str) -> (String, String) {
        (from.to_owned(), to.to_owned())
    }

    #[test]
    fn versoes_recusam_major_e_format_novos_e_avisam_minor() {
        let err = check_versions(&manifest(2, "1.0.0"), "1.0.0").unwrap_err();
        assert!(err.0.contains("mais novo"), "{err}");
        let err = check_versions(&manifest(1, "2.0.0"), "1.0.0").unwrap_err();
        assert!(err.0.contains("major diferente"), "{err}");
        assert_eq!(
            check_versions(&manifest(1, "1.2.0"), "1.2.9").unwrap(),
            Vec::<String>::new()
        );
        let warn = check_versions(&manifest(1, "1.3.0"), "1.2.0").unwrap();
        assert_eq!(
            warn,
            ["o pacote é da Hades 1.3.0 e esta é a 1.2.0 (minor diferente)"]
        );
    }

    #[test]
    fn rewriter_prefere_o_prefixo_mais_longo_e_nao_reescreve_destino() {
        let rewrite = path_rewriter(&[pair("/a", "/x"), pair("/a/b", "/y"), pair("/x", "/z")]);
        assert_eq!(rewrite("/a/b/c e /a/d e /x"), "/y/c e /x/d e /z");
    }

    #[test]
    fn rewriter_ignora_origem_vazia_e_igual_ao_destino() {
        let rewrite = path_rewriter(&[pair("", "/x"), pair("/k", "/k")]);
        assert_eq!(rewrite("abc /k"), "abc /k");
    }

    #[test]
    fn rewriter_escapa_metacaractere_e_o_ultimo_par_igual_vence() {
        let rewrite = path_rewriter(&[pair("/a.b", "/1"), pair("/a.b", "/2")]);
        assert_eq!(rewrite("/a.b /aXb"), "/2 /aXb");
    }

    #[test]
    fn rewrite_json_so_toca_valores_string() {
        let value: Value =
            serde_json::from_str(r#"{"/a":"/a/x","n":1,"l":["/a",true,null]}"#).unwrap();
        let rewritten = rewrite_json(value, &|t: &str| t.replace("/a", "/b"));
        assert_eq!(
            rewritten.to_string(),
            r#"{"/a":"/b/x","n":1,"l":["/b",true,null]}"#
        );
    }

    #[test]
    fn safe_name_recusa_o_que_escapa_do_diretorio() {
        for ok in ["fake-a", "_x", "a.b", "demo"] {
            assert!(safe_name(ok, "t").is_ok(), "{ok}");
        }
        for bad in ["", ".", "..", ".x", "-x", "a/b", "a b"] {
            assert_eq!(
                safe_name(bad, "agente").unwrap_err().0,
                format!("nome inseguro no pacote (agente): {bad}")
            );
        }
    }

    #[test]
    fn join_e_dirname_seguem_o_node() {
        assert_eq!(node_join("/a/b", "c"), "/a/b/c");
        assert_eq!(node_join("/a/b/", "c"), "/a/b/c");
        assert_eq!(node_join("/a/b", ".."), "/a");
        assert_eq!(node_join("/a//b", "./c"), "/a/b/c");
        assert_eq!(node_join("a", ".."), ".");
        assert_eq!(node_join("../a", "b"), "../a/b");
        assert_eq!(node_dirname("/a/b/c"), "/a/b");
        assert_eq!(node_dirname("/a"), "/");
        assert_eq!(node_dirname("a"), ".");
        assert_eq!(node_dirname("/a/b/"), "/a");
    }

    #[test]
    fn plans_dir_volta_para_o_home_so_quando_veio_de_hades_plans() {
        let env = Env::empty().with("HOME", "/h");
        assert_eq!(
            plans_dir_of(&env, "/x/.claude/hades/plans/d.md", None),
            "/h/.claude/hades/plans"
        );
        assert_eq!(plans_dir_of(&env, "/proj/docs/d.md", None), "/proj/docs");
        assert_eq!(plans_dir_of(&env, "/proj/docs/d.md", Some("/o")), "/o");
    }

    #[test]
    fn work_root_local_segue_a_variavel() {
        let env = Env::empty()
            .with("HOME", "/h")
            .with("HADES_WORK_ROOT", "/w");
        assert_eq!(local_work_root(&env), "/w");
        assert_eq!(
            local_work_root(&Env::empty().with("HOME", "/nao-existe")),
            "/nao-existe/.claude/hades/work"
        );
    }

    #[test]
    fn parse_record_distingue_json_invalido_de_nao_objeto() {
        let bad = Part::new("orchestration/agents/x.json", "{");
        assert_eq!(
            parse_record(&bad).unwrap_err().0,
            "orchestration/agents/x.json do pacote não é JSON válido"
        );
        let list = Part::new("orchestration/plan.json", "[1]");
        assert_eq!(
            parse_record(&list).unwrap_err().0,
            "orchestration/plan.json do pacote não é um objeto JSON"
        );
    }
}
