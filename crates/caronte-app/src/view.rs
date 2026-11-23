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
    Align, Button, ButtonKind, Card, DataTable, EmptyState, Page, PageHeader, Rail, RailBrand,
    RailGroup, RailItem, RailStat, Skeleton, Spinner, StatusBadge, StatusBar, TermBlock,
};
use hades_ui::icons::IconName;
use hades_ui::palette;
use hades_ui::tokens::space;

use crate::service;
use crate::vm::{
    self, COLUNAS_ACHADOS, COLUNAS_AGENTES, COLUNAS_PACOTES, COLUNAS_PARTES, COLUNAS_PLANOS,
    COLUNAS_WORKTREES, CachePrevia, Estacionando, EstadoPrevia, EstadoRetomada, PacoteResumo,
    PlanoResumo, Tarefa, Tela,
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
    pacotes: Vec<PacoteResumo>,
    pacote: Option<String>,
    t_pacotes: Tarefa,
    retomada: EstadoRetomada,
    t_retomada: Tarefa,
    estacionando: Estacionando,
    recolhida: bool,
    referencia_git: Option<String>,
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
            pacotes: Vec::new(),
            pacote: None,
            t_pacotes: Tarefa::default(),
            retomada: EstadoRetomada::SemPlano,
            t_retomada: Tarefa::default(),
            estacionando: Estacionando::default(),
            recolhida: false,
            referencia_git: service::referencia_git(&cwd),
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
                        if v.tela == Tela::Previa
                            && v.plano.is_some()
                            && matches!(v.previa, EstadoPrevia::SemPlano)
                        {
                            v.carregar_previa(cx);
                        }
                    }
                    Err(e) => v.aviso = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Nome solto de arquivo vira o caminho dentro da pasta de pacotes; caminho com diretório passa como veio.
    fn caminho_de_pacote(&self, arq: &str) -> String {
        if arq.contains('/') {
            return arq.to_owned();
        }
        service::pasta_de_pacotes(&self.env, &self.cwd)
            .join(arq)
            .display()
            .to_string()
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
            self.retomada = EstadoRetomada::SemPlano;
            self.t_retomada.invalidar();
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
        self.t_retomada.invalidar();
        self.retomada = EstadoRetomada::SemPlano;
        match self.tela {
            Tela::Previa => self.carregar_previa(cx),
            Tela::Retomar => self.carregar_retomada(cx),
            _ => {}
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
            match self.tela {
                Tela::Pacotes => self.carregar_pacotes(cx),
                Tela::Retomar => self.carregar_retomada(cx),
                _ => {}
            }
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
        match tela {
            Tela::Previa => self.carregar_previa(cx),
            Tela::Pacotes => self.carregar_pacotes(cx),
            Tela::Retomar => self.carregar_retomada(cx),
            Tela::Planos => {}
        }
    }

    /// Lista a pasta de pacotes no executor de fundo: metadados e cabeçalho em claro, nunca o conteúdo cifrado.
    fn carregar_pacotes(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.t_pacotes.iniciar() else {
            return;
        };
        let pasta = service::pasta_de_pacotes(&self.env, &self.cwd);
        cx.spawn(async move |this, cx| {
            let lista = cx
                .background_executor()
                .spawn(async move { service::listar_pacotes(&pasta) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !v.t_pacotes.concluir(g) {
                    return;
                }
                if let Some(c) = &v.pacote
                    && !lista.iter().any(|p| &p.caminho == c)
                {
                    v.pacote = None;
                }
                v.pacotes = lista;
                cx.notify();
            });
        })
        .detach();
    }

    /// Prompt e passos de retomada do plano selecionado; o `start_command` lê a config, então roda em fundo.
    fn carregar_retomada(&mut self, cx: &mut Context<Self>) {
        let (Some(nome), Some(work)) = (self.plano.clone(), self.work.clone()) else {
            self.t_retomada.invalidar();
            self.retomada = EstadoRetomada::SemPlano;
            return;
        };
        let Some(g) = self.t_retomada.iniciar() else {
            return;
        };
        if !matches!(self.retomada, EstadoRetomada::Pronta(_)) {
            self.retomada = EstadoRetomada::Carregando;
        }
        let (env, cwd) = (self.env.clone(), self.cwd.clone());
        cx.spawn(async move |this, cx| {
            let pasta = service::pasta_de_planos(&env);
            let r = cx
                .background_executor()
                .spawn(async move { service::retomada(&env, &cwd, &work, &pasta, &nome) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !v.t_retomada.concluir(g) {
                    return;
                }
                v.retomada = match r {
                    Ok(r) => EstadoRetomada::Pronta(Box::new(r)),
                    Err(e) => EstadoRetomada::Falhou(e),
                };
                cx.notify();
            });
        })
        .detach();
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

    fn estado_rail(&self) -> RailStat {
        let prontos = self
            .planos
            .iter()
            .filter(|p| !p.agentes.is_empty() && p.faltam().is_empty())
            .count();
        let total = self.planos.len();
        let fracao = if total == 0 {
            0.0
        } else {
            prontos as f32 / total as f32
        };
        RailStat::new("Planos prontos", format!("{prontos} de {total}")).fraction(fracao)
    }

    fn rail(&self, cx: &mut Context<Self>) -> Rail {
        let entidade = cx.entity();
        let item_da_tela = |t: Tela| {
            let item = RailItem::new(t.rotulo()).icon(match t {
                Tela::Planos => IconName::ScrollText,
                Tela::Previa => IconName::Eye,
                Tela::Pacotes => IconName::Package,
                Tela::Retomar => IconName::RotateCcw,
            });
            match t {
                Tela::Planos => item.badge(self.planos.len().to_string()),
                Tela::Pacotes if !self.pacotes.is_empty() => {
                    item.badge(self.pacotes.len().to_string())
                }
                _ => item,
            }
        };
        let preparo = [Tela::Planos, Tela::Previa].map(item_da_tela);
        let pacote = [Tela::Pacotes, Tela::Retomar].map(item_da_tela);
        Rail::new([
            RailGroup::new("Preparo", preparo),
            RailGroup::new("Pacote", pacote),
        ])
        .brand(RailBrand::new("Caronte").subtitle("planos e pacotes"))
        .stat(self.estado_rail())
        .theme_toggle()
        .collapsed(self.recolhida)
        .on_collapse({
            let entidade = entidade.clone();
            move |recolhida, _, cx| {
                entidade.update(cx, |v, cx| {
                    v.recolhida = recolhida;
                    cx.notify();
                });
            }
        })
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
                    .icon(IconName::CirclePause)
                    .on_click(move |_, _, cx| e.update(cx, |v, cx| v.estacionar(cx)))
            }
            (Err(m), _) => Button::new("estacionar", "Estacionar")
                .icon(IconName::CirclePause)
                .disabled(m),
            (Ok(()), None) => Button::new("estacionar", "Estacionar")
                .icon(IconName::CirclePause)
                .disabled("Selecione um agente"),
        };
        let atualizar = {
            let e = entidade.clone();
            Button::new("atualizar-planos", "Atualizar")
                .kind(ButtonKind::Ghost)
                .icon(IconName::RotateCcw)
                .on_click(move |_, _, cx| {
                    e.update(cx, |v, cx| {
                        v.recarregar_planos(cx);
                        cx.notify();
                    });
                })
        };
        let mut cabecalho = PageHeader::new("Planos")
            .subtitle("Planos de orquestração e a prontidão dos agentes para o pack")
            .action(atualizar)
            .action(estacionar);
        if let Some(ref_git) = &self.referencia_git {
            cabecalho = cabecalho
                .meta(ref_git.clone())
                .meta_icon(IconName::GitBranch);
        }
        let pagina = Page::new("planos").header(cabecalho);

        if self.planos.is_empty() {
            return pagina
                .child(EmptyState::new("Nenhum plano").fill().hint(
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

        let Some(resumo) = self.plano_atual() else {
            return pagina
                .child(tabela.fill("planos-tabela"))
                .into_any_element();
        };
        let pagina = pagina.child(tabela);
        if resumo.agentes.is_empty() {
            return pagina
                .child(
                    Card::new()
                        .title(resumo.nome.clone())
                        .fill()
                        .child(EmptyState::new("Nenhum agente ligado").fill().hint(
                            "Nenhum registro de agente cita este plano; o pack levaria só os arquivos do plano.",
                        )),
                )
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

        let mut selos = div().flex().flex_wrap().gap(px(space::SM));
        for a in &resumo.agentes {
            selos = selos.child(StatusBadge::new(
                format!("{} · {}", a.name, vm::rotulo_prontidao(a.readiness)),
                vm::tom_prontidao(a.readiness),
            ));
        }

        let mut card = Card::new()
            .title(resumo.nome.clone())
            .meta(format!("{} agentes", resumo.agentes.len()))
            .child(agentes)
            .child(selos);
        let faltam = resumo.faltam();
        if !faltam.is_empty() {
            let mut term = TermBlock::new().dim("# estacione antes de empacotar");
            let mut copia = Vec::new();
            for a in faltam {
                let cmd = vm::comando_park(&a.name);
                copia.push(cmd.clone());
                term = term.line(cmd);
            }
            card = card.child(term.copy("copiar-park", copia.join("\n")));
        }
        pagina.child(card).into_any_element()
    }

    fn tela_previa(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entidade = cx.entity();
        let titulo = self
            .plano
            .clone()
            .map_or_else(|| "Prévia do pack".to_owned(), |p| format!("Prévia · {p}"));
        let pagina =
            Page::new("previa").header(PageHeader::new(titulo).subtitle(
                "O que entra no pacote e o resultado da guarda de segredo, antes do pack",
            ));
        let Some(nome) = self.plano.clone() else {
            return pagina
                .child(
                    EmptyState::new("Nenhum plano selecionado")
                        .fill()
                        .hint("Escolha um plano na tela Planos para ver o que entra no pacote."),
                )
                .into_any_element();
        };
        match &self.previa {
            EstadoPrevia::SemPlano => pagina
                .child(
                    EmptyState::new("Sem prévia")
                        .fill()
                        .hint("Volte à tela Planos e escolha um plano."),
                )
                .into_any_element(),
            EstadoPrevia::Carregando => pagina
                .child(carregando("previa", "Calculando a prévia"))
                .into_any_element(),
            EstadoPrevia::NaoPronta(faltam) => {
                let mut term = TermBlock::new().dim("# estacione antes de empacotar");
                let mut copia = Vec::new();
                for a in faltam {
                    let cmd = vm::comando_park(a);
                    copia.push(cmd.clone());
                    term = term.line(cmd);
                }
                pagina
                    .child(EmptyState::new("Agentes ainda rodando").hint(
                        "O pack recusa enquanto algum agente do plano não estiver estacionado.",
                    ))
                    .child(term.copy("copiar-park", copia.join("\n")))
                    .into_any_element()
            }
            EstadoPrevia::Falhou(motivo) => pagina
                .child(
                    EmptyState::new("A prévia falhou")
                        .fill()
                        .hint(motivo.clone()),
                )
                .into_any_element(),
            EstadoPrevia::Pronta(p) => {
                let achados = p.blocked.len();
                let mut pagina = pagina.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::SM))
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
                let recalcular = {
                    let e = entidade.clone();
                    let plano = nome.clone();
                    Button::new("recalcular-previa", "Recalcular")
                        .kind(ButtonKind::Ghost)
                        .icon(IconName::RotateCcw)
                        .on_click(move |_, _, cx| {
                            e.update(cx, |v, cx| {
                                v.cache.esquecer(&plano);
                                v.carregar_previa(cx);
                                cx.notify();
                            });
                        })
                };
                pagina = pagina.child(
                    Card::new()
                        .title("Partes")
                        .meta(format!("{} partes", p.parts.len()))
                        .badge(vm::rotulo_guarda(achados), vm::tom_guarda(achados))
                        .action(recalcular)
                        .child(partes),
                );
                if !p.worktrees.is_empty() {
                    let mut trees = DataTable::new(colunas(&COLUNAS_WORKTREES));
                    for l in vm::linhas_worktrees(&p.worktrees) {
                        trees = trees.row(l.into_iter().map(SharedString::from));
                    }
                    let ver_planos = {
                        let e = entidade.clone();
                        Button::new("ver-planos", "Ver planos")
                            .kind(ButtonKind::Ghost)
                            .icon(IconName::ScrollText)
                            .on_click(move |_, _, cx| {
                                e.update(cx, |v, cx| {
                                    v.mudar_tela(Tela::Planos, cx);
                                    cx.notify();
                                });
                            })
                    };
                    let com_patch = p.worktrees.iter().filter(|w| w.patch.is_some()).count();
                    pagina = pagina.child(
                        Card::new()
                            .title("Worktrees")
                            .meta(format!("{}", p.worktrees.len()))
                            .badge(format!("{com_patch} com patch"), hades_ui::Tone::Neutral)
                            .action(ver_planos)
                            .child(trees),
                    );
                }
                let allow = vm::sugestao_allow(&p.blocked);
                if achados > 0 {
                    let mut tabela = DataTable::new(colunas(&COLUNAS_ACHADOS));
                    for l in vm::linhas_achados(&p.blocked) {
                        tabela = tabela.row(l.into_iter().map(SharedString::from));
                    }
                    pagina = pagina.child(
                        Card::new()
                            .title("Achados")
                            .meta(format!("{achados}"))
                            .child(tabela),
                    );
                }
                let comando = vm::comando_pack(&nome, &[]);
                let mut copia = comando.clone();
                let mut term = TermBlock::new()
                    .dim("# a senha vai no terminal, sem eco; o app não guarda nem repassa senha")
                    .line(comando);
                if !allow.is_empty() {
                    let com_allow = vm::comando_pack(&nome, &allow);
                    term = term
                        .dim("# só se o dono confirmar que o achado é esperado:")
                        .line(com_allow);
                }
                copia.push('\n');
                pagina
                    .child(
                        Card::new()
                            .title("Comando")
                            .description("Rode no terminal: a senha nunca passa pelo app.")
                            .child(term.copy("copiar-pack", copia.trim_end().to_owned())),
                    )
                    .into_any_element()
            }
        }
    }
}

impl CaronteView {
    fn tela_pacotes(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entidade = cx.entity();
        let pagina = Page::new("pacotes").header(
            PageHeader::new("Pacotes")
                .subtitle("Pacotes .caronte gerados; o unpack roda no terminal"),
        );
        if self.pacotes.is_empty() {
            let pasta = service::pasta_de_pacotes(&self.env, &self.cwd);
            return pagina
                .child(EmptyState::new("Nenhum pacote").fill().hint(format!(
                    "Os pacotes .caronte ficam em {}; gere um com o pack no terminal.",
                    pasta.display()
                )))
                .into_any_element();
        }
        let mut tabela = DataTable::new(colunas(&COLUNAS_PACOTES));
        for l in vm::linhas_pacotes(&self.pacotes, service::agora_ms()) {
            tabela = tabela.row(l.into_iter().map(SharedString::from));
        }
        if let Some(i) = self
            .pacote
            .as_ref()
            .and_then(|c| self.pacotes.iter().position(|p| &p.caminho == c))
        {
            tabela = tabela.selected(i);
        }
        tabela = tabela.on_select(move |i, _, cx| {
            entidade.update(cx, |v, cx| {
                v.pacote = v.pacotes.get(i).map(|p| p.caminho.clone());
                cx.notify();
            });
        });
        let Some(p) = self
            .pacote
            .as_ref()
            .and_then(|c| self.pacotes.iter().find(|p| &p.caminho == c))
        else {
            return pagina
                .child(tabela)
                .child(
                    Card::new().title("Unpack").fill().child(
                        EmptyState::new("Selecione um pacote").fill().hint(
                            "O app mostra o comando de unpack; a senha é pedida só no terminal.",
                        ),
                    ),
                )
                .into_any_element();
        };
        let comando = vm::comando_unpack(&p.caminho);
        pagina
            .child(tabela)
            .child(
                Card::new().title("Unpack").meta(p.caminho.clone()).child(
                    TermBlock::new()
                        .dim(
                            "# a senha vai no terminal, sem eco; o app não abre nem decifra pacote",
                        )
                        .line(comando.clone())
                        .copy("copiar-unpack", comando),
                ),
            )
            .into_any_element()
    }

    fn tela_retomar(&self) -> gpui::AnyElement {
        let titulo = self
            .plano
            .clone()
            .map_or_else(|| "Retomar".to_owned(), |p| format!("Retomar · {p}"));
        let pagina = Page::new("retomar").header(
            PageHeader::new(titulo)
                .subtitle("Prompt do orquestrador e passos para retomar os agentes estacionados"),
        );
        match &self.retomada {
            EstadoRetomada::SemPlano => pagina
                .child(
                    EmptyState::new("Nenhum plano selecionado")
                        .fill()
                        .hint("Escolha um plano na tela Planos para ver o prompt de retomada."),
                )
                .into_any_element(),
            EstadoRetomada::Carregando => pagina
                .child(carregando("retomada", "Montando a retomada"))
                .into_any_element(),
            EstadoRetomada::Falhou(motivo) => pagina
                .child(
                    EmptyState::new("A retomada falhou")
                        .fill()
                        .hint(motivo.clone()),
                )
                .into_any_element(),
            EstadoRetomada::Pronta(r) => {
                let mut pagina = pagina;
                if r.agentes.is_empty() {
                    pagina = pagina.child(EmptyState::new("Nenhum agente estacionado").hint(
                        "Depois do unpack, os agentes estacionados do plano aparecem aqui com os passos para retomar.",
                    ));
                }
                for (i, a) in r.agentes.iter().enumerate() {
                    let copia =
                        [a.workspace.as_str(), a.iniciar.as_str(), a.adotar.as_str()].join("\n");
                    pagina = pagina.child(
                        Card::new().title(a.nome.clone()).child(
                            TermBlock::new()
                                .line(a.workspace.clone())
                                .line(a.iniciar.clone())
                                .line(a.adotar.clone())
                                .copy(format!("copiar-agente-{i}"), copia),
                        ),
                    );
                }
                let mut prompt = TermBlock::new();
                for linha in r.prompt.lines() {
                    prompt = prompt.line(linha.to_owned());
                }
                pagina
                    .child(
                        Card::new()
                            .title("Prompt do orquestrador")
                            .child(prompt.copy("copiar-prompt", r.prompt.clone())),
                    )
                    .into_any_element()
            }
        }
    }
}

fn carregando(id: &str, rotulo: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(space::MD))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(Spinner::new(SharedString::from(format!("{id}-giro"))))
                .child(StatusBadge::new(rotulo, hades_ui::Tone::Busy)),
        )
        .child(Skeleton::new(SharedString::from(format!("{id}-sk1"))))
        .child(Skeleton::new(SharedString::from(format!("{id}-sk2"))))
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
        let (esquerda, versao) = self.rodape();
        let rail = self.rail(cx);
        let corpo = match self.tela {
            Tela::Planos => self.tela_planos(cx),
            Tela::Previa => self.tela_previa(cx),
            Tela::Pacotes => self.tela_pacotes(cx),
            Tela::Retomar => self.tela_retomar(),
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
            .child(div().flex_1().flex().min_h_0().child(rail).child(corpo))
            .child(StatusBar::new(esquerda, "").version(versao))
    }
}

pub fn abrir_janela(cx: &mut App, tela: Tela) {
    let aberta = cx.open_window(gpui::WindowOptions::default(), |window, cx| {
        window.set_window_title(crate::NOME);
        let cwd = std::env::current_dir().unwrap_or_default();
        let view: Entity<CaronteView> = cx.new(|cx| {
            CaronteView::iniciar(cx);
            let mut v = CaronteView::new(Env::from_process(), cwd, cx.focus_handle(), cx);
            if let Some(nome) = std::env::args().skip_while(|a| a != "--plano").nth(1) {
                v.selecionar_plano(nome, cx);
            }
            if let Some(arq) = std::env::args().skip_while(|a| a != "--pacote").nth(1) {
                v.pacote = Some(v.caminho_de_pacote(&arq));
            }
            v.mudar_tela(tela, cx);
            v
        });
        view
    });
    if aberta.is_err() {
        cx.quit();
    }
}
