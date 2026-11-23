//! View gpui do Caronte, exportada para o app próprio e para o painel do Hades.

pub mod config;
mod service;
mod view;
pub mod vm;

pub use view::{CaronteView, abrir_janela};

pub const NOME: &str = "Caronte";
