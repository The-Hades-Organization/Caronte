//! View-model do app: formatação, tons, comandos prontos e controle das tarefas de fundo, sem janela nem I/O.
//! Regras do painel TS (`src/panel/caronte.ts`): nome de plano `PLAN_NAME` (PC:40), cache da prévia de 30 s (PC:46),
//! estados que o `agent park` recusa em silêncio (PC:52) e comandos prontos (PC:81).

use std::collections::{HashMap, HashSet};

use caronte_core::guard::Finding;
use caronte_core::manifest::WorktreeInfo;
use caronte_core::plan::{PackPreview, PlanAgent, Readiness};
use hades_ui::Tone;

/// Teto de pacotes listados (PC:45, `MAX_PACKAGES`).
pub const MAX_PACOTES: usize = 200;
/// Teto de planos listados (PC:44, `MAX_PLANS`).
pub const MAX_PLANOS: usize = 100;
/// Validade da prévia em cache (PC:46, `PREVIEW_TTL_MS`).
pub const VALIDADE_PREVIA_MS: u64 = 30_000;
/// Espera padrão do `hades agent park` em segundos (PC:47, `TIMEOUT_DEFAULT_S`).
pub const ESPERA_PARK_PADRAO_S: u64 = 1200;
/// Teto da saída do park mostrada no rodapé (PC:48, `OUTPUT_MAX`).
pub const SAIDA_PARK_MAX: usize = 2000;

pub const COLUNAS_PLANOS: [&str; 4] = ["Plano", "Agentes", "Prontos", "Atualizado"];
pub const COLUNAS_AGENTES: [&str; 4] = ["Agente", "Estado", "Prontidão", "Worktree"];
pub const COLUNAS_PARTES: [&str; 2] = ["Parte", "Tamanho"];
pub const COLUNAS_WORKTREES: [&str; 5] = ["Agente", "Branch", "Head", "Bundle", "Patch"];
pub const COLUNAS_PACOTES: [&str; 4] = ["Pacote", "Tamanho", "Atualizado", "Formato"];
pub const COLUNAS_ACHADOS: [&str; 2] = ["Caminho", "Regra"];

/// Telas da navegação lateral (`design/caronte.html`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tela {
    Planos,
    Previa,
    Pacotes,
    Retomar,
    Config,
}

impl Tela {
    pub const TODAS: [Tela; 5] = [
        Tela::Planos,
        Tela::Previa,
        Tela::Pacotes,
        Tela::Retomar,
        Tela::Config,
    ];
    /// Telas com atalho numérico; Configurações fica de fora porque tem campos de texto.
    pub const COM_ATALHO: [Tela; 4] = [Tela::Planos, Tela::Previa, Tela::Pacotes, Tela::Retomar];

    pub fn rotulo(self) -> &'static str {
        match self {
            Tela::Planos => "Planos",
            Tela::Previa => "Prévia do pack",
            Tela::Pacotes => "Pacotes",
            Tela::Retomar => "Retomar",
            Tela::Config => "Configurações",
        }
    }

    pub fn de_flag(nome: &str) -> Option<Tela> {
        match nome {
            "planos" => Some(Tela::Planos),
            "previa" => Some(Tela::Previa),
            "pacotes" => Some(Tela::Pacotes),
            "retomar" => Some(Tela::Retomar),
            "configuracoes" => Some(Tela::Config),
            _ => None,
        }
    }

    pub fn de_rotulo(rotulo: &str) -> Option<Tela> {
        Tela::TODAS.into_iter().find(|t| t.rotulo() == rotulo)
    }
}

/// Atalho: teclas 1 a 4 trocam de tela, como o clique no `Rail`.
pub fn tela_da_tecla(tecla: &str) -> Option<Tela> {
    let n: usize = tecla.parse().ok()?;
    Tela::COM_ATALHO.get(n.checked_sub(1)?).copied()
}

/// `1 plano`, `3 planos`.
pub fn planos(n: usize) -> String {
    format!("{n} {}", if n == 1 { "plano" } else { "planos" })
}

/// `1 configuração`, `2 configurações`.
pub fn configuracoes(n: usize) -> String {
    format!(
        "{n} {}",
        if n == 1 {
            "configuração"
        } else {
            "configurações"
        }
    )
}

/// PC:40 (`PLAN_NAME`): `^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$`.
pub fn plano_valido(nome: &str) -> bool {
    let mut chars = nome.chars();
    let Some(primeiro) = chars.next() else {
        return false;
    };
    primeiro.is_ascii_alphanumeric()
        && nome.len() <= 100
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Plano da lista: arquivo de `plans/` sem `.md`, com os agentes ligados e a prontidão de cada um.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanoResumo {
    pub nome: String,
    pub modificado_ms: u64,
    pub agentes: Vec<PlanAgent>,
}

impl PlanoResumo {
    pub fn pronto(&self) -> bool {
        self.agentes
            .iter()
            .all(|a| a.readiness != Readiness::Working)
    }

    pub fn faltam(&self) -> Vec<&PlanAgent> {
        self.agentes
            .iter()
            .filter(|a| a.readiness == Readiness::Working)
            .collect()
    }
}

/// Mais novo primeiro; empate pelo nome (PC:99).
pub fn ordenar_planos(planos: &mut [PlanoResumo]) {
    planos.sort_by(|a, b| {
        b.modificado_ms
            .cmp(&a.modificado_ms)
            .then_with(|| a.nome.cmp(&b.nome))
    });
}

pub fn rotulo_prontidao(prontidao: Readiness) -> &'static str {
    match prontidao {
        Readiness::Parked => "PARKED",
        Readiness::Ended => "encerrado",
        Readiness::Working => "rodando",
    }
}

pub fn tom_prontidao(prontidao: Readiness) -> Tone {
    match prontidao {
        Readiness::Parked => Tone::Ok,
        Readiness::Ended => Tone::Neutral,
        Readiness::Working => Tone::Warn,
    }
}

pub fn linhas_planos(planos: &[PlanoResumo], agora_ms: u64) -> Vec<[String; 4]> {
    planos
        .iter()
        .map(|p| {
            let prontos = p.agentes.len() - p.faltam().len();
            [
                p.nome.clone(),
                p.agentes.len().to_string(),
                format!("{prontos}/{}", p.agentes.len()),
                idade(agora_ms, p.modificado_ms),
            ]
        })
        .collect()
}

pub fn linhas_agentes(agentes: &[PlanAgent]) -> Vec<[String; 4]> {
    agentes
        .iter()
        .map(|a| {
            [
                a.name.clone(),
                a.state.clone(),
                rotulo_prontidao(a.readiness).to_owned(),
                if a.worktree.is_empty() {
                    "—".to_owned()
                } else {
                    a.worktree.clone()
                },
            ]
        })
        .collect()
}

/// `há 5 min`, `há 3 h`, `há 2 d`; instante no futuro (relógio desajustado) vira `agora`.
pub fn idade(agora_ms: u64, instante_ms: u64) -> String {
    let s = agora_ms.saturating_sub(instante_ms) / 1000;
    match s {
        0..=59 => "agora".to_owned(),
        60..=3599 => format!("há {} min", s / 60),
        3600..=86_399 => format!("há {} h", s / 3600),
        _ => format!("há {} d", s / 86_400),
    }
}

/// `2,1 KB`, `2,8 MB`: base 1024, uma casa, vírgula decimal.
pub fn bytes(n: usize) -> String {
    const KB: f64 = 1024.0;
    let v = n as f64;
    if n < 1024 {
        return format!("{n} B");
    }
    let (valor, unidade) = if v < KB * KB {
        (v / KB, "KB")
    } else if v < KB * KB * KB {
        (v / (KB * KB), "MB")
    } else {
        (v / (KB * KB * KB), "GB")
    };
    format!("{valor:.1} {unidade}").replace('.', ",")
}

/// Aspas simples só quando o nome pede (`shlexQuote`, PC:76).
pub fn aspas(texto: &str) -> String {
    let simples = !texto.is_empty()
        && texto.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@' | '%' | '+')
        });
    if simples {
        texto.to_owned()
    } else {
        format!("'{}'", texto.replace('\'', "'\\''"))
    }
}

/// PC:81 (`parkCommand`).
pub fn comando_park(agente: &str) -> String {
    format!("hades agent park {agente}")
}

/// O pack roda no terminal, onde a senha passa pelo tty: o app só mostra o comando.
pub fn comando_pack(plano: &str, allow: &[String]) -> String {
    let mut comando = format!("! hades caronte pack {}", aspas(plano));
    for caminho in allow {
        comando.push_str(&format!(" --allow {}", aspas(caminho)));
    }
    comando
}

/// O unpack pede a senha no tty: o app mostra o comando e nunca abre nem cifra pacote.
pub fn comando_unpack(arquivo: &str) -> String {
    format!("! hades caronte unpack {}", aspas(arquivo))
}

pub fn comando_workspace(worktree: &str, agente: &str) -> String {
    format!(
        "herdr workspace create --cwd {} --label hd-{agente} --no-focus",
        aspas(worktree)
    )
}

pub fn comando_adopt(agente: &str) -> String {
    format!("hades agent adopt {agente} && hades agent resume {agente}")
}

/// Pacote `.caronte` da pasta de saída; só metadados do arquivo e do cabeçalho, nunca o conteúdo cifrado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacoteResumo {
    pub nome: String,
    pub caminho: String,
    pub tamanho: u64,
    pub modificado_ms: u64,
    pub formato: Option<i64>,
}

/// Mais novo primeiro; empate pelo nome (PC:123-131).
pub fn ordenar_pacotes(pacotes: &mut [PacoteResumo]) {
    pacotes.sort_by(|a, b| {
        b.modificado_ms
            .cmp(&a.modificado_ms)
            .then_with(|| a.nome.cmp(&b.nome))
    });
}

pub fn linhas_pacotes(pacotes: &[PacoteResumo], agora_ms: u64) -> Vec<[String; 4]> {
    pacotes
        .iter()
        .map(|p| {
            [
                p.nome.clone(),
                bytes(p.tamanho as usize),
                idade(agora_ms, p.modificado_ms),
                p.formato
                    .map_or_else(|| "ilegível".to_owned(), |v| format!("v{v}")),
            ]
        })
        .collect()
}

/// Versão do formato (`v`) lida só do cabeçalho em claro: MAGIC, u32 BE do tamanho e JSON. Sem senha, sem decifrar.
pub fn formato_do_cabecalho(inicio: &[u8]) -> Option<i64> {
    let magic = caronte_core::format::MAGIC;
    let corpo = inicio.strip_prefix(magic.as_slice())?;
    let (tam, resto) = corpo.split_at_checked(4)?;
    let tam = u32::from_be_bytes(tam.try_into().ok()?) as usize;
    let json: serde_json::Value = serde_json::from_slice(resto.get(..tam)?).ok()?;
    json.get("v")?.as_i64()
}

/// Bytes a ler do começo do arquivo para cobrir o cabeçalho (teto do formato: 4096, mais MAGIC e tamanho).
pub const BYTES_CABECALHO: usize = 8 + 4 + 4096;

/// Passos de retomada de um agente estacionado, como `resumeLines` imprime (CC:236-249).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetomadaAgente {
    pub nome: String,
    pub workspace: String,
    pub iniciar: String,
    pub adotar: String,
}

/// Prompt do orquestrador e os passos por agente do plano selecionado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retomada {
    pub prompt: String,
    pub agentes: Vec<RetomadaAgente>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum EstadoRetomada {
    #[default]
    SemPlano,
    Carregando,
    Pronta(Box<Retomada>),
    Falhou(String),
}

pub fn comando_dry_run(plano: &str) -> String {
    format!("hades caronte pack {} --dry-run", aspas(plano))
}

/// Caminhos que o `--allow` liberaria, sem repetir (a guarda nunca devolve o valor, só caminho e regra).
pub fn sugestao_allow(achados: &[Finding]) -> Vec<String> {
    let mut vistos = HashSet::new();
    achados
        .iter()
        .filter(|f| vistos.insert(f.path.clone()))
        .map(|f| f.path.clone())
        .collect()
}

/// PC:52 (`ALREADY_SETTLED`): o `agent park` nesses estados só avisa e sai 0.
pub fn estado_assentado(estado: &str) -> bool {
    matches!(estado, "parked" | "done" | "archived")
}

/// `Ok` se o botão Estacionar vale para o agente; senão o motivo que o botão mostra desabilitado.
pub fn pode_estacionar(
    agente: Option<&PlanAgent>,
    hades_no_path: bool,
    em_voo: bool,
) -> Result<(), &'static str> {
    let Some(a) = agente else {
        return Err("Selecione um agente do plano");
    };
    if !hades_no_path {
        return Err("O comando hades não está no PATH");
    }
    if em_voo {
        return Err("Este agente já está sendo estacionado");
    }
    if a.readiness != Readiness::Working || estado_assentado(&a.state) {
        return Err("O agente já está estacionado ou encerrado");
    }
    Ok(())
}

pub fn tom_guarda(achados: usize) -> Tone {
    if achados == 0 { Tone::Ok } else { Tone::Bad }
}

pub fn rotulo_guarda(achados: usize) -> String {
    match achados {
        0 => "limpo".to_owned(),
        1 => "1 achado".to_owned(),
        n => format!("{n} achados"),
    }
}

pub fn linhas_partes(previa: &PackPreview) -> Vec<[String; 2]> {
    previa
        .parts
        .iter()
        .map(|(caminho, tamanho)| [caminho.clone(), bytes(*tamanho)])
        .collect()
}

pub fn linhas_worktrees(worktrees: &[WorktreeInfo]) -> Vec<[String; 5]> {
    let curto = |head: &str| head.chars().take(10).collect::<String>();
    worktrees
        .iter()
        .map(|w| {
            [
                w.agent.clone(),
                w.branch.clone(),
                curto(&w.head),
                w.bundle.clone().unwrap_or_else(|| "—".to_owned()),
                w.patch.clone().unwrap_or_else(|| "—".to_owned()),
            ]
        })
        .collect()
}

pub fn linhas_achados(achados: &[Finding]) -> Vec<[String; 2]> {
    achados
        .iter()
        .map(|f| [f.path.clone(), f.rule.as_str().to_owned()])
        .collect()
}

/// Colunas de número alinhadas à direita.
pub fn coluna_numerica(nome: &str) -> bool {
    matches!(nome, "Agentes" | "Prontos" | "Tamanho")
}

/// O que a tela Prévia mostra para o plano selecionado.
#[derive(Clone, Debug, PartialEq)]
pub enum EstadoPrevia {
    SemPlano,
    Carregando,
    /// Agentes ainda rodando: o pack recusaria, então a prévia nem roda (PC:121).
    NaoPronta(Vec<String>),
    Pronta(Box<PackPreview>),
    Falhou(String),
}

/// Controle de uma tarefa de fundo: no máximo uma em voo e resultado de geração velha é descartado (`invalidar`
/// quando a entrada mudou: outro plano). Mesmo desenho do `vm::Tarefa` do Moirai.
#[derive(Debug, Default)]
pub struct Tarefa {
    geracao: u64,
    em_voo: bool,
}

impl Tarefa {
    /// Geração da nova execução, ou `None` se a anterior ainda não terminou.
    pub fn iniciar(&mut self) -> Option<u64> {
        if self.em_voo {
            return None;
        }
        self.em_voo = true;
        Some(self.geracao)
    }

    /// Descarta o que estiver em voo e libera uma nova execução.
    pub fn invalidar(&mut self) {
        self.geracao = self.geracao.wrapping_add(1);
        self.em_voo = false;
    }

    /// `true` se o resultado da geração `geracao` ainda vale; só então libera a próxima execução.
    pub fn concluir(&mut self, geracao: u64) -> bool {
        if geracao != self.geracao {
            return false;
        }
        self.em_voo = false;
        true
    }

    pub fn carregando(&self) -> bool {
        self.em_voo
    }
}

/// Agentes sendo estacionados agora: o segundo clique no mesmo agente não sobe outro subprocesso (PC:211).
#[derive(Debug, Default)]
pub struct Estacionando {
    em_voo: HashSet<String>,
}

impl Estacionando {
    pub fn iniciar(&mut self, agente: &str) -> bool {
        self.em_voo.insert(agente.to_owned())
    }

    pub fn concluir(&mut self, agente: &str) {
        self.em_voo.remove(agente);
    }

    pub fn contem(&self, agente: &str) -> bool {
        self.em_voo.contains(agente)
    }
}

/// Prévia em cache por plano, com validade de 30 s (PC:46, 128-129). O relógio entra por parâmetro.
#[derive(Debug, Default)]
pub struct CachePrevia {
    itens: HashMap<String, (u64, PackPreview)>,
}

impl CachePrevia {
    pub fn obter(&self, plano: &str, agora_ms: u64) -> Option<&PackPreview> {
        let (em, previa) = self.itens.get(plano)?;
        (agora_ms.saturating_sub(*em) < VALIDADE_PREVIA_MS).then_some(previa)
    }

    pub fn guardar(&mut self, plano: &str, agora_ms: u64, previa: PackPreview) {
        self.itens.insert(plano.to_owned(), (agora_ms, previa));
    }

    pub fn esquecer(&mut self, plano: &str) {
        self.itens.remove(plano);
    }
}

/// Corta a saída do subprocesso no teto, sem partir um caractere.
pub fn cortar(texto: &str, max: usize) -> String {
    if texto.len() <= max {
        return texto.to_owned();
    }
    let mut fim = max;
    while !texto.is_char_boundary(fim) {
        fim -= 1;
    }
    format!("{}…", &texto[..fim])
}

#[cfg(test)]
mod tests;
