use super::*;
use std::fs;
use tempfile::TempDir;

const KIT: &str = "\
HADES_CARONTE_OUT_DIR\t\tpath\tOrquestração\tPasta dos pacotes\tnext\tOnde o pack grava.\n\
HADES_CARONTE_SCRYPT_LOG2N\t17\tint:14:20\tOrquestração\tCusto do scrypt\tnext\tCusto.\n\
HADES_OUTRA\t1\tbool\tG\tOutra\tnext\tNão é do Caronte.\n";

struct Sandbox {
    _dir: TempDir,
    env: Env,
    store: ConfigStore,
    cwd: std::path::PathBuf,
}

fn sandbox() -> Sandbox {
    let dir = TempDir::new().unwrap();
    let skill = dir.path().join("skill");
    let kit = skill.join("kit");
    let dados = dir.path().join("dados");
    fs::create_dir_all(&kit).unwrap();
    fs::create_dir_all(&dados).unwrap();
    fs::write(kit.join("config.tsv"), KIT).unwrap();
    let env = Env::empty()
        .with("HOME", dir.path().to_string_lossy())
        .with("HADES_SKILL_DIR", skill.to_string_lossy())
        .with("HADES_DATA", dados.to_string_lossy());
    Sandbox {
        cwd: dir.path().to_path_buf(),
        store: ConfigStore { dir: dados, kit },
        env,
        _dir: dir,
    }
}

fn textos(linhas: &[Linha]) -> Vec<String> {
    linhas.iter().map(|l| l.valor.clone()).collect()
}

const OUT: &str = "HADES_CARONTE_OUT_DIR";
const N: &str = "HADES_CARONTE_SCRYPT_LOG2N";

#[test]
fn mostra_so_as_chaves_do_caronte_com_padrao_e_faixa() {
    let s = sandbox();
    let linhas = ConfigLocal::new(&s.env, &s.cwd).carregar().unwrap();
    let chaves: Vec<_> = linhas.iter().map(|l| l.chave.as_str()).collect();
    assert_eq!(chaves, [OUT, N]);
    assert_eq!(linhas[0].faixa(), None);
    assert_eq!(linhas[0].valor, "");
    assert_eq!(linhas[1].faixa(), Some((14, 20)));
    assert_eq!(linhas[1].valor, "17");
    assert_eq!(linhas[1].origem, Origem::Padrao);
}

#[test]
fn gravar_pela_pagina_e_ler_pelo_config_value_do_cli() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = " 18 ".into();
    t[0] = "~/pacotes".into();
    assert_eq!(pendentes(&linhas, &t), 2);
    assert_eq!(salvar(&fonte, &linhas, &t), Ok(2));
    assert_eq!(config::config_value(&s.env, &s.cwd, N, ""), "18");
    assert_eq!(config::config_value(&s.env, &s.cwd, OUT, ""), "~/pacotes");
    let depois = fonte.carregar().unwrap();
    assert_eq!(depois[1].origem, Origem::Arquivo);
    assert_eq!(pendentes(&depois, &textos(&depois)), 0);
}

#[test]
fn grava_no_config_env_do_estado_e_nao_na_skill() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = "20".into();
    salvar(&fonte, &linhas, &t).unwrap();
    let env_txt = fs::read_to_string(s.store.dir.join("config.env")).unwrap();
    assert!(env_txt.contains("HADES_CARONTE_SCRYPT_LOG2N=20\n"));
    assert_eq!(
        fs::read_to_string(s.store.kit.join("config.tsv")).unwrap(),
        KIT
    );
}

#[test]
fn valor_fora_da_faixa_ou_nao_inteiro_e_recusado_sem_gravar() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    for ruim in ["13", "21", "-3", "abc", "", "17.5", "+17", "1e2"] {
        let mut t = textos(&linhas);
        t[1] = ruim.into();
        assert!(erros(&linhas, &t)[1].is_some(), "{ruim}");
        assert!(salvar(&fonte, &linhas, &t).is_err(), "{ruim}");
    }
    assert!(!s.store.dir.join("config.env").exists());
    let mut t = textos(&linhas);
    t[1] = "21".into();
    assert_eq!(
        erros(&linhas, &t)[1].as_deref(),
        Some("Informe um número inteiro entre 14 e 20")
    );
}

#[test]
fn caminho_vazio_absoluto_ou_com_til_passa_e_o_resto_e_recusado() {
    let s = sandbox();
    let linhas = ConfigLocal::new(&s.env, &s.cwd).carregar().unwrap();
    for bom in ["", "~/x", "/var/pacotes", "  /a/b  "] {
        assert!(validar(&linhas[0], bom).is_ok(), "{bom}");
    }
    for ruim in [
        "relativo/x",
        "/com espaço",
        "/aspas\"",
        "/$HOME",
        "/a`b",
        "/a\\b",
        "/a'b",
    ] {
        assert!(validar(&linhas[0], ruim).is_err(), "{ruim}");
    }
    let mut t = textos(&linhas);
    t[0] = "relativo".into();
    assert!(salvar(&ConfigLocal::new(&s.env, &s.cwd), &linhas, &t).is_err());
    assert!(!s.store.dir.join("config.env").exists());
}

#[test]
fn zeros_a_esquerda_normalizam_como_o_ts() {
    let s = sandbox();
    let linhas = ConfigLocal::new(&s.env, &s.cwd).carregar().unwrap();
    assert_eq!(validar(&linhas[1], " 018 "), Ok("18".to_owned()));
}

#[test]
fn restaurar_padrao_tira_a_chave_do_arquivo() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = "19".into();
    salvar(&fonte, &linhas, &t).unwrap();

    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = linhas[1].padrao.clone();
    assert_eq!(pendentes(&linhas, &t), 1);
    assert_eq!(salvar(&fonte, &linhas, &t), Ok(1));
    assert_eq!(config::config_value(&s.env, &s.cwd, N, ""), "17");
    let env_txt = fs::read_to_string(s.store.dir.join("config.env")).unwrap();
    assert!(!env_txt.contains(N));
    assert_eq!(fonte.carregar().unwrap()[1].origem, Origem::Padrao);
}

#[test]
fn restaurar_caminho_vazio_volta_ao_padrao() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[0] = "/srv/pacotes".into();
    salvar(&fonte, &linhas, &t).unwrap();
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[0] = String::new();
    assert_eq!(salvar(&fonte, &linhas, &t), Ok(1));
    assert_eq!(config::config_value(&s.env, &s.cwd, OUT, ""), "");
    assert_eq!(fonte.carregar().unwrap()[0].origem, Origem::Padrao);
}

#[test]
fn valor_do_ambiente_vence_o_arquivo_e_fica_so_leitura() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = "19".into();
    salvar(&fonte, &linhas, &t).unwrap();

    let env = s.env.with(N, "15");
    let fonte = ConfigLocal::new(&env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    assert_eq!(linhas[1].valor, "15");
    assert_eq!(linhas[1].origem, Origem::Ambiente);
    assert!(linhas[1].somente_leitura());
    assert_eq!(config::config_value(&env, &s.cwd, N, ""), "15");
    let mut t = textos(&linhas);
    t[1] = "16".into();
    assert_eq!(pendentes(&linhas, &t), 0);
    assert_eq!(erros(&linhas, &t)[1], None);
    assert_eq!(salvar(&fonte, &linhas, &t), Ok(0));
    let env_txt = fs::read_to_string(s.store.dir.join("config.env")).unwrap();
    assert!(env_txt.contains("HADES_CARONTE_SCRYPT_LOG2N=19\n"));
}

#[test]
fn uma_linha_invalida_impede_gravar_as_outras() {
    let s = sandbox();
    let fonte = ConfigLocal::new(&s.env, &s.cwd);
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[0] = "/srv/ok".into();
    t[1] = "0".into();
    let erro = salvar(&fonte, &linhas, &t).unwrap_err();
    assert!(erro.starts_with("Custo do scrypt:"), "{erro}");
    assert!(!s.store.dir.join("config.env").exists());
}

#[test]
fn sem_instalacao_nao_grava() {
    let dir = TempDir::new().unwrap();
    let skill = dir.path().join("skill");
    fs::create_dir_all(skill.join("kit")).unwrap();
    fs::write(skill.join("kit/config.tsv"), KIT).unwrap();
    let env = Env::empty()
        .with("HOME", dir.path().to_string_lossy())
        .with("HADES_SKILL_DIR", skill.to_string_lossy());
    let fonte = ConfigLocal::new(&env, dir.path());
    assert!(!fonte.gravavel());
    let linhas = fonte.carregar().unwrap();
    let mut t = textos(&linhas);
    t[1] = "20".into();
    assert!(salvar(&fonte, &linhas, &t).is_err());
    assert!(!skill.join("kit/config.env").exists());
}

#[test]
fn faixa_do_tipo_so_para_int() {
    assert_eq!(faixa_do_tipo("int:14:20"), Some((14, 20)));
    assert_eq!(faixa_do_tipo("path"), None);
}

#[test]
fn dica_do_vazio_so_na_pasta_dos_pacotes_e_custo_de_memoria_explicado() {
    let s = sandbox();
    let linhas = ConfigLocal::new(&s.env, &s.cwd).carregar().unwrap();
    assert_eq!(linhas[0].dica_do_vazio(), Some("~/.claude/hades/caronte"));
    assert_eq!(linhas[1].dica_do_vazio(), None);
    assert!(linhas[1].descricao_completa().contains("17 = 128 MB"));
    assert_eq!(
        custo_de_memoria("14"),
        "Com 14 a derivação usa 16 MB de memória; cada passo a mais dobra o custo (17 = 128 MB)."
    );
    assert!(custo_de_memoria("17").contains("128 MB"));
    assert!(custo_de_memoria("20").contains("1024 MB"));
    assert!(custo_de_memoria("x").contains("17 usa 128 MB"));
}
