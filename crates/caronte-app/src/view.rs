//! `CaronteView`: casca (topo, rail, status) e as telas Planos e Prévia do pack. Pack e unpack ficam no terminal,
//! onde a senha passa pelo tty: o app mostra o comando pronto e nunca pede senha.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Window, div, px,
};
use hades_core::Env;
use hades_core::work::WorkEnv;
use hades_ui::components::{
    Align, Button, CopyButton, DataTable, EmptyState, PageHeader, Rail, RailGroup, RailItem,
    StatusBadge, StatusBar, TermBlock, TopBar,
};
use hades_ui::palette;

use crate::service;
use crate::vm::{
    self, COLUNAS_ACHADOS, COLUNAS_AGENTES, COLUNAS_PARTES, COLUNAS_PLANOS, COLUNAS_WORKTREES,
    CachePrevia, Estacionando, EstadoPrevia, PlanoResumo, Tarefa, Tela,
};

const PERIODO_MS: u64 = 500;
const TIQUES_PLANOS: u32 = 10;

pub struct CaronteView {
    env: Env,
    cwd: PathBuf,
    work: Option<WorkEnv>,
    hades: bool,
    tela: Tela,
    planos: Vec<PlanoResumo>,
    plano: Option<String>,
    agente: Option<String>,
    previa: EstadoPrevia,
    cache: CachePrevia,
    aviso: Option<String>,
    foco: FocusHandle,
    tique: u32,
    t_planos: Tarefa,
    t_previa: Tarefa,
    estacionando: Estacionando,
}

impl CaronteView {
    pub fn new(env: Env, cwd: PathBuf, foco: FocusHandle, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            hades: service::hades_no_path(&env),
            work: None,
            tela: Tela::Planos,
            planos: Vec::new(),
            plano: None,
            agente: None,
            previa: EstadoPrevia::SemPlano,
            cache: CachePrevia::default(),
            aviso: None,
            foco,
            tique: 0,
            t_planos: Tarefa::default(),
            t_previa: Tarefa::default(),
            estacionando: Estacionando::default(),
            env,
            cwd,
        };
        view.recarregar_planos(cx);
        view
    }

    /// Lê a pasta de planos e a prontidão dos agentes no executor de fundo; a renovação periódica pula se a
    /// anterior ainda não terminou.
    fn recarregar_planos(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.t_planos.iniciar() else {
            return;
        };
        let (env, cwd) = (self.env.clone(), self.cwd.clone());
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let work = service::carregar_raiz(&env, &cwd).map_err(|e| e.to_string())?;
                    let mut planos = service::listar_planos(&work, &service::pasta_de_planos(&env));
                    vm::ordenar_planos(&mut planos);
                    Ok::<_, String>((work, planos))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !v.t_planos.concluir(g) {
                    return;
                }
                match r {
                    Ok((work, planos)) => {
                        v.work = Some(work);
                        v.planos = planos;
                        v.aviso = None;
                        v.conferir_selecao();
                    }
                    Err(e) => v.aviso = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Plano que sumiu da pasta deixa de ser o selecionado.
    fn conferir_selecao(&mut self) {
        if let Some(nome) = &self.plano
            && !self.planos.iter().any(|p| &p.nome == nome)
        {
            self.plano = None;
            self.agente = None;
            self.previa = EstadoPrevia::SemPlano;
            self.t_previa.invalidar();
        }
    }

    fn plano_atual(&self) -> Option<&PlanoResumo> {
        let nome = self.plano.as_ref()?;
        self.planos.iter().find(|p| &p.nome == nome)
    }

    fn selecionar_plano(&mut self, nome: String, cx: &mut Context<Self>) {
        if self.plano.as_ref() == Some(&nome) {
            return;
        }
        self.plano = Some(nome);
        self.agente = None;
        self.t_previa.invalidar();
        self.previa = EstadoPrevia::SemPlano;
        if self.tela == Tela::Previa {
            self.carregar_previa(cx);
        }
    }

    /// Prévia do plano selecionado: sem agente rodando e fora do cache, calcula no executor de fundo.
    fn carregar_previa(&mut self, cx: &mut Context<Self>) {
        self.t_previa.invalidar();
        let Some(resumo) = self.plano_atual() else {
            self.previa = EstadoPrevia::SemPlano;
            return;
        };
        let nome = resumo.nome.clone();
        let faltam: Vec<String> = resumo.faltam().iter().map(|a| a.name.clone()).collect();
        if !faltam.is_empty() {
            self.previa = EstadoPrevia::NaoPronta(faltam);
            return;
        }
        if let Some(p) = self.cache.obter(&nome, service::agora_ms()) {
            self.previa = EstadoPrevia::Pronta(Box::new(p.clone()));
            return;
        }
        let (Some(work), Some(g)) = (self.work.clone(), self.t_previa.iniciar()) else {
            self.previa = EstadoPrevia::Carregando;
            return;
        };
        self.previa = EstadoPrevia::Carregando;
        let pasta = service::pasta_de_planos(&self.env);
        cx.spawn(async move |this, cx| {
            let alvo = nome.clone();
            let r = cx
                .background_executor()
                .spawn(async move { service::previa(&work, &pasta, &alvo) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !v.t_previa.concluir(g) {
                    return;
                }
                v.previa = match r {
                    Ok(p) => {
                        v.cache.guardar(&nome, service::agora_ms(), p.clone());
                        EstadoPrevia::Pronta(Box::new(p))
                    }
                    Err(e) => EstadoPrevia::Falhou(e),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// `hades agent park <agente>` por subprocesso no executor de fundo, só no agente da linha selecionada.
    fn estacionar(&mut self, cx: &mut Context<Self>) {
        let (Some(plano), Some(agente), Some(work)) =
            (self.plano.clone(), self.agente.clone(), self.work.clone())
        else {
            return;
        };
        if !self.estacionando.iniciar(&agente) {
            return;
        }
        self.aviso = Some(format!("estacionando {agente}…"));
        let (env, cwd) = (self.env.clone(), self.cwd.clone());
        let pasta = service::pasta_de_planos(&env);
        cx.spawn(async move |this, cx| {
            let (nome, quem) = (plano.clone(), agente.clone());
            let r = cx
                .background_executor()
                .spawn(async move { service::estacionar(&env, &cwd, &work, &pasta, &nome, &quem) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.estacionando.concluir(&agente);
                v.cache.esquecer(&plano);
                v.aviso = Some(match r {
                    Ok(saida) if saida.is_empty() => format!("{agente} estacionado"),
                    Ok(saida) => saida,
                    Err(e) => format!("falhou ao estacionar {agente}: {e}"),
                });
                v.recarregar_planos(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn tiquetaque(&mut self, cx: &mut Context<Self>) {
        self.tique = self.tique.wrapping_add(1);
        if self.tique.is_multiple_of(TIQUES_PLANOS) {
            self.recarregar_planos(cx);
        }
    }

    pub fn iniciar(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(PERIODO_MS))
                    .await;
                if this.update(cx, |v, cx| v.tiquetaque(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn mudar_tela(&mut self, tela: Tela, cx: &mut Context<Self>) {
        self.tela = tela;
        if tela == Tela::Previa {
            self.carregar_previa(cx);
        }
    }

    fn titulo(&self) -> String {
        match &self.plano {
            Some(p) => format!("Caronte · {p}"),
            None => "Caronte".to_owned(),
        }
    }

    fn rodape(&self) -> (String, String) {
        let ocupado = self.t_planos.carregando() || self.t_previa.carregando();
        let esquerda = self.aviso.clone().unwrap_or_else(|| {
            if ocupado {
                "calculando…".to_owned()
            } else {
                format!("{} planos", self.planos.len())
            }
        });
        (esquerda, format!("caronte {}", env!("CARGO_PKG_VERSION")))
    }

    fn rail(&self, cx: &mut Context<Self>) -> Rail {
        let entidade = cx.entity();
        let itens = Tela::TODAS.map(|t| {
            let item = RailItem::new(t.rotulo());
            if t == Tela::Planos {
                item.badge(self.planos.len().to_string())
            } else {
                item
            }
        });
        Rail::new([RailGroup::new("Caronte", itens)])
            .selected(self.tela.rotulo())
            .on_select(move |rotulo, _, cx| {
                if let Some(t) = Tela::de_rotulo(rotulo) {
                    entidade.update(cx, |v, cx| {
                        v.mudar_tela(t, cx);
                        cx.notify();
                    });
                }
            })
    }

    fn tela_planos(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entidade = cx.entity();
        let mut coluna = div().flex().flex_col().gap(px(14.0));
        let agente_sel = self.plano_atual().and_then(|p| {
            let nome = self.agente.as_ref()?;
            p.agentes.iter().find(|a| &a.name == nome)
        });
        let motivo = vm::pode_estacionar(
            agente_sel,
            self.hades,
            agente_sel.is_some_and(|a| self.estacionando.contem(&a.name)),
        );
        let estacionar = match (motivo, agente_sel) {
            (Ok(()), Some(a)) => {
                let e = entidade.clone();
                Button::new("estacionar", format!("Estacionar {}", a.name))
                    .primary()
                    .on_click(move |_, _, cx| e.update(cx, |v, cx| v.estacionar(cx)))
            }
            (Err(m), _) => Button::new("estacionar", "Estacionar").disabled(m),
            (Ok(()), None) => {
                Button::new("estacionar", "Estacionar").disabled("Selecione um agente")
            }
        };
        coluna = coluna.child(PageHeader::new("Planos").action(estacionar));

        if self.planos.is_empty() {
            return coluna
                .child(EmptyState::new("Nenhum plano").hint(
                    "Os planos ficam em ~/.claude/hades/plans; o app só lê o estado e mostra os comandos.",
                ))
                .into_any_element();
        }

        let agora = service::agora_ms();
        let mut tabela = DataTable::new(colunas(&COLUNAS_PLANOS));
        for l in vm::linhas_planos(&self.planos, agora) {
            tabela = tabela.row(l.into_iter().map(SharedString::from));
        }
        if let Some(i) = self
            .plano
            .as_ref()
            .and_then(|n| self.planos.iter().position(|p| &p.nome == n))
        {
            tabela = tabela.selected(i);
        }
        let e1 = entidade.clone();
        tabela = tabela.on_select(move |i, _, cx| {
            e1.update(cx, |v, cx| {
                if let Some(p) = v.planos.get(i) {
                    v.selecionar_plano(p.nome.clone(), cx);
                }
                cx.notify();
            });
        });
        coluna = coluna.child(tabela);

        let Some(resumo) = self.plano_atual() else {
            return coluna.into_any_element();
        };
        coluna = coluna.child(PageHeader::new(resumo.nome.clone()));
        if resumo.agentes.is_empty() {
            return coluna
                .child(EmptyState::new("Nenhum agente ligado").hint(
                    "Nenhum registro de agente cita este plano; o pack levaria só os arquivos do plano.",
                ))
                .into_any_element();
        }
        let mut agentes = DataTable::new(colunas(&COLUNAS_AGENTES));
        for l in vm::linhas_agentes(&resumo.agentes) {
            agentes = agentes.row(l.into_iter().map(SharedString::from));
        }
        if let Some(i) = self
            .agente
            .as_ref()
            .and_then(|n| resumo.agentes.iter().position(|a| &a.name == n))
        {
            agentes = agentes.selected(i);
        }
        let e2 = entidade.clone();
        agentes = agentes.on_select(move |i, _, cx| {
            e2.update(cx, |v, cx| {
                v.agente = v
                    .plano_atual()
                    .and_then(|p| p.agentes.get(i))
                    .map(|a| a.name.clone());
                cx.notify();
            });
        });
        coluna = coluna.child(agentes);

        let mut selos = div().flex().gap(px(8.0));
        for a in &resumo.agentes {
            selos = selos.child(StatusBadge::new(
                format!("{} · {}", a.name, vm::rotulo_prontidao(a.readiness)),
                vm::tom_prontidao(a.readiness),
            ));
        }
        coluna = coluna.child(selos);

        let faltam = resumo.faltam();
        if !faltam.is_empty() {
            let mut term = TermBlock::new().dim("# estacione antes de empacotar");
            for a in faltam {
                term = term.line(vm::comando_park(&a.name));
            }
            coluna = coluna.child(term);
        }
        coluna.into_any_element()
    }

    fn tela_previa(&self) -> gpui::AnyElement {
        let mut coluna = div().flex().flex_col().gap(px(14.0));
        let titulo = self
            .plano
            .clone()
            .map_or_else(|| "Prévia do pack".to_owned(), |p| format!("Prévia · {p}"));
        coluna = coluna.child(PageHeader::new(titulo));
        let Some(nome) = self.plano.clone() else {
            return coluna
                .child(
                    EmptyState::new("Nenhum plano selecionado")
                        .hint("Escolha um plano na tela Planos para ver o que entra no pacote."),
                )
                .into_any_element();
        };
        match &self.previa {
            EstadoPrevia::SemPlano => coluna
                .child(
                    EmptyState::new("Sem prévia").hint("Volte à tela Planos e escolha um plano."),
                )
                .into_any_element(),
            EstadoPrevia::Carregando => coluna
                .child(
                    EmptyState::new("Calculando a prévia…")
                        .hint("Montar a prévia lê as worktrees e roda git; a janela segue livre."),
                )
                .into_any_element(),
            EstadoPrevia::NaoPronta(faltam) => {
                let mut term = TermBlock::new().dim("# estacione antes de empacotar");
                for a in faltam {
                    term = term.line(vm::comando_park(a));
                }
                coluna
                    .child(EmptyState::new("Agentes ainda rodando").hint(
                        "O pack recusa enquanto algum agente do plano não estiver estacionado.",
                    ))
                    .child(term)
                    .into_any_element()
            }
            EstadoPrevia::Falhou(motivo) => coluna
                .child(EmptyState::new("A prévia falhou").hint(motivo.clone()))
                .into_any_element(),
            EstadoPrevia::Pronta(p) => {
                let achados = p.blocked.len();
                coluna = coluna.child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(StatusBadge::new(
                            format!("guarda de segredo: {}", vm::rotulo_guarda(achados)),
                            vm::tom_guarda(achados),
                        ))
                        .child(StatusBadge::new(
                            format!("{} · {} partes", vm::bytes(p.raw_bytes), p.parts.len()),
                            hades_ui::Tone::Neutral,
                        )),
                );
                let mut partes = DataTable::new(colunas(&COLUNAS_PARTES));
                for l in vm::linhas_partes(p) {
                    partes = partes.row(l.into_iter().map(SharedString::from));
                }
                coluna = coluna.child(partes);
                if !p.worktrees.is_empty() {
                    let mut trees = DataTable::new(colunas(&COLUNAS_WORKTREES));
                    for l in vm::linhas_worktrees(&p.worktrees) {
                        trees = trees.row(l.into_iter().map(SharedString::from));
                    }
                    coluna = coluna.child(trees);
                }
                let allow = vm::sugestao_allow(&p.blocked);
                if achados > 0 {
                    let mut tabela = DataTable::new(colunas(&COLUNAS_ACHADOS));
                    for l in vm::linhas_achados(&p.blocked) {
                        tabela = tabela.row(l.into_iter().map(SharedString::from));
                    }
                    coluna = coluna.child(tabela);
                }
                let comando = vm::comando_pack(&nome, &[]);
                let mut term = TermBlock::new()
                    .dim("# a senha vai no terminal, sem eco; o app não guarda nem repassa senha")
                    .line(comando.clone());
                if !allow.is_empty() {
                    term = term
                        .dim("# só se o dono confirmar que o achado é esperado:")
                        .line(vm::comando_pack(&nome, &allow));
                }
                coluna
                    .child(term)
                    .child(CopyButton::new("copiar-pack", comando))
                    .into_any_element()
            }
        }
    }
}

fn colunas(nomes: &[&str]) -> Vec<(SharedString, Align)> {
    nomes
        .iter()
        .map(|n| {
            let alinhamento = if vm::coluna_numerica(n) {
                Align::Numeric
            } else {
                Align::Start
            };
            (SharedString::from((*n).to_owned()), alinhamento)
        })
        .collect()
}

impl Render for CaronteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (esquerda, direita) = self.rodape();
        let rail = self.rail(cx);
        let corpo = match self.tela {
            Tela::Planos => self.tela_planos(cx),
            Tela::Previa => self.tela_previa(),
        };
        div()
            .id("caronte")
            .track_focus(&self.foco)
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, _, cx| {
                if let Some(t) = vm::tela_da_tecla(&ev.keystroke.key) {
                    v.mudar_tela(t, cx);
                    cx.notify();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.fg)
            .child(TopBar::new(self.titulo()).trailing("gpui"))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h(px(0.0))
                    .child(rail)
                    .child(div().flex_1().p(px(16.0)).child(corpo)),
            )
            .child(StatusBar::new(esquerda, direita))
    }
}

pub fn abrir_janela(cx: &mut App) {
    let aberta = cx.open_window(gpui::WindowOptions::default(), |window, cx| {
        window.set_window_title(crate::NOME);
        let cwd = std::env::current_dir().unwrap_or_default();
        let view: Entity<CaronteView> = cx.new(|cx| {
            CaronteView::iniciar(cx);
            CaronteView::new(Env::from_process(), cwd, cx.focus_handle(), cx)
        });
        view
    });
    if aberta.is_err() {
        cx.quit();
    }
}
