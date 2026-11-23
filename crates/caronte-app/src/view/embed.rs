//! O Caronte dentro do painel do Hades: telas do trilho, comandos da paleta e a view sem trilho nem janela.

use std::path::PathBuf;

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div};
use hades_core::Env;
use hades_ui::icons::IconName;
use hades_ui::{Comando, Embutida, FerramentaEmbutida, FerramentaInfo, TelaInfo};

use super::CaronteView;
use crate::vm::{self, Tela};

const fn tela_info(t: Tela) -> TelaInfo {
    TelaInfo {
        id: t.id(),
        rotulo: t.rotulo(),
        icone: t.icone(),
    }
}

/// As telas do trilho próprio, na mesma ordem.
const TELAS: &[TelaInfo] = &[
    tela_info(Tela::Planos),
    tela_info(Tela::Previa),
    tela_info(Tela::Pacotes),
    tela_info(Tela::Retomar),
    tela_info(Tela::Config),
];

/// As telas quando as Configurações vêm do painel.
const TELAS_INTEGRADO: &[TelaInfo] = &[
    tela_info(Tela::Planos),
    tela_info(Tela::Previa),
    tela_info(Tela::Pacotes),
    tela_info(Tela::Retomar),
];

const ATUALIZAR: &str = "planos.atualizar";
const ESTACIONAR: &str = "planos.estacionar";
const RECALCULAR: &str = "previa.recalcular";

/// Os botões das telas Planos e Prévia do pack.
const COMANDOS: &[Comando] = &[
    Comando {
        id: ATUALIZAR,
        grupo: "Planos",
        rotulo: "Atualizar os planos",
        descricao: Some("Relê a pasta de planos e a prontidão dos agentes"),
        icone: IconName::RotateCcw,
        atalho: None,
    },
    Comando {
        id: ESTACIONAR,
        grupo: "Planos",
        rotulo: "Estacionar o agente",
        descricao: Some("Estaciona o agente selecionado no plano aberto"),
        icone: IconName::CirclePause,
        atalho: None,
    },
    Comando {
        id: RECALCULAR,
        grupo: "Prévia do pack",
        rotulo: "Recalcular a prévia",
        descricao: Some("Descarta a prévia guardada do plano aberto e calcula de novo"),
        icone: IconName::RotateCcw,
        atalho: None,
    },
];

/// Telas que a view mostra com as opções `opts`.
pub fn telas(opts: Embutida) -> &'static [TelaInfo] {
    if opts.integrado {
        TELAS_INTEGRADO
    } else {
        TELAS
    }
}

impl CaronteView {
    /// Tela de id `id` entre as visíveis; id desconhecido, ou Configurações com o painel integrado, dá a primeira.
    fn tela_visivel(&self, id: &str) -> Tela {
        telas(Embutida {
            integrado: self.integrado,
        })
        .iter()
        .find(|t| t.id == id)
        .and_then(|t| Tela::de_flag(t.id))
        .unwrap_or(Tela::Planos)
    }

    /// A mesma condição que habilita o botão Estacionar.
    fn pode_estacionar_agora(&self) -> bool {
        let agente_sel = self.plano_atual().and_then(|p| {
            let nome = self.agente.as_ref()?;
            p.agentes.iter().find(|a| &a.name == nome)
        });
        let estacionando = agente_sel.is_some_and(|a| self.estacionando.contem(&a.name));
        agente_sel.is_some() && vm::pode_estacionar(agente_sel, self.hades, estacionando).is_ok()
    }
}

impl FerramentaEmbutida for CaronteView {
    /// Ambiente do processo e pasta do projeto.
    type Ambiente = (Env, PathBuf);

    fn info() -> FerramentaInfo {
        FerramentaInfo {
            id: "caronte",
            titulo: crate::NOME,
            icone: IconName::Package,
            telas: TELAS,
            comandos: COMANDOS,
        }
    }

    fn embutida(
        (env, cwd): Self::Ambiente,
        foco: Option<&str>,
        opts: Embutida,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::iniciar(cx);
        let mut v = Self::new(env, cwd, cx.focus_handle(), cx);
        v.integrado = opts.integrado;
        if let Some(id) = foco {
            let t = v.tela_visivel(id);
            v.mudar_tela(t, cx);
        }
        v
    }

    fn render_tela(
        &mut self,
        tela: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.tela_visivel(tela);
        if t != self.tela {
            self.mudar_tela(t, cx);
        }
        let corpo = self.corpo(t, window, cx);
        div()
            .size_full()
            .flex()
            .min_h_0()
            .child(corpo)
            .into_any_element()
    }

    fn executar(&mut self, comando: &str, _window: &mut Window, cx: &mut Context<Self>) {
        match comando {
            ATUALIZAR => {
                self.recarregar_planos(cx);
                cx.notify();
            }
            ESTACIONAR if self.pode_estacionar_agora() => self.estacionar(cx),
            RECALCULAR => {
                if let Some(plano) = self.plano.clone() {
                    self.cache.esquecer(&plano);
                    self.carregar_previa(cx);
                    cx.notify();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_sem_ids_repetidos() {
        assert!(CaronteView::info().ids_repetidos().is_empty());
    }

    #[test]
    fn toda_tela_do_trilho_tem_tela_info() {
        let info = CaronteView::info();
        for t in Tela::TODAS {
            let tela = info
                .tela(t.id())
                .unwrap_or_else(|| panic!("sem TelaInfo: {}", t.rotulo()));
            assert_eq!((tela.rotulo, tela.icone), (t.rotulo(), t.icone()));
        }
        assert_eq!(info.telas.len(), Tela::TODAS.len());
    }

    #[test]
    fn ids_das_telas_valem_na_flag_tela() {
        for t in CaronteView::info().telas {
            assert_eq!(Tela::de_flag(t.id).map(Tela::id), Some(t.id));
        }
    }

    #[test]
    fn integrado_tira_configuracoes() {
        let ids = |opts| telas(opts).iter().map(|t| t.id).collect::<Vec<_>>();
        assert!(ids(Embutida::default()).contains(&Tela::Config.id()));
        let integrado = ids(Embutida { integrado: true });
        assert!(!integrado.contains(&Tela::Config.id()));
        assert_eq!(integrado.len(), Tela::TODAS.len() - 1);
    }
}
