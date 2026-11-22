//! Guarda de segredo do pacote: nome e conteúdo. Porta de LC:217-252 e RG:10-13, 66-68. O achado nomeia o caminho
//! lógico e a regra, nunca o valor.

use std::sync::OnceLock;

use regex::bytes::Regex;

/// RG:10-12: marcadores montados em partes, para o fonte não conter o literal que a varredura procura.
const SECRET_PARTS: [(&str, &str); 3] = [
    ("Hds!", "[A-Za-z0-9]"),
    ("eyJh", "bGci"),
    ("BEGIN (RSA|OPENSSH) ", "PRIVATE"),
];
/// RG:13. `\b` é ASCII como o do JS (`(?-u:\b)`), e `[^ ]` casa qualquer byte menos o espaço, inclusive `\n`.
const SECRET_TAIL: &str = r"devtunnels\.ms/?(?-u:[^ ])*[a-z0-9]{8}-|(?-u:\b)(JWT_SECRET|INTERNAL_SERVICE_TOKEN|MINIO_ROOT_PASSWORD)=[A-Za-z0-9_-]{8,}";

/// RG:66-68 (`secretPattern`): ERE só dos marcadores de segredo.
pub fn secret_pattern() -> String {
    let mut alternatives: Vec<String> = SECRET_PARTS
        .iter()
        .map(|(a, b)| format!("{a}{b}"))
        .collect();
    alternatives.push(SECRET_TAIL.to_owned());
    alternatives.join("|")
}

/// LC:33-34: nomes que nunca entram, comparados no nome-base.
const SECRET_NAMES: [&str; 4] = [
    r"(?i-u)^credenciais\.md$",
    r"(?-u)^\.env(\..+)?$",
    r"(?i-u)\.(pem|key|p12|pfx)$",
    r"(?i-u)^id_(rsa|dsa|ecdsa|ed25519)(\.|$)",
];
const ENV_EXAMPLE: &str = r"(?-u)^\.env\.(example|sample)$";

fn name_regexes() -> &'static (Vec<Regex>, Regex) {
    static COMPILED: OnceLock<(Vec<Regex>, Regex)> = OnceLock::new();
    COMPILED.get_or_init(|| {
        (
            SECRET_NAMES
                .iter()
                .map(|p| Regex::new(p).expect("padrão de nome válido"))
                .collect(),
            Regex::new(ENV_EXAMPLE).expect("padrão de exemplo válido"),
        )
    })
}

fn content_regex() -> &'static Regex {
    static COMPILED: OnceLock<Regex> = OnceLock::new();
    COMPILED.get_or_init(|| Regex::new(&secret_pattern()).expect("padrão de segredo válido"))
}

/// `basename` do Node para caminho lógico com `/`: último segmento, sem barras finais.
pub fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Regra que barrou o achado (`"nome"` ou `"conteudo"` no TS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Nome,
    Conteudo,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Nome => "nome",
            Rule::Conteudo => "conteudo",
        }
    }
}

/// LC:62-64 (`Finding`): caminho lógico e regra, nunca o valor achado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub rule: Rule,
}

/// LC:217-221 (`secretName`): decisão só pelo nome; `.env.example` e `.env.sample` passam.
pub fn secret_name(path: &str) -> bool {
    let name = basename(path).as_bytes();
    let (secrets, example) = name_regexes();
    !example.is_match(name) && secrets.iter().any(|re| re.is_match(name))
}

/// LC:223-232 (`scanText`): achados de uma parte; `path` é o caminho lógico.
pub fn scan_text(path: &str, text: &[u8]) -> Vec<Finding> {
    let mut found = Vec::new();
    if secret_name(path) {
        found.push(Finding {
            path: path.to_owned(),
            rule: Rule::Nome,
        });
    }
    if content_regex().is_match(text) {
        found.push(Finding {
            path: path.to_owned(),
            rule: Rule::Conteudo,
        });
    }
    found
}

/// LC:234-246 (`scanDiff`): quebra um `git diff` em `diff --git` para o achado nomear o arquivo do repositório.
pub fn scan_diff(prefix: &str, diff: &[u8]) -> Vec<Finding> {
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    static FILE: OnceLock<Regex> = OnceLock::new();
    let split = SPLIT.get_or_init(|| Regex::new(r"(?m)^diff --git ").expect("padrão válido"));
    let file = FILE.get_or_init(|| Regex::new(r"(?-u)^a/([^\r\n]+?) b/").expect("padrão válido"));
    let mut found = Vec::new();
    for block in split.split(diff) {
        if block.is_empty() {
            continue;
        }
        let name = file
            .captures(block)
            .map(|c| String::from_utf8_lossy(&c[1]).into_owned())
            .unwrap_or_else(|| "?".to_owned());
        found.extend(scan_text(&format!("{prefix}/{name}"), block));
    }
    found
}

/// LC:248-252 (`applyAllow`): `--allow` libera um caminho lógico por igualdade.
pub fn apply_allow(findings: Vec<Finding>, allow: &[String]) -> (Vec<Finding>, Vec<String>) {
    let mut allowed: Vec<String> = Vec::new();
    let mut blocked = Vec::new();
    for finding in findings {
        if allow.contains(&finding.path) {
            if !allowed.contains(&finding.path) {
                allowed.push(finding.path);
            }
        } else {
            blocked.push(finding);
        }
    }
    (blocked, allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker() -> String {
        ["Hds", "!", "abcdef1234"].join("")
    }

    #[test]
    fn nome_barra_env_credenciais_e_chaves() {
        for name in [
            ".env",
            ".env.local",
            "docs/credenciais.md",
            "CREDENCIAIS.MD",
            "a/b/server.PEM",
            "x.key",
            "c.p12",
            "c.pfx",
            "id_rsa",
            "id_ed25519.pub",
            "sub/id_ecdsa",
        ] {
            assert!(secret_name(name), "{name}");
        }
    }

    #[test]
    fn nome_deixa_passar_exemplo_e_nomes_comuns() {
        for name in [
            ".env.example",
            "x/.env.sample",
            "env.txt",
            "a.txt",
            "id_rsa_notes",
            "keyboard.md",
            ".envrc2/",
            "credenciais.md.bak",
        ] {
            assert!(!secret_name(name), "{name}");
        }
    }

    #[test]
    fn conteudo_acha_cada_marcador_sem_devolver_o_valor() {
        let hds = marker();
        let jwt = ["eyJh", "bGciOi"].join("");
        let pem = ["-----BEGIN RSA ", "PRIVATE KEY-----"].join("");
        let ssh = ["BEGIN OPENSSH ", "PRIVATE"].join("");
        let tunnel = "https://x.devtunnels.ms/?a=zzzz1234abcd-1";
        let env = "JWT_SECRET=abcdefgh12";
        for text in [hds.as_str(), &jwt, &pem, &ssh, tunnel, env] {
            let found = scan_text("a.txt", text.as_bytes());
            assert_eq!(
                found,
                vec![Finding {
                    path: "a.txt".into(),
                    rule: Rule::Conteudo
                }],
                "{text}"
            );
        }
    }

    #[test]
    fn conteudo_respeita_a_borda_ascii_da_variavel() {
        assert!(scan_text("a", b"MY_JWT_SECRET=abcdefgh12").is_empty());
        assert!(scan_text("a", b"JWT_SECRET=curto").is_empty());
        assert!(!scan_text("a", "é JWT_SECRET=abcdefgh12".as_bytes()).is_empty());
        assert!(scan_text("a", b"Hds!_").is_empty());
    }

    #[test]
    fn conteudo_aceita_bytes_invalidos_em_utf8() {
        let mut data = vec![0xff, 0xfe, b'\n'];
        data.extend_from_slice(marker().as_bytes());
        assert_eq!(scan_text("bin", &data).len(), 1);
    }

    #[test]
    fn nome_e_conteudo_juntos_geram_dois_achados_na_ordem() {
        let found = scan_text(".env", marker().as_bytes());
        let rules: Vec<_> = found.iter().map(|f| f.rule.as_str()).collect();
        assert_eq!(rules, ["nome", "conteudo"]);
    }

    #[test]
    fn diff_nomeia_o_arquivo_do_repositorio() {
        let diff = format!(
            "diff --git a/ok.txt b/ok.txt\n+limpo\ndiff --git a/config.txt b/config.txt\n+senha={}\ndiff --git a/.env b/.env\n+A=1\n",
            marker()
        );
        let found = scan_diff("worktrees/fake-a", diff.as_bytes());
        let listed: Vec<_> = found
            .iter()
            .map(|f| (f.path.as_str(), f.rule.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("worktrees/fake-a/config.txt", "conteudo"),
                ("worktrees/fake-a/.env", "nome"),
            ]
        );
    }

    #[test]
    fn diff_sem_cabecalho_nomeia_interrogacao() {
        let found = scan_diff("w", marker().as_bytes());
        assert_eq!(found[0].path, "w/?");
    }

    #[test]
    fn allow_libera_por_igualdade_e_deduplica() {
        let findings = vec![
            Finding {
                path: "a/.env".into(),
                rule: Rule::Nome,
            },
            Finding {
                path: "a/.env".into(),
                rule: Rule::Conteudo,
            },
            Finding {
                path: "b".into(),
                rule: Rule::Conteudo,
            },
        ];
        let (blocked, allowed) = apply_allow(findings, &["a/.env".to_owned(), "a".to_owned()]);
        assert_eq!(allowed, ["a/.env"]);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].path, "b");
    }

    #[test]
    fn basename_como_o_do_node() {
        assert_eq!(basename("a/b/c.txt"), "c.txt");
        assert_eq!(basename("c.txt"), "c.txt");
        assert_eq!(basename("a/b/"), "b");
    }
}
