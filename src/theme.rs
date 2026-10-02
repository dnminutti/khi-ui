//! Temas de color y tipografía.
//!
//! Un tema es un archivo TOML de pocas líneas (ver `themes/claro.toml`). Los tres de `themes/` van
//! dentro del binario; los del usuario se leen de `~/.config/khi-ui/themes/*.toml` al arrancar.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, vec2};
use std::sync::Arc;

#[derive(Clone)]
pub struct Theme {
    pub name: String,
    /// Nombre en inglés, opcional (`name_en`). `name` sigue siendo la clave que se guarda.
    pub name_en: Option<String>,
    pub dark: bool,
    pub base: Color32,
    pub mantle: Color32,
    pub crust: Color32,
    pub surface: Color32,
    pub muted: Color32,
    pub text: Color32,
    pub accent: Color32,
}

/// Temas incluidos. El primero es el predeterminado cuando no hay uno guardado (ver `DESIGN.md`).
const BUILTIN: [&str; 3] = [
    include_str!("../themes/oscuro.toml"),
    include_str!("../themes/claro.toml"),
    include_str!("../themes/catppuccin-mocha.toml"),
];

pub const SEMIBOLD: &str = "semibold";

impl Theme {
    /// Lee el subconjunto de TOML que usan los temas: líneas `clave = valor` y comentarios con `#`.
    pub fn parse(src: &str) -> Result<Theme, String> {
        let mut values = std::collections::HashMap::new();
        for line in src.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim();
            if key.starts_with('#') {
                continue;
            }
            let value = value.trim();
            let value = match value.strip_prefix('"') {
                Some(rest) => rest.split('"').next().unwrap_or_default(),
                None => value.split_whitespace().next().unwrap_or_default(),
            };
            values.insert(key, value);
        }
        let color = |key: &str| {
            let v = values.get(key).ok_or(format!("falta `{key}`"))?;
            let rgb = u32::from_str_radix(v.trim_start_matches('#'), 16).map_err(|_| format!("color inválido en `{key}`: {v}"))?;
            Ok::<_, String>(Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        };
        Ok(Theme {
            name: values.get("name").ok_or("falta `name`")?.to_string(),
            name_en: values.get("name_en").map(|n| n.to_string()),
            dark: values.get("dark") == Some(&"true"),
            base: color("base")?,
            mantle: color("mantle")?,
            crust: color("crust")?,
            surface: color("surface")?,
            muted: color("muted")?,
            text: color("text")?,
            accent: color("accent")?,
        })
    }

    pub fn label(&self) -> &str {
        crate::i18n::tr(&self.name, self.name_en.as_deref().unwrap_or(&self.name))
    }

    /// Mezcla el acento con el fondo: sirve para marcar la selección sin tapar el texto.
    pub fn tint(&self) -> Color32 {
        lerp(self.base, self.accent, 0.18)
    }

    pub fn hover(&self) -> Color32 {
        lerp(self.base, self.surface, 0.7)
    }

    /// Color de los errores. El rojo por defecto de egui (`#ff0000`) no llega a 4.5:1 sobre fondos
    /// claros, así que cada modo usa un rojo que sí pasa.
    pub fn error(&self) -> Color32 {
        if self.dark { Color32::from_rgb(0xf3, 0x8b, 0xa8) } else { Color32::from_rgb(0xb9, 0x1c, 0x1c) }
    }

    /// Borde de botones, desplegables y campos: `surface` sola casi no se distingue del fondo, y un
    /// control tiene que separarse de él por al menos 3:1 (WCAG 1.4.11).
    pub fn border(&self) -> Color32 {
        lerp(self.surface, self.muted, 0.72)
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = self.base;
        v.window_fill = self.mantle;
        v.extreme_bg_color = self.crust;
        v.faint_bg_color = self.surface;
        v.weak_text_color = Some(self.muted);
        v.hyperlink_color = self.accent;
        v.error_fg_color = self.error();
        v.selection.bg_fill = self.tint();
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.window_stroke = Stroke::new(1.0, self.surface);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);

        let w = &mut v.widgets;
        let fills = [self.base, self.surface, lerp(self.surface, self.text, 0.08), lerp(self.surface, self.text, 0.16), self.surface];
        for (visuals, fill) in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open].into_iter().zip(fills) {
            visuals.corner_radius = CornerRadius::same(6);
            visuals.bg_fill = fill;
            visuals.weak_bg_fill = fill;
            visuals.bg_stroke = Stroke::new(1.0, self.border());
            visuals.fg_stroke.color = self.text;
            visuals.expansion = 0.0;
        }
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.surface);
        // egui dibuja un control con foco de teclado con el estilo `active`: el borde de acento es el
        // indicador de foco.
        w.active.bg_stroke = Stroke::new(2.0, self.accent);

        ctx.all_styles_mut(|s| {
            s.visuals = v.clone();
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(12.0, 6.0);
            s.spacing.scroll.bar_width = 6.0;
            s.spacing.scroll.floating = true;
            s.text_styles = [
                (TextStyle::Small, FontId::proportional(11.5)),
                (TextStyle::Body, FontId::proportional(14.0)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
                (TextStyle::Heading, FontId::new(26.0, FontFamily::Name(SEMIBOLD.into()))),
            ]
            .into();
        });
    }
}

/// Fuentes del sistema para japonés, chino y coreano, que Inter no trae: sin ellas esos caracteres se
/// ven como cuadrados. Por idioma se carga la primera que exista (macOS, Windows y Linux). En Linux,
/// Noto Sans CJK cubre los tres idiomas.
const CJK: [&[&str]; 3] = [
    &[
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    ],
    &["/System/Library/Fonts/Hiragino Sans GB.ttc", r"C:\Windows\Fonts\msyh.ttc"],
    &["/System/Library/Fonts/Supplemental/AppleGothic.ttf", r"C:\Windows\Fonts\malgun.ttf"],
];

/// Inter reemplaza a la fuente por defecto; los íconos siguen saliendo de las fuentes de emoji de egui
/// y los caracteres CJK, de las fuentes del sistema en `CJK`.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (i, bytes) in CJK.iter().filter_map(|paths| paths.iter().find_map(|p| std::fs::read(p).ok())).enumerate() {
        let name = format!("cjk-{i}");
        // Las fuentes CJK dibujan los glifos más arriba que Inter; se bajan para alinearlos.
        let tweak = egui::FontTweak { y_offset_factor: 0.1, ..Default::default() };
        fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes).tweak(tweak)));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().push(name);
    }
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for (name, bytes) in [
        ("inter", &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..]),
        (SEMIBOLD, &include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "inter".into());
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), [SEMIBOLD.to_string()].into_iter().chain(fallback).collect());
    ctx.set_fonts(fonts);
}

/// Temas incluidos más los del usuario. Un archivo inválido se informa por stderr y se omite.
pub fn load_all() -> Vec<Theme> {
    let user = crate::settings::dir()
        .and_then(|d| std::fs::read_dir(d.join("themes")).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| {
            let parsed = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| Theme::parse(&s));
            parsed.inspect_err(|e| eprintln!("tema {}: {e}", p.display())).ok()
        });
    BUILTIN.iter().map(|s| Theme::parse(s).expect("tema incluido")).chain(user).collect()
}

pub fn saved() -> Option<String> {
    crate::settings::get("theme")
}

pub fn save(name: &str) {
    crate::settings::set("theme", name);
}

/// Contraste WCAG 2.x entre dos colores, de 1 a 21.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtin_themes_parse() {
        let names: Vec<_> = super::BUILTIN.iter().map(|s| super::Theme::parse(s).unwrap().name).collect();
        assert_eq!(names, ["Oscuro", "Claro", "Catppuccin Mocha"]);
        let err = super::Theme::parse("name = \"X\"\nbase = \"#1e1e2e\" # c\n").err();
        assert_eq!(err.as_deref(), Some("falta `mantle`"));
    }


    /// Los temas incluidos cumplen WCAG AA: 4.5:1 para el texto secundario y el acento sobre cada
    /// fondo donde aparecen, y 3:1 para el borde de los controles.
    #[test]
    fn builtin_themes_contrast() {
        for theme in super::BUILTIN.iter().map(|s| super::Theme::parse(s).unwrap()) {
            let t = &theme;
            for (what, fg, bg, min) in [
                ("muted/base", t.muted, t.base, 4.5),
                ("muted/mantle", t.muted, t.mantle, 4.5),
                ("muted/crust", t.muted, t.crust, 4.5),
                ("muted/surface", t.muted, t.surface, 4.5),
                ("muted/hover", t.muted, t.hover(), 4.5),
                ("accent/base", t.accent, t.base, 4.5),
                ("accent/tint", t.accent, t.tint(), 4.5),
                ("base/accent", t.base, t.accent, 4.5),
                ("error/base", t.error(), t.base, 4.5),
                ("error/crust", t.error(), t.crust, 4.5),
                ("border/base", t.border(), t.base, 3.0),
                ("border/mantle", t.border(), t.mantle, 3.0),
            ] {
                let r = super::contrast(fg, bg);
                assert!(r >= min, "{}: {what} {r:.2} < {min}", t.name);
            }
        }
    }
}
