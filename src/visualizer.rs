//! Visualizador de frecuencias.
//!
//! `Tap` envuelve la fuente de audio y copia las muestras que van al parlante a un búfer compartido.
//! `Spectrum` convierte las últimas muestras en bandas de frecuencia suavizadas, y cada `Style` las
//! dibuja a su manera. Para agregar un estilo nuevo: sumar una variante a `Style` (y a `Style::ALL`)
//! y su dibujo en `Style::paint`; el menú del visualizador lo muestra solo. Los colores salen de una
//! `Palette`: agregar una es sumar una entrada a `PALETTES` con sus colores en hexadecimal.

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke, pos2, vec2};
use rodio::Source;
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::i18n::tr;

/// Muestras que se analizan por cuadro (unos 93 ms a 44,1 kHz). Una ventana larga da barras estables.
const WINDOW: usize = 4096;
const BANDS: usize = 48;
const MIN_HZ: f32 = 60.0;
const MAX_HZ: f32 = 16_000.0;
// Pendiente que se suma a los agudos: 0.75 equivale a +4.5 dB por octava, lo habitual en analizadores. La música pierde energía
// hacia los agudos; sin compensar, las barras de la derecha casi no se ven.
const TILT: f32 = 0.75;
// Ganancia automática: la barra más alta reciente marca el 100 % y ese tope baja con esta
// constante de tiempo (segundos), así cada canción usa la altura completa sea fuerte o suave.
const AUTO_GAIN_FALL: f32 = 3.0;
// Amplitud mínima que se considera sonido (unos -46 dBFS); evita inflar el silencio.
const MIN_REF: f32 = 0.005;
// Curva de la altura: menor que 1 levanta los valores bajos, mayor que 1 resalta los golpes.
const CURVE: f32 = 0.85;
// Fracción del pico reciente que ya llena la barra: deja que los golpes topen y el resto use la altura.
const HEADROOM: f32 = 0.55;
// Tiempos de subida y bajada de las barras, en segundos. Más alto = movimiento más suave.
const ATTACK: f32 = 0.06;
const RELEASE: f32 = 0.35;
/// Instantáneas del espectro que se guardan para los ecos del estilo psicodélico.
const HISTORY: usize = 12;

#[derive(Default)]
pub struct Shared {
    samples: VecDeque<f32>,
    rate: u32,
    written: u64,
}

pub type Feed = Arc<Mutex<Shared>>;

/// Fuente de audio que pasa las muestras sin cambios y copia el primer canal al `Feed`.
pub struct Tap<S> {
    inner: S,
    feed: Feed,
    batch: Vec<f32>,
    channel: u16,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, feed: Feed) -> Self {
        Self { inner, feed, batch: Vec::with_capacity(512), channel: 0 }
    }

    /// Se bloquea el mutex una vez cada 512 muestras, no por muestra, para no frenar al hilo de audio.
    fn flush(&mut self) {
        if let Ok(mut shared) = self.feed.lock() {
            shared.samples.extend(self.batch.drain(..));
            let excess = shared.samples.len().saturating_sub(WINDOW);
            shared.samples.drain(..excess);
            shared.rate = self.inner.sample_rate().get();
            shared.written += 1;
        }
        self.batch.clear();
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            self.batch.push(sample as f32);
            if self.batch.len() == self.batch.capacity() {
                self.flush();
            }
        }
        self.channel = (self.channel + 1) % self.inner.channels().get();
        Some(sample)
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.channel = 0;
        self.inner.try_seek(pos)
    }
}

/// Niveles por banda entre 0 y 1, suavizados en el tiempo y entre bandas vecinas.
#[derive(Default)]
pub struct Spectrum {
    pub levels: Vec<f32>,
    /// Instantáneas recientes de `levels`, la más nueva al final.
    pub history: VecDeque<Vec<f32>>,
    /// Máximo reciente de cada banda, que cae despacio (indicador de pico).
    pub peaks: Vec<f32>,
    /// Últimas muestras crudas, para dibujar la forma de onda.
    pub wave: Vec<f32>,
    /// Distancia recorrida por el túnel de estrellas; avanza más rápido con los graves.
    pub travel: f32,
    /// Graves suavizados (0 a 1) para estilos que deben latir sin temblar.
    pub pulse: f32,
    /// Fase acumulada del estilo psicodélico: avanza siempre y más rápido con los graves.
    pub flow: f32,
    target: Vec<f32>,
    /// Referencia de la ganancia automática: amplitud que corresponde a una barra llena.
    reference: f32,
    last_written: u64,
    stale: f32,
    since_snapshot: f32,
}

impl Spectrum {
    /// Recalcula las bandas; `dt` son los segundos desde el cuadro anterior.
    pub fn update(&mut self, feed: &Feed, dt: f32) {
        // Se copian las muestras y se suelta el mutex antes de calcular.
        let fresh = {
            let shared = feed.lock().unwrap();
            let fresh = shared.written != self.last_written && shared.samples.len() == WINDOW;
            self.last_written = shared.written;
            fresh.then(|| (shared.samples.iter().copied().collect::<Vec<_>>(), shared.rate))
        };
        match fresh {
            Some((samples, rate)) => {
                let amplitudes = bands(&samples, rate);
                let loudest = amplitudes.iter().copied().fold(0.0, f32::max);
                let fall = (-(self.stale + dt) / AUTO_GAIN_FALL).exp();
                self.reference = loudest.max(self.reference * fall).max(MIN_REF);
                let reference = self.reference;
                self.target = blur(&amplitudes.iter().map(|a| (a / (reference * HEADROOM)).powf(CURVE).min(1.0)).collect::<Vec<_>>());
                self.wave = samples;
                self.stale = 0.0;
            }
            // Un cuadro sin muestras nuevas es normal; solo tras 150 ms (pausa, carga) las barras bajan.
            None => {
                self.stale += dt;
                if self.stale > 0.15 {
                    self.target = vec![0.0; BANDS];
                    self.wave.iter_mut().for_each(|s| *s *= 0.8);
                }
            }
        }
        self.target.resize(BANDS, 0.0);
        self.levels.resize(BANDS, 0.0);
        for (level, &target) in self.levels.iter_mut().zip(&self.target) {
            let tau = if target > *level { ATTACK } else { RELEASE };
            *level += (target - *level) * (1.0 - (-dt / tau).exp());
        }
        self.peaks.resize(BANDS, 0.0);
        for (peak, &level) in self.peaks.iter_mut().zip(&self.levels) {
            *peak = level.max(*peak - dt * 0.3);
        }
        self.travel += dt * (0.15 + 1.6 * self.average(0..8) + 0.4 * self.average(8..28));
        self.pulse += (self.average(0..8) - self.pulse) * (1.0 - (-dt / 0.25).exp());
        self.flow += dt * (0.4 + 0.8 * self.pulse);
        self.since_snapshot += dt;
        if self.since_snapshot >= 0.05 {
            self.since_snapshot = 0.0;
            self.history.push_back(self.levels.clone());
            if self.history.len() > HISTORY {
                self.history.pop_front();
            }
        }
    }

    fn average(&self, range: std::ops::Range<usize>) -> f32 {
        let slice = self.levels.get(range).unwrap_or_default();
        slice.iter().sum::<f32>() / slice.len().max(1) as f32
    }
}

/// Suaviza cada banda con sus vecinas (1-2-1) para que no salten barras aisladas.
fn blur(levels: &[f32]) -> Vec<f32> {
    (0..levels.len())
        .map(|i| {
            let left = levels[i.saturating_sub(1)];
            let right = levels[(i + 1).min(levels.len() - 1)];
            (left + 2.0 * levels[i] + right) / 4.0
        })
        .collect()
}

/// Amplitud lineal por banda (1.0 = senoidal a escala completa), con la pendiente `TILT` aplicada.
/// Usa el algoritmo de Goertzel: mide unas pocas frecuencias por banda en lugar de una FFT
/// completa, que para 48 bandas sale igual de barato y sin dependencias.
fn bands(samples: &[f32], rate: u32) -> Vec<f32> {
    let n = samples.len() as f32;
    let windowed: Vec<f32> = samples
        .iter()
        .enumerate()
        // Ventana de Hann, multiplicada por 2 para compensar la amplitud que pierde.
        .map(|(i, s)| s * (1.0 - (TAU * i as f32 / (n - 1.0)).cos()))
        .collect();
    let bin = rate as f32 / n;
    let ratio = (MAX_HZ / MIN_HZ).powf(1.0 / BANDS as f32);
    (0..BANDS)
        .map(|b| {
            let lo = MIN_HZ * ratio.powi(b as i32);
            let hi = lo * ratio;
            let probes = ((hi - lo) / bin).ceil().clamp(1.0, 4.0) as usize;
            let power = (0..probes)
                .map(|k| goertzel(&windowed, lo + (k as f32 + 0.5) * (hi - lo) / probes as f32, rate))
                .fold(0.0, f32::max);
            let center = lo * ratio.sqrt();
            power.sqrt() * (center / 1000.0).powf(TILT)
        })
        .collect()
}

/// Potencia normalizada (1.0 = senoidal a escala completa) de `freq` en la señal.
fn goertzel(samples: &[f32], freq: f32, rate: u32) -> f32 {
    let coeff = 2.0 * (TAU * freq / rate as f32).cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    power / (samples.len() as f32 / 2.0).powi(2)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Style {
    Bars,
    Mirror,
    Led,
    Mountains,
    Oscilloscope,
    Ring,
    Aurora,
    Starfield,
    Psychedelic,
}

impl Style {
    pub const ALL: [Style; 9] = [
        Style::Bars,
        Style::Mirror,
        Style::Led,
        Style::Mountains,
        Style::Oscilloscope,
        Style::Ring,
        Style::Aurora,
        Style::Starfield,
        Style::Psychedelic,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Style::Bars => "bars",
            Style::Mirror => "mirror",
            Style::Led => "led",
            Style::Mountains => "mountains",
            Style::Oscilloscope => "oscilloscope",
            Style::Ring => "ring",
            Style::Aurora => "aurora",
            Style::Starfield => "starfield",
            Style::Psychedelic => "psychedelic",
        }
    }

    pub fn from_key(key: &str) -> Style {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or(Style::Bars)
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Bars => tr("Barras", "Bars"),
            Style::Mirror => tr("Espejo", "Mirror"),
            Style::Led => tr("LED retro", "Retro LED"),
            Style::Mountains => tr("Montañas", "Mountains"),
            Style::Oscilloscope => tr("Osciloscopio", "Oscilloscope"),
            Style::Ring => tr("Anillo", "Ring"),
            Style::Aurora => tr("Aurora", "Aurora"),
            Style::Starfield => tr("Túnel de estrellas", "Starfield"),
            Style::Psychedelic => tr("Psicodélico", "Psychedelic"),
        }
    }

    /// El estilo siguiente (`step = 1`) o anterior (`step = -1`), dando la vuelta al final.
    pub fn cycle(self, step: isize) -> Style {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as isize;
        Self::ALL[(i + step).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    /// `time` en segundos anima los estilos que se mueven solos; `colors` es la paleta elegida.
    pub fn paint(self, painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, time: f32, colors: &Colors) {
        let levels = &spectrum.levels;
        if levels.is_empty() {
            return;
        }
        match self {
            Style::Bars => bars(painter, rect, levels, colors),
            Style::Mirror => mirror(painter, rect, levels, time, colors),
            Style::Led => led(painter, rect, spectrum, colors),
            Style::Mountains => mountains(painter, rect, spectrum, colors),
            Style::Oscilloscope => oscilloscope(painter, rect, &spectrum.wave, colors.at(time * 0.03)),
            Style::Ring => ring(painter, rect, spectrum, time, colors),
            Style::Aurora => aurora(painter, rect, spectrum, time, colors),
            Style::Starfield => starfield(painter, rect, spectrum, colors),
            Style::Psychedelic => psychedelic(painter, rect, spectrum, time, colors),
        }
    }
}

/// Posición x y ancho de `n` columnas con `gap` entre ellas.
fn columns(rect: Rect, n: usize, gap: f32) -> impl Iterator<Item = (f32, f32)> {
    let width = (rect.width() - gap * (n as f32 - 1.0)) / n as f32;
    (0..n).map(move |i| (rect.left() + i as f32 * (width + gap), width))
}

/// Paleta de colores del visualizador, definida en hexadecimal. Sin colores usa los del tema de la app.
pub struct Palette {
    name: &'static str,
    en: &'static str,
    pub key: &'static str,
    hex: &'static [u32],
}

pub const PALETTES: [Palette; 8] = [
    Palette { name: "Tema de la app", en: "App theme", key: "theme", hex: &[] },
    Palette { name: "Arcoíris", en: "Rainbow", key: "rainbow", hex: &[0xff5f5f, 0xffa24c, 0xffe066, 0x6bdc8c, 0x4cc9f0, 0x6c7cff, 0xd070ff] },
    Palette {
        name: "Catppuccin", en: "Catppuccin",
        key: "catppuccin",
        hex: &[0xcba6f7, 0xf5c2e7, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x74c7ec, 0xb4befe],
    },
    Palette { name: "Neón", en: "Neon", key: "neon", hex: &[0xff2bd6, 0x00f0ff, 0x7b2bff, 0x39ff14] },
    Palette { name: "Fuego", en: "Fire", key: "fire", hex: &[0xd62828, 0xf77f00, 0xfcbf49, 0xfff3b0] },
    Palette { name: "Océano", en: "Ocean", key: "ocean", hex: &[0x0077b6, 0x00b4d8, 0x90e0ef, 0x48cae4] },
    Palette { name: "Atardecer", en: "Sunset", key: "sunset", hex: &[0xff5f6d, 0xffc371, 0xc471ed, 0xf64f59] },
    Palette { name: "Monocromo", en: "Monochrome", key: "mono", hex: &[0xffffff, 0x9aa0a6] },
];

impl Palette {
    pub fn label(&self) -> &'static str {
        tr(self.name, self.en)
    }

    /// Índice de la paleta guardada; sin preferencia (o una desconocida) se usa Arcoíris.
    pub fn from_key(key: &str) -> usize {
        let find = |key: &str| PALETTES.iter().position(|p| p.key == key);
        find(key).or_else(|| find("rainbow")).unwrap_or(0)
    }

    pub fn colors(&self, accent: Color32) -> Colors {
        match self.hex {
            [] => Colors(vec![accent, accent.lerp_to_gamma(Color32::WHITE, 0.5)]),
            hex => Colors(hex.iter().map(|&h| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)).collect()),
        }
    }
}

/// Colores resueltos de una paleta. `at` recorre la paleta como un ciclo: 0 y 1 dan el mismo color.
pub struct Colors(Vec<Color32>);

impl Colors {
    pub fn at(&self, t: f32) -> Color32 {
        let n = self.0.len();
        let x = t.rem_euclid(1.0) * n as f32;
        let i = x as usize % n;
        self.0[i].lerp_to_gamma(self.0[(i + 1) % n], x.fract())
    }

    /// Como `at`, pero recorre la paleta sin volver al primer color (para degradados de un extremo al otro).
    fn span(&self, t: f32) -> Color32 {
        self.at(t.clamp(0.0, 1.0) * (self.0.len() - 1).max(1) as f32 / self.0.len() as f32)
    }
}

/// Color con alfa cero en formato premultiplicado: se suma a lo de abajo en vez de taparlo (brillo).
fn additive(c: Color32, k: f32) -> Color32 {
    let m = |x: u8| (x as f32 * k).min(255.0) as u8;
    Color32::from_rgba_premultiplied(m(c.r()), m(c.g()), m(c.b()), 0)
}

/// Número pseudoaleatorio estable en 0..1 para el elemento `k`.
fn hash(k: u32) -> f32 {
    let mut x = k.wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x as f32 / u32::MAX as f32
}

fn bars(painter: &egui::Painter, rect: Rect, levels: &[f32], colors: &Colors) {
    let rect = rect.shrink2(vec2(rect.width() * 0.06, rect.height() * 0.18));
    for (i, ((x, width), level)) in columns(rect, levels.len(), 4.0).zip(levels).enumerate() {
        let color = colors.span(i as f32 / levels.len() as f32);
        let height = (rect.height() * level).max(3.0);
        let bar = Rect::from_min_size(pos2(x, rect.bottom() - height), vec2(width, height));
        painter.rect_filled(bar, 3, color.gamma_multiply(0.35 + 0.65 * level));
    }
}

fn mirror(painter: &egui::Painter, rect: Rect, levels: &[f32], time: f32, colors: &Colors) {
    let rect = rect.shrink2(vec2(rect.width() * 0.05, rect.height() * 0.12));
    let mid = rect.center().y;
    for (i, ((x, width), level)) in columns(rect, levels.len(), 3.0).zip(levels).enumerate() {
        let half = (rect.height() * 0.5 * level).max(1.5);
        let color = colors.at(i as f32 / levels.len() as f32 + time * 0.05).gamma_multiply(0.45 + 0.55 * level);
        painter.rect_filled(Rect::from_min_max(pos2(x, mid - half), pos2(x + width, mid + half)), 3, color);
    }
}

fn led(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, colors: &Colors) {
    let rect = rect.shrink2(vec2(rect.width() * 0.08, rect.height() * 0.15));
    let (cols, rows) = (24, 18);
    // Cada columna promedia dos bandas.
    let pair = |v: &[f32], c: usize| (v.get(c * 2).copied().unwrap_or(0.0) + v.get(c * 2 + 1).copied().unwrap_or(0.0)) / 2.0;
    let gap = 3.0;
    let segment = (rect.height() - gap * (rows - 1) as f32) / rows as f32;
    for (c, (x, width)) in columns(rect, cols, 5.0).enumerate() {
        let level = pair(&spectrum.levels, c);
        let peak = pair(&spectrum.peaks, c);
        let peak_row = ((peak * rows as f32) as usize).min(rows - 1);
        for row in 0..rows {
            let frac = row as f32 / rows as f32;
            // Tres zonas como un vúmetro: bajo, medio y pico toman el inicio, el medio y el final de la paleta.
            let color = colors.span(match frac {
                f if f < 0.6 => 0.0,
                f if f < 0.85 => 0.5,
                _ => 1.0,
            });
            let lit = frac < level || (row == peak_row && peak > 0.02);
            let y = rect.bottom() - (row + 1) as f32 * (segment + gap) + gap;
            let cell = Rect::from_min_size(pos2(x, y), vec2(width, segment));
            painter.rect_filled(cell, 1, if lit { color } else { color.gamma_multiply(0.08) });
        }
    }
}

fn mountains(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, colors: &Colors) {
    let rect = rect.shrink2(vec2(rect.width() * 0.04, rect.height() * 0.15));
    let n = spectrum.levels.len();
    let point = |i: usize, v: f32| pos2(rect.left() + rect.width() * i as f32 / (n - 1) as f32, rect.bottom() - rect.height() * v);
    // Relleno: una franja de triángulos entre la curva y la base, que se desvanece hacia abajo.
    let mut mesh = Mesh::default();
    for (i, &level) in spectrum.levels.iter().enumerate() {
        let color = colors.span(i as f32 / n as f32);
        mesh.colored_vertex(point(i, level), color.gamma_multiply(0.55));
        mesh.colored_vertex(point(i, 0.0), color.gamma_multiply(0.02));
        if i > 0 {
            let a = (i as u32 - 1) * 2;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
    }
    painter.add(Shape::mesh(mesh));
    for (i, pair) in spectrum.levels.windows(2).enumerate() {
        let segment = [point(i, pair[0]), point(i + 1, pair[1])];
        painter.line_segment(segment, Stroke::new(2.5, colors.span(i as f32 / n as f32)));
    }
    for (i, &peak) in spectrum.peaks.iter().enumerate() {
        painter.circle_filled(point(i, peak) - vec2(0.0, 6.0), 2.5, Color32::WHITE.gamma_multiply(0.8));
    }
}

fn oscilloscope(painter: &egui::Painter, rect: Rect, wave: &[f32], color: Color32) {
    // Un cuarto de la ventana (~23 ms): lo que muestra un osciloscopio típico sin volverse una mancha.
    let half = wave.len() / 4;
    if half < 2 {
        return;
    }
    // Arranca en un cruce por cero hacia arriba para que la onda no salte de lado en cada cuadro.
    let start = (1..half).find(|&i| wave[i - 1] < 0.0 && wave[i] >= 0.0).unwrap_or(0);
    let shown = &wave[start..start + half];
    let peak = shown.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = rect.height() * 0.38 / peak.max(0.3);
    let step = (half / 600).max(1);
    let points: Vec<Pos2> = shown
        .iter()
        .step_by(step)
        .enumerate()
        .map(|(k, s)| pos2(rect.left() + rect.width() * (k * step) as f32 / half as f32, rect.center().y - s * gain))
        .collect();
    // Tres trazos del mismo camino, de ancho y tenue a fino e intenso: efecto neón.
    for (width, k) in [(14.0, 0.10), (6.0, 0.28), (2.0, 1.0)] {
        painter.add(Shape::line(points.clone(), Stroke::new(width, additive(color, k))));
    }
}

fn ring(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, time: f32, colors: &Colors) {
    let center = rect.center();
    let size = rect.width().min(rect.height());
    let bass = spectrum.average(0..8);
    let r0 = size * 0.2 * (1.0 + bass * 0.25);
    let n = spectrum.levels.len() * 2;
    painter.circle_filled(center, r0 * 0.92, additive(colors.at(time * 0.05), 0.05 + bass * 0.2));
    let width = TAU * r0 / n as f32 * 0.6;
    for k in 0..n {
        let band = if k < n / 2 { k } else { n - 1 - k };
        let level = spectrum.levels[band];
        let angle = k as f32 / n as f32 * TAU + time * 0.15 - TAU / 4.0;
        let dir = vec2(angle.cos(), angle.sin());
        let color = colors.at(k as f32 / n as f32 + time * 0.05).gamma_multiply(0.5 + 0.5 * level);
        painter.line_segment([center + dir * r0, center + dir * (r0 + 4.0 + level * size * 0.28)], Stroke::new(width, color));
    }
}

fn aurora(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, time: f32, colors: &Colors) {
    let waves = 5;
    let n = 96;
    let mid = rect.center().y + rect.height() * 0.1;
    for w in 0..waves {
        let energy = spectrum.average(w * BANDS / waves..(w + 1) * BANDS / waves);
        let color = colors.span(w as f32 / (waves - 1) as f32);
        let amp = rect.height() * (0.08 + 0.35 * energy);
        let (freq, speed, phase) = (2.0 + w as f32 * 0.7, 0.4 + w as f32 * 0.15, w as f32 * 1.7);
        let mut mesh = Mesh::default();
        let mut line = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let u = i as f32 / n as f32;
            // La envolvente apaga la cinta en los bordes de la pantalla.
            let y = mid - ((u * freq + time * speed) * PI + phase).sin() * amp * (u * PI).sin();
            let top = pos2(rect.left() + rect.width() * u, y);
            mesh.colored_vertex(top, additive(color, 0.35));
            mesh.colored_vertex(pos2(top.x, mid), additive(color, 0.0));
            if i > 0 {
                let a = (i as u32 - 1) * 2;
                mesh.add_triangle(a, a + 1, a + 2);
                mesh.add_triangle(a + 1, a + 3, a + 2);
            }
            line.push(top);
        }
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::line(line, Stroke::new(2.0, additive(color, 0.8))));
    }
}

fn starfield(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, colors: &Colors) {
    let center = rect.center();
    let reach = rect.size().length() * 0.5;
    let bass = spectrum.average(0..8);
    for k in 0..400u32 {
        let angle = hash(k) * TAU;
        // z va de 0 (lejos) a 1 (encima de la pantalla) y vuelve a empezar.
        let z = (hash(k + 9_999) + spectrum.travel * 0.3).fract();
        let depth = 1.0 - z * 0.98;
        let radius = reach * 0.04 / depth;
        if radius > reach {
            continue;
        }
        let dir = vec2(angle.cos(), angle.sin());
        // La estela se alarga con los graves, como un salto a velocidad luz.
        let tail = reach * 0.04 / (depth + 0.02 + bass * 0.08);
        // Una de cada tres estrellas toma un color de la paleta; el resto es blanca.
        let color = if k % 3 == 0 { colors.at(hash(k + 77)) } else { Color32::WHITE };
        painter.line_segment([center + dir * tail, center + dir * radius], Stroke::new(0.5 + 2.5 * z, color.gamma_multiply(z * z)));
    }
}

/// Inspirado en los visualizadores de Windows Media Player: un plasma de colores que fluye y
/// late con los graves, y encima un caleidoscopio de ecos que se expanden.
///
/// Todo lo que reacciona a la música usa valores suavizados (`pulse`) o acumulados (`flow`): si la
/// música entrara directo en la fase de un seno, cada golpe haría saltar la imagen entera.
fn psychedelic(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum, time: f32, colors: &Colors) {
    let pulse = spectrum.pulse;
    let flow = spectrum.flow;

    // Fondo: malla con un color por vértice; la GPU interpola entre vértices y queda un degradado continuo.
    let (cols, rows) = (64u32, 36u32);
    let aspect = rect.width() / rect.height().max(1.0);
    let mut mesh = Mesh::default();
    for j in 0..=rows {
        for i in 0..=cols {
            let (u, v) = (i as f32 / cols as f32, j as f32 / rows as f32);
            let (x, y) = ((u - 0.5) * aspect * 6.0, (v - 0.5) * 6.0);
            let d = (x * x + y * y).sqrt();
            let wave = (x * 1.3 + time * 0.5).sin()
                + (y * 1.7 - time * 0.6).sin()
                + (d * 2.2 - flow * 1.5).sin()
                + ((x + y) * 0.9 + time * 0.3).sin();
            let hue = (wave * 0.1 + time * 0.03).rem_euclid(1.0);
            let value = (0.2 + 0.35 * pulse + 0.12 * (wave * 0.25 + 0.5)).min(1.0);
            mesh.colored_vertex(rect.lerp_inside(vec2(u, v)), colors.at(hue).gamma_multiply(value));
        }
    }
    for j in 0..rows {
        for i in 0..cols {
            let a = j * (cols + 1) + i;
            let (b, c, d) = (a + 1, a + cols + 1, a + cols + 2);
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    painter.add(Shape::mesh(mesh));

    // Caleidoscopio: una flor con 6 ejes de simetría por cada instantánea reciente del espectro.
    let center = rect.center();
    let size = rect.height().min(rect.width());
    let symmetry = 6;
    let petal = 12; // puntos por medio pétalo; cada uno promedia 3 bandas para que el borde sea suave
    let echoes = 8;
    for (age, levels) in spectrum.history.iter().rev().take(echoes).enumerate() {
        let age = age as f32;
        let fade = (1.0 - age / echoes as f32).powf(1.5);
        let rotation = time * 0.15 + age * 0.06;
        let scale = 1.0 + age * 0.12;
        let points: Vec<Pos2> = (0..symmetry * petal * 2)
            .map(|k| {
                // Cada pétalo recorre las bandas de ida y de vuelta, en espejo.
                let within = k % (petal * 2);
                let step = if within < petal { within } else { petal * 2 - 1 - within };
                let level = (0..3).map(|o| levels.get(step * 2 + o).copied().unwrap_or(0.0)).sum::<f32>() / 3.0;
                let angle = k as f32 / (symmetry * petal * 2) as f32 * TAU + rotation;
                let radius = (size * (0.14 + 0.08 * pulse) + level * size * 0.14) * scale;
                center + vec2(angle.cos(), angle.sin()) * radius
            })
            .collect();
        let hue = (time * 0.05 + age * 0.05 + 0.5).rem_euclid(1.0);
        let color = colors.at(hue).lerp_to_gamma(Color32::WHITE, 0.35).gamma_multiply(fade);
        painter.add(Shape::closed_line(points, Stroke::new(1.0 + 2.0 * fade, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_its_band() {
        let rate = 44_100;
        let tone: Vec<f32> = (0..WINDOW).map(|i| (TAU * 1000.0 * i as f32 / rate as f32).sin() * 0.5).collect();
        let levels = bands(&tone, rate);
        let loudest = levels.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let ratio = (MAX_HZ / MIN_HZ).powf(1.0 / BANDS as f32);
        let expected = ((1000.0 / MIN_HZ).ln() / ratio.ln()) as usize;
        assert_eq!(loudest, expected);
        // Tono de amplitud 0.5 a 1 kHz, donde la pendiente vale 1: la banda mide ~0.5 y las lejanas casi nada.
        assert!((levels[loudest] - 0.5).abs() < 0.1 && levels[2] < 0.01, "{levels:?}");
        assert!(Style::from_key(Style::Psychedelic.key()) == Style::Psychedelic);
    }

    #[test]
    fn levels_move_smoothly() {
        let feed = Feed::default();
        let mut s = Spectrum::default();
        let tone = rodio::source::SineWave::new(1000.0).amplify(0.5);
        Tap::new(tone, feed.clone()).take(WINDOW).for_each(drop);
        // Un cuadro a 60 fps no llega de golpe al máximo, y un cuadro sin muestras nuevas no lo tira a cero.
        s.update(&feed, 1.0 / 60.0);
        let first = s.levels.iter().copied().fold(0.0, f32::max);
        s.update(&feed, 1.0 / 60.0);
        let second = s.levels.iter().copied().fold(0.0, f32::max);
        assert!(first > 0.1 && first < 0.5 && second > first, "{first} {second}");
    }
}

/// Herramienta para ajustar las constantes de arriba: pasa canciones reales (una tranquila y dos
/// fuertes) por la misma tubería que la app y muestra en qué rango se mueve cada banda.
/// `cargo test --release diag_real_music -- --ignored --nocapture`.
#[cfg(test)]
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn diag_real_music() {
    use crate::khinsider;
    let c = khinsider::client();
    for query in ["minecraft volume alpha", "metal gear rising revengeance vocal", "doom eternal"] {
        let (albums, _) = khinsider::list(&c, &khinsider::search_path(query), 1).await.unwrap();
        let d = khinsider::album(&c, &albums[0].slug).await.unwrap();
        let track = d.tracks.iter().find(|t| khinsider::seconds(&t.duration) > 90.0).unwrap();
        let mp3 = khinsider::audio(&c, track).await.unwrap();
        let dec = rodio::Decoder::try_from(std::io::Cursor::new(mp3)).unwrap();
        let (ch, rate) = (dec.channels().get() as usize, dec.sample_rate().get());
        // Igual que en la app: Tap sobre el decodificador y un update por cuadro a 60 fps.
        let feed = Feed::default();
        let mut tap = Tap::new(dec, feed.clone());
        let mut spectrum = Spectrum::default();
        let per_frame = rate as usize / 60 * ch;
        let mut shown: Vec<Vec<f32>> = Vec::new();
        for _ in 0..60 * 60 {
            if tap.by_ref().take(per_frame).count() < per_frame {
                break;
            }
            spectrum.update(&feed, 1.0 / 60.0);
            shown.push(spectrum.levels.clone());
        }
        println!("\n{} / {} (lo que se dibuja, {} cuadros)", d.title, track.name, shown.len());
        for b in (0..BANDS).step_by(4) {
            let mut v: Vec<f32> = shown.iter().map(|f| f[b]).collect();
            v.sort_by(f32::total_cmp);
            let hz = MIN_HZ * (MAX_HZ / MIN_HZ).powf(b as f32 / BANDS as f32);
            println!("banda {b:2} {hz:7.0} Hz  p10 {:.2}  mediana {:.2}  p90 {:.2}", v[v.len() / 10], v[v.len() / 2], v[v.len() * 9 / 10]);
        }
    }
}
