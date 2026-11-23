use caronte_app::vm::Tela;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let escuro = args.iter().any(|a| a == "--dark");
    let tela = args
        .iter()
        .position(|a| a == "--tela")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| Tela::de_flag(n))
        .unwrap_or(Tela::Planos);
    gpui_platform::application().run(move |cx| {
        let modo = if escuro {
            hades_ui::ThemeMode::Dark
        } else {
            hades_ui::ThemeMode::Light
        };
        hades_ui::init(cx, modo);
        caronte_app::abrir_janela(cx, tela);
    });
}
