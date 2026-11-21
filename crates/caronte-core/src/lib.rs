//! Regra de negócio do Caronte, sem interface.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compila() {
        assert_eq!(env!("CARGO_PKG_NAME"), "caronte-core");
    }
}
