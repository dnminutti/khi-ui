//! Idioma de la interfaz. Cada texto se escribe en los dos idiomas donde se usa: `tr("Hola", "Hello")`.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

// ponytail: global de proceso porque la app tiene una sola ventana; pasarlo por contexto si hubiera varias.
static ENGLISH: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Es, Lang::En];

    fn key(self) -> &'static str {
        ["es", "en"][self as usize]
    }

    pub fn label(self) -> &'static str {
        ["Español", "English"][self as usize]
    }

    pub fn current() -> Lang {
        if ENGLISH.load(Relaxed) { Lang::En } else { Lang::Es }
    }

    pub fn set(self) {
        ENGLISH.store(self == Lang::En, Relaxed);
        crate::settings::set("language", self.key());
    }
}

/// Lee el idioma guardado; sin preferencia usa el del sistema (`LANG`), y si no es español, inglés.
pub fn init() {
    let key = crate::settings::get("language").or_else(|| std::env::var("LANG").ok()).unwrap_or_default();
    ENGLISH.store(!key.starts_with("es"), Relaxed);
}

pub fn tr<'a>(es: &'a str, en: &'a str) -> &'a str {
    if ENGLISH.load(Relaxed) { en } else { es }
}
