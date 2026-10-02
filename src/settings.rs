//! Preferencias que sobreviven entre sesiones: un archivo de texto por clave en
//! `~/.config/khi-ui/` (o `$XDG_CONFIG_HOME/khi-ui/`).

use std::path::PathBuf;

pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("khi-ui"))
}

pub fn get(key: &str) -> Option<String> {
    Some(std::fs::read_to_string(dir()?.join(key)).ok()?.trim().to_string())
}

pub fn set(key: &str, value: impl ToString) {
    if let Some(dir) = dir() {
        let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(key), value.to_string()));
    }
}
