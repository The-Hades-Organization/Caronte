//! `caronte pack` e `caronte brief` de ponta a ponta sobre a fixture `orq` da matriz de paridade.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const PASSWORD: &str = "senha de teste 123";

fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity")
}

fn copy_tree(from: &Path, to: &Path, home: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target, home);
        } else {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(&target, text.replace("@HOME@", home)).unwrap();
        }
    }
}

fn home_with(setup: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    copy_tree(
        &parity_dir().join("fixtures/orq"),
        &home,
        &home.to_string_lossy(),
    );
    let status = Command::new("sh")
        .arg(parity_dir().join(setup))
        .current_dir(&home)
        .env("HOME", &home)
        .status()
        .unwrap();
    assert!(status.success(), "{setup}");
    dir
}

fn caronte(home: &Path, args: &[&str], password: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_caronte"));
    cmd.args(args)
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HADES_WORK_ROOT", home.join("work"))
        .env("HADES_NOW", "2026-11-20T12:00:00Z")
        .env("HADES_CARONTE_SCRYPT_LOG2N", "14");
    if let Some(p) = password {
        cmd.env("HADES_CARONTE_PASSWORD", p);
    }
    cmd.output().unwrap()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn pack_grava_pacote_modo_600() {
    let dir = home_with("setup-orq.sh");
    let home = dir.path().join("home");
    let file = home.join("out/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--out", file.to_str().unwrap()],
        Some(PASSWORD),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("pacote gravado:"), "{stdout}");
    assert!(stdout.contains("modo 600"), "{stdout}");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(fs::metadata(&file).unwrap().len() > 0);
    assert!(!home.join("out/p.caronte.tmp").exists());
}

#[test]
fn pack_dry_run_nao_grava_nem_pede_senha() {
    let dir = home_with("setup-orq.sh");
    let home = dir.path().join("home");
    let file = home.join("out/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--dry-run", "--out", file.to_str().unwrap()],
        None,
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("o pack seguiria"));
    assert!(!file.exists());
}

#[test]
fn pack_recusa_out_existente() {
    let dir = home_with("setup-orq-out-existe.sh");
    let home = dir.path().join("home");
    let file = home.join("out/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--out", file.to_str().unwrap()],
        Some(PASSWORD),
    );
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("já existe"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn pack_recusa_out_dentro_de_repo() {
    let dir = home_with("setup-orq.sh");
    let home = dir.path().join("home");
    let file = home.join("trees/fake-a/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--out", file.to_str().unwrap()],
        Some(PASSWORD),
    );
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("dentro de um repositório"));
    assert!(!file.exists());
}

#[test]
fn pack_senha_vazia_e_uso() {
    let dir = home_with("setup-orq.sh");
    let home = dir.path().join("home");
    let file = home.join("out/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--out", file.to_str().unwrap()],
        Some(""),
    );
    assert_eq!(code(&out), 2);
    assert!(!file.exists());
}

#[test]
fn pack_recusa_agente_trabalhando() {
    let dir = home_with("setup-orq-trabalhando.sh");
    let home = dir.path().join("home");
    let file = home.join("out/p.caronte");
    let out = caronte(
        &home,
        &["pack", "demo", "--out", file.to_str().unwrap()],
        Some(PASSWORD),
    );
    assert_eq!(code(&out), 1, "{}", text(&out.stderr));
    assert!(!file.exists());
}

#[test]
fn brief_imprime_prompt_de_retomada() {
    let dir = home_with("setup-orq-brief.sh");
    let home = dir.path().join("home");
    let out = caronte(&home, &["brief", "demo"], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("demo.md"));
}

#[test]
fn sem_subcomando_e_uso() {
    let dir = home_with("setup-orq.sh");
    let out = caronte(&dir.path().join("home"), &[], None);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stdout).starts_with("uso: hades caronte pack"));
}
