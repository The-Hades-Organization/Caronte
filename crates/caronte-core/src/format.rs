//! Cifra e contêiner do pacote `.caronte`. Porta de `src/lib/caronte.ts` (LC) da skill: o formato é contrato do
//! unpack e só cresce.
//!
//! ```text
//! MAGIC "CARONTE\x01" (8) | u32 BE len | header JSON compacto | AES-256-GCM(gzip(contêiner)) | tag (16)
//! ```

use std::fmt;
use std::io::{Read, Write};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// LC:18.
pub const FORMAT_VERSION: i64 = 1;
/// LC:19.
pub const MAGIC: &[u8; 8] = b"CARONTE\x01";
/// LC:20.
const IV_BYTES: usize = 12;
/// LC:21.
const SALT_BYTES: usize = 16;
/// LC:22.
const TAG_BYTES: usize = 16;
/// LC:23.
const KEY_BYTES: usize = 32;
/// LC:24.
const LEN_BYTES: usize = 4;
/// LC:25.
const SIZE_BYTES: usize = 8;
/// LC:26.
pub const DEFAULT_LOG2N: u8 = 17;
/// LC:27.
const SCRYPT_R: u32 = 8;
/// LC:28.
const SCRYPT_P: u32 = 1;
/// Teto dos parâmetros lidos do cabeçalho (LC:30).
const MAX_LOG2N: i64 = 22;
/// LC:31.
const MAX_HEADER_BYTES: usize = 4096;
/// LC:32.
const MAX_PART_BYTES: u64 = 1 << 30;

/// LC:41.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaronteErrorCode {
    WrongPassword,
    Tampered,
    NotAPackage,
    Unsupported,
    /// `die` de LC:128 (código 2 no TS), que no core vira erro tipado.
    EmptyPassword,
}

impl CaronteErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WrongPassword => "wrong-password",
            Self::Tampered => "tampered",
            Self::NotAPackage => "not-a-package",
            Self::Unsupported => "unsupported",
            Self::EmptyPassword => "empty-password",
        }
    }
}

/// LC:43-50.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaronteError {
    pub code: CaronteErrorCode,
    pub message: String,
}

impl CaronteError {
    pub fn new(code: CaronteErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn tampered(message: impl Into<String>) -> Self {
        Self::new(CaronteErrorCode::Tampered, message)
    }
}

impl fmt::Display for CaronteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CaronteError {}

/// LC:52-56.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// caminho lógico dentro do pacote
    pub path: String,
    pub data: Vec<u8>,
}

impl Part {
    pub fn new(path: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            data: data.into(),
        }
    }
}

/// LC:93 (`createHash("sha256")`), em hex minúsculo.
pub fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// LC:108-111.
fn derive_key(password: &str, log2n: u8, salt: &[u8]) -> Result<[u8; KEY_BYTES], CaronteError> {
    let params = scrypt::Params::new(log2n, SCRYPT_R, SCRYPT_P, KEY_BYTES).map_err(|_| {
        CaronteError::new(
            CaronteErrorCode::Unsupported,
            "parâmetros de cifra do pacote não suportados",
        )
    })?;
    let normalized: String = password.nfkc().collect();
    let mut key = [0u8; KEY_BYTES];
    scrypt::scrypt(normalized.as_bytes(), salt, &params, &mut key).map_err(|_| {
        CaronteError::new(
            CaronteErrorCode::Unsupported,
            "parâmetros de cifra do pacote não suportados",
        )
    })?;
    Ok(key)
}

/// Contêiner: `[u32 tamanho do nome][nome][u64 tamanho][bytes]` por parte, o `manifest.json` sempre a primeira
/// (LC:114-126).
pub fn frame(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        let name = part.path.as_bytes();
        out.extend_from_slice(&(name.len() as u32).to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(part.data.len() as u64).to_be_bytes());
        out.extend_from_slice(&part.data);
    }
    out
}

/// LC:128-152.
pub fn unframe(raw: &[u8]) -> Result<Vec<Part>, CaronteError> {
    let truncated = || CaronteError::tampered("contêiner truncado ou adulterado");
    let mut parts = Vec::new();
    let mut at = 0usize;
    let take = |at: &mut usize, bytes: usize| -> Result<&[u8], CaronteError> {
        let end = at
            .checked_add(bytes)
            .filter(|end| *end <= raw.len())
            .ok_or_else(truncated)?;
        let slice = &raw[*at..end];
        *at = end;
        Ok(slice)
    };
    while at < raw.len() {
        let name_len =
            u32::from_be_bytes(take(&mut at, LEN_BYTES)?.try_into().expect("4 bytes")) as usize;
        let path = String::from_utf8_lossy(take(&mut at, name_len)?).into_owned();
        let size = u64::from_be_bytes(take(&mut at, SIZE_BYTES)?.try_into().expect("8 bytes"));
        if size > MAX_PART_BYTES {
            return Err(CaronteError::tampered(format!(
                "parte {path} acima do teto"
            )));
        }
        let data = take(&mut at, size as usize)?.to_vec();
        parts.push(Part { path, data });
    }
    Ok(parts)
}

/// Arquivo: MAGIC | u32 tamanho do cabeçalho | cabeçalho JSON | texto cifrado (gzip do contêiner) | tag GCM
/// (LC:154-172). Sorteia sal e IV; o KDF usa `2^log2n`.
pub fn encrypt_package(parts: &[Part], password: &str, log2n: u8) -> Result<Vec<u8>, CaronteError> {
    let mut salt = [0u8; SALT_BYTES];
    let mut iv = [0u8; IV_BYTES];
    getrandom::getrandom(&mut salt)
        .map_err(|e| CaronteError::tampered(format!("sem aleatoriedade do sistema: {e}")))?;
    getrandom::getrandom(&mut iv)
        .map_err(|e| CaronteError::tampered(format!("sem aleatoriedade do sistema: {e}")))?;
    seal_with(parts, password, log2n, salt, iv)
}

/// `encrypt_package` com sal e IV dados: o núcleo determinístico, usado também para reproduzir vetores.
pub fn seal_with(
    parts: &[Part],
    password: &str,
    log2n: u8,
    salt: [u8; SALT_BYTES],
    iv: [u8; IV_BYTES],
) -> Result<Vec<u8>, CaronteError> {
    if password.is_empty() {
        return Err(CaronteError::new(
            CaronteErrorCode::EmptyPassword,
            "senha vazia: sem senha, sem pacote",
        ));
    }
    let header = json!({
        "v": FORMAT_VERSION,
        "kdf": "scrypt",
        "log2n": log2n,
        "r": SCRYPT_R,
        "p": SCRYPT_P,
        "salt": STANDARD.encode(salt),
        "cipher": "aes-256-gcm",
        "iv": STANDARD.encode(iv),
    });
    let header_bytes = serde_json::to_vec(&header).expect("cabeçalho serializa");
    let mut aad = Vec::with_capacity(MAGIC.len() + LEN_BYTES + header_bytes.len());
    aad.extend_from_slice(MAGIC);
    aad.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    aad.extend_from_slice(&header_bytes);

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&frame(parts))
        .map_err(|e| CaronteError::tampered(e.to_string()))?;
    let compressed = gz
        .finish()
        .map_err(|e| CaronteError::tampered(e.to_string()))?;

    let key = derive_key(password, log2n, &salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &compressed,
                aad: &aad,
            },
        )
        .map_err(|_| CaronteError::tampered("falha ao cifrar"))?;
    let mut file = aad;
    file.extend_from_slice(&sealed);
    Ok(file)
}

struct Opened<'a> {
    log2n: u8,
    salt: Vec<u8>,
    iv: Vec<u8>,
    aad: &'a [u8],
    sealed: &'a [u8],
}

/// LC:167-190 (`readHeader`).
fn read_header(file: &[u8]) -> Result<Opened<'_>, CaronteError> {
    if file.len() < MAGIC.len() + LEN_BYTES + TAG_BYTES || &file[..MAGIC.len()] != MAGIC {
        return Err(CaronteError::new(
            CaronteErrorCode::NotAPackage,
            "não é um pacote do Caronte",
        ));
    }
    let header_len = u32::from_be_bytes(
        file[MAGIC.len()..MAGIC.len() + LEN_BYTES]
            .try_into()
            .expect("4 bytes"),
    ) as usize;
    let end = MAGIC.len() + LEN_BYTES + header_len;
    if header_len > MAX_HEADER_BYTES || end + TAG_BYTES > file.len() {
        return Err(CaronteError::tampered("cabeçalho do pacote inválido"));
    }
    let header: Value = serde_json::from_slice(&file[MAGIC.len() + LEN_BYTES..end])
        .map_err(|_| CaronteError::tampered("cabeçalho do pacote ilegível"))?;
    let version = header.get("v").and_then(Value::as_f64).unwrap_or(0.0);
    if version > FORMAT_VERSION as f64 {
        return Err(CaronteError::new(
            CaronteErrorCode::Unsupported,
            format!(
                "formato {version} é mais novo que o desta skill ({FORMAT_VERSION}): atualize a Hades"
            ),
        ));
    }
    let text = |key: &str| header.get(key).and_then(Value::as_str);
    let int = |key: &str| {
        header
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.fract() == 0.0)
            .map(|n| n as i64)
    };
    let log2n = int("log2n").filter(|n| (1..=MAX_LOG2N).contains(n));
    let sane = text("kdf") == Some("scrypt")
        && text("cipher") == Some("aes-256-gcm")
        && log2n.is_some()
        && int("r") == Some(i64::from(SCRYPT_R))
        && int("p") == Some(i64::from(SCRYPT_P));
    let (Some(salt), Some(iv), true) = (text("salt"), text("iv"), sane) else {
        return Err(CaronteError::new(
            CaronteErrorCode::Unsupported,
            "parâmetros de cifra do pacote não suportados",
        ));
    };
    let unreadable = || {
        CaronteError::new(
            CaronteErrorCode::Unsupported,
            "parâmetros de cifra do pacote não suportados",
        )
    };
    Ok(Opened {
        log2n: log2n.expect("conferido") as u8,
        salt: STANDARD.decode(salt).map_err(|_| unreadable())?,
        iv: STANDARD.decode(iv).map_err(|_| unreadable())?,
        aad: &file[..end],
        sealed: &file[end..],
    })
}

/// Decifra e desfaz o contêiner. GCM não distingue senha errada de arquivo adulterado: a mensagem diz as duas
/// (LC:192-200).
pub fn decrypt_package(file: &[u8], password: &str) -> Result<Vec<Part>, CaronteError> {
    let opened = read_header(file)?;
    let wrong = || {
        CaronteError::new(
            CaronteErrorCode::WrongPassword,
            "senha errada ou pacote adulterado (a autenticação GCM falhou)",
        )
    };
    if opened.iv.len() != IV_BYTES {
        return Err(wrong());
    }
    let key = derive_key(password, opened.log2n, &opened.salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&opened.iv),
            Payload {
                msg: opened.sealed,
                aad: opened.aad,
            },
        )
        .map_err(|_| wrong())?;
    let mut raw = Vec::new();
    GzDecoder::new(plain.as_slice())
        .read_to_end(&mut raw)
        .map_err(|_| CaronteError::tampered("contêiner comprimido inválido"))?;
    unframe(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "senha de teste 123";
    const LOG2N: u8 = 14;

    fn parts() -> Vec<Part> {
        vec![
            Part::new("a.txt", "conteúdo"),
            Part::new("b/c.bin", vec![0, 1, 2, 255]),
        ]
    }

    fn sealed() -> Vec<u8> {
        encrypt_package(&parts(), PASSWORD, LOG2N).unwrap()
    }

    fn replace_header(file: &[u8], from: &str, to: &str) -> Vec<u8> {
        let text: String = file.iter().map(|b| *b as char).collect();
        text.replacen(from, to, 1)
            .chars()
            .map(|c| c as u8)
            .collect()
    }

    #[test]
    fn dada_a_senha_certa_quando_decifra_entao_volta_o_mesmo_conteiner() {
        let back = decrypt_package(&sealed(), PASSWORD).unwrap();
        assert_eq!(back, parts());
    }

    #[test]
    fn dada_a_senha_errada_quando_decifra_entao_falha_com_erro_claro() {
        let err = decrypt_package(&sealed(), "outra").unwrap_err();
        assert_eq!(err.code, CaronteErrorCode::WrongPassword);
        assert!(err.message.contains("senha errada ou pacote adulterado"));
    }

    #[test]
    fn dado_um_byte_do_texto_cifrado_trocado_quando_decifra_entao_o_gcm_barra() {
        let mut file = sealed();
        let at = file.len() - 20;
        file[at] ^= 1;
        let err = decrypt_package(&file, PASSWORD).unwrap_err();
        assert!(err.message.contains("GCM"));
    }

    #[test]
    fn dado_um_parametro_do_cabecalho_adulterado_quando_decifra_entao_falha() {
        let file = replace_header(&sealed(), "\"log2n\":14", "\"log2n\":15");
        assert!(decrypt_package(&file, PASSWORD).is_err());
    }

    #[test]
    fn dado_um_arquivo_qualquer_e_um_log2n_absurdo_quando_decifra_entao_recusa_sem_derivar() {
        let err = decrypt_package(b"nao sou um pacote, mas tenho tamanho suficiente", PASSWORD)
            .unwrap_err();
        assert_eq!(err.code, CaronteErrorCode::NotAPackage);
        assert!(err.message.contains("não é um pacote"));
        let file = replace_header(&sealed(), "\"log2n\":14", "\"log2n\":99");
        let err = decrypt_package(&file, PASSWORD).unwrap_err();
        assert_eq!(err.code, CaronteErrorCode::Unsupported);
        assert!(err.message.contains("não suportados"));
    }

    #[test]
    fn dada_senha_vazia_quando_cifra_entao_recusa() {
        let err = encrypt_package(&parts(), "", LOG2N).unwrap_err();
        assert!(err.message.contains("sem senha, sem pacote"));
    }

    #[test]
    fn dado_formato_mais_novo_quando_decifra_entao_unsupported() {
        let file = replace_header(&sealed(), "\"v\":1", "\"v\":2");
        let err = decrypt_package(&file, PASSWORD).unwrap_err();
        assert_eq!(err.code, CaronteErrorCode::Unsupported);
        assert!(err.message.contains("mais novo"));
    }

    #[test]
    fn dado_cabecalho_acima_do_teto_ou_truncado_quando_decifra_entao_tampered() {
        let mut file = sealed();
        file[8..12].copy_from_slice(&5000u32.to_be_bytes());
        assert_eq!(
            decrypt_package(&file, PASSWORD).unwrap_err().code,
            CaronteErrorCode::Tampered
        );
        let short = &sealed()[..30];
        assert_eq!(
            decrypt_package(short, PASSWORD).unwrap_err().code,
            CaronteErrorCode::Tampered
        );
    }

    #[test]
    fn dada_senha_com_acento_composto_quando_cifra_entao_a_forma_nfkc_abre() {
        let composed = "sen\u{e3}o";
        let decomposed = "sena\u{303}o";
        let file = encrypt_package(&parts(), composed, LOG2N).unwrap();
        assert_eq!(decrypt_package(&file, decomposed).unwrap(), parts());
    }

    #[test]
    fn conteiner_truncado_ou_parte_acima_do_teto_vira_tampered() {
        let raw = frame(&parts());
        assert_eq!(
            unframe(&raw[..raw.len() - 1]).unwrap_err().code,
            CaronteErrorCode::Tampered
        );
        let mut huge = Vec::new();
        huge.extend_from_slice(&1u32.to_be_bytes());
        huge.push(b'x');
        huge.extend_from_slice(&(MAX_PART_BYTES + 1).to_be_bytes());
        let err = unframe(&huge).unwrap_err();
        assert!(err.message.contains("acima do teto"));
    }

    #[test]
    fn contêiner_tem_o_layout_u32_nome_u64_bytes() {
        let raw = frame(&[Part::new("ab", "xyz")]);
        let mut want = vec![0, 0, 0, 2, b'a', b'b'];
        want.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 3]);
        want.extend_from_slice(b"xyz");
        assert_eq!(raw, want);
    }

    #[test]
    fn sha256_confere_o_vetor_conhecido() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
