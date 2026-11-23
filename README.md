# Caronte

Caronte: ferramenta do Hades, reescrita em Rust.

## Crates

- `crates/caronte-core`: regra de negócio, sem interface.
- `crates/caronte-cli`: binário `caronte` (clap).
- `crates/caronte-app`: view gpui exportada e binário `caronte-app`.

## Estado

CLI (`pack`, `unpack`, `brief`) e app com as telas Planos, Prévia do pack, Pacotes, Retomar e Configurações; a view também roda embutida no painel do Hades (`FerramentaEmbutida`, Hades-UI v0.3.0, hades-core v0.2.7).

## Relação com o Hades-Skill

Faz parte da reescrita em Rust das ferramentas do [Hades-Skill](https://github.com/The-Hades-Organization/Hades-Skill). O Hades-Skill continua sendo a fonte até o corte, quando passa a chamar o binário.

## Licença

MIT. Veja `LICENSE`.
