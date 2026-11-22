use caronte_core::guard::Rule;

use super::*;

fn agente(nome: &str, estado: &str, prontidao: Readiness) -> PlanAgent {
    PlanAgent {
        name: nome.to_owned(),
        state: estado.to_owned(),
        readiness: prontidao,
        worktree: format!("/w/{nome}"),
    }
}

fn plano(nome: &str, ms: u64, agentes: Vec<PlanAgent>) -> PlanoResumo {
    PlanoResumo {
        nome: nome.to_owned(),
        modificado_ms: ms,
        agentes,
    }
}

fn previa() -> PackPreview {
    PackPreview {
        parts: vec![("plan/a.md".to_owned(), 2150)],
        raw_bytes: 2150,
        agents: vec!["a".to_owned()],
        worktrees: Vec::new(),
        blocked: Vec::new(),
        allowed: Vec::new(),
    }
}

#[test]
fn teclas_trocam_de_tela() {
    assert_eq!(tela_da_tecla("1"), Some(Tela::Planos));
    assert_eq!(tela_da_tecla("2"), Some(Tela::Previa));
    assert_eq!(tela_da_tecla("0"), None);
    assert_eq!(tela_da_tecla("3"), None);
    assert_eq!(tela_da_tecla("a"), None);
    assert_eq!(Tela::de_rotulo("Planos"), Some(Tela::Planos));
    assert_eq!(Tela::de_rotulo("nada"), None);
}

#[test]
fn nome_de_plano_segue_o_padrao_do_painel() {
    assert!(plano_valido("hades-rust"));
    assert!(plano_valido("a.b_c-1"));
    assert!(plano_valido(&"a".repeat(100)));
    assert!(!plano_valido(&"a".repeat(101)));
    assert!(!plano_valido(""));
    assert!(!plano_valido("-x"));
    assert!(!plano_valido(".x"));
    assert!(!plano_valido("a/b"));
    assert!(!plano_valido("a b"));
    assert!(!plano_valido("../x"));
}

#[test]
fn planos_do_mais_novo_ao_mais_antigo_com_empate_pelo_nome() {
    let mut lista = vec![
        plano("b", 10, vec![]),
        plano("velho", 1, vec![]),
        plano("a", 10, vec![]),
        plano("novo", 99, vec![]),
    ];
    ordenar_planos(&mut lista);
    let nomes: Vec<&str> = lista.iter().map(|p| p.nome.as_str()).collect();
    assert_eq!(nomes, ["novo", "a", "b", "velho"]);
}

#[test]
fn plano_pronto_quando_ninguem_roda() {
    let pronto = plano(
        "p",
        0,
        vec![
            agente("a", "parked", Readiness::Parked),
            agente("b", "done", Readiness::Ended),
        ],
    );
    assert!(pronto.pronto());
    let faltando = plano(
        "p",
        0,
        vec![
            agente("a", "parked", Readiness::Parked),
            agente("b", "working", Readiness::Working),
        ],
    );
    assert!(!faltando.pronto());
    assert_eq!(faltando.faltam()[0].name, "b");
    assert!(plano("vazio", 0, vec![]).pronto());
}

#[test]
fn linhas_de_planos_contam_prontos_e_idade() {
    let p = plano(
        "hades-rust",
        0,
        vec![
            agente("a", "parked", Readiness::Parked),
            agente("b", "working", Readiness::Working),
            agente("c", "done", Readiness::Ended),
        ],
    );
    let linhas = linhas_planos(&[p], 7_200_000);
    assert_eq!(linhas[0], ["hades-rust", "3", "2/3", "há 2 h"]);
}

#[test]
fn linhas_de_agentes_mostram_estado_e_worktree() {
    let mut sem_tree = agente("c", "?", Readiness::Working);
    sem_tree.worktree = String::new();
    let linhas = linhas_agentes(&[agente("a", "parked", Readiness::Parked), sem_tree]);
    assert_eq!(linhas[0], ["a", "parked", "PARKED", "/w/a"]);
    assert_eq!(linhas[1], ["c", "?", "rodando", "—"]);
}

#[test]
fn idade_em_unidades_inteiras() {
    assert_eq!(idade(1_000, 0), "agora");
    assert_eq!(idade(60_000, 0), "há 1 min");
    assert_eq!(idade(3_599_000, 0), "há 59 min");
    assert_eq!(idade(3_600_000, 0), "há 1 h");
    assert_eq!(idade(86_400_000, 0), "há 1 d");
    assert_eq!(idade(0, 5_000), "agora");
}

#[test]
fn tamanhos_com_virgula_decimal() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(1023), "1023 B");
    assert_eq!(bytes(2150), "2,1 KB");
    assert_eq!(bytes(1024 * 1024), "1,0 MB");
    assert_eq!(bytes(2_936_012), "2,8 MB");
    assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3,0 GB");
}

#[test]
fn comandos_prontos_com_aspas_so_quando_preciso() {
    assert_eq!(comando_park("moirai-port"), "hades agent park moirai-port");
    assert_eq!(
        comando_pack("hades-rust", &[]),
        "! hades caronte pack hades-rust"
    );
    assert_eq!(comando_dry_run("a.b"), "hades caronte pack a.b --dry-run");
    assert_eq!(
        comando_pack("p", &["reports/a/.env".to_owned(), "x y".to_owned()]),
        "! hades caronte pack p --allow reports/a/.env --allow 'x y'"
    );
    assert_eq!(aspas("it's"), "'it'\\''s'");
    assert_eq!(aspas(""), "''");
}

#[test]
fn sugestao_de_allow_nao_repete_caminho() {
    let achados = vec![
        Finding {
            path: "a/.env".to_owned(),
            rule: Rule::Nome,
        },
        Finding {
            path: "a/.env".to_owned(),
            rule: Rule::Conteudo,
        },
        Finding {
            path: "b/x".to_owned(),
            rule: Rule::Conteudo,
        },
    ];
    assert_eq!(sugestao_allow(&achados), ["a/.env", "b/x"]);
    assert_eq!(
        linhas_achados(&achados)[1],
        ["a/.env", "conteudo"],
        "só caminho e regra, nunca o valor"
    );
}

#[test]
fn botao_estacionar_explica_por_que_esta_desabilitado() {
    let rodando = agente("a", "working", Readiness::Working);
    assert_eq!(pode_estacionar(Some(&rodando), true, false), Ok(()));
    assert_eq!(
        pode_estacionar(None, true, false),
        Err("Selecione um agente do plano")
    );
    assert_eq!(
        pode_estacionar(Some(&rodando), false, false),
        Err("O comando hades não está no PATH")
    );
    assert_eq!(
        pode_estacionar(Some(&rodando), true, true),
        Err("Este agente já está sendo estacionado")
    );
    let parked = agente("a", "parked", Readiness::Parked);
    assert!(pode_estacionar(Some(&parked), true, false).is_err());
    let assentado_no_registro = agente("a", "archived", Readiness::Working);
    assert!(pode_estacionar(Some(&assentado_no_registro), true, false).is_err());
}

#[test]
fn guarda_limpa_ou_com_achados() {
    assert_eq!(rotulo_guarda(0), "limpo");
    assert_eq!(rotulo_guarda(1), "1 achado");
    assert_eq!(rotulo_guarda(3), "3 achados");
    assert_eq!(tom_guarda(0), Tone::Ok);
    assert_eq!(tom_guarda(2), Tone::Bad);
}

#[test]
fn tabelas_da_previa() {
    let mut p = previa();
    p.worktrees = vec![WorktreeInfo {
        agent: "a".to_owned(),
        path: "/w/a".to_owned(),
        branch: "f3/a".to_owned(),
        head: "0123456789abcdef".to_owned(),
        base_ref: "origin/main".to_owned(),
        base_sha: "f".repeat(40),
        bundle: Some("git/a.bundle".to_owned()),
        patch: None,
    }];
    assert_eq!(linhas_partes(&p)[0], ["plan/a.md", "2,1 KB"]);
    assert_eq!(
        linhas_worktrees(&p.worktrees)[0],
        ["a", "f3/a", "0123456789", "git/a.bundle", "—"]
    );
}

#[test]
fn tarefa_descarta_resultado_de_geracao_velha() {
    let mut t = Tarefa::default();
    let g = t.iniciar().unwrap();
    assert!(t.carregando());
    assert_eq!(t.iniciar(), None, "só uma em voo");
    t.invalidar();
    assert!(!t.carregando());
    assert!(!t.concluir(g), "resultado velho é descartado");
    let g2 = t.iniciar().unwrap();
    assert_ne!(g, g2);
    assert!(t.concluir(g2));
    assert!(!t.carregando());
}

#[test]
fn estacionar_bloqueia_o_segundo_clique_no_mesmo_agente() {
    let mut e = Estacionando::default();
    assert!(e.iniciar("a"));
    assert!(!e.iniciar("a"));
    assert!(e.iniciar("b"));
    assert!(e.contem("a"));
    e.concluir("a");
    assert!(!e.contem("a"));
    assert!(e.iniciar("a"));
}

#[test]
fn cache_da_previa_vale_30_segundos() {
    let mut c = CachePrevia::default();
    assert!(c.obter("p", 0).is_none());
    c.guardar("p", 1_000, previa());
    assert!(c.obter("p", 30_999).is_some());
    assert!(c.obter("p", 31_000).is_none());
    assert!(c.obter("outro", 1_000).is_none());
    c.esquecer("p");
    assert!(c.obter("p", 1_000).is_none());
}

#[test]
fn corte_respeita_caractere() {
    assert_eq!(cortar("abc", 5), "abc");
    assert_eq!(cortar("abcdef", 3), "abc…");
    assert_eq!(cortar("ããã", 3), "ã…");
}

#[test]
fn numericas_a_direita() {
    assert!(coluna_numerica("Tamanho"));
    assert!(coluna_numerica("Prontos"));
    assert!(!coluna_numerica("Plano"));
}
