//! Colores principales de una portada y el degradado pixel art que se arma con ellos.

use crate::theme::{contrast, lerp};
use eframe::egui::{Color32, Rect, Shape, pos2, vec2};
use std::collections::HashMap;

/// Hasta tres colores principales de la imagen, del más frecuente al menos frecuente. Los colores se
/// agrupan en cubos (8 niveles por canal) y se descartan los que se parecen demasiado a uno ya elegido.
pub fn dominant(bytes: &[u8]) -> Vec<Color32> {
    let Ok(img) = image::load_from_memory(bytes) else { return Vec::new() };
    let small = img.thumbnail(32, 32).to_rgb8();
    let mut buckets: HashMap<[u8; 3], (u32, [u32; 3])> = HashMap::new();
    for p in small.pixels() {
        let (n, sum) = buckets.entry([p[0] >> 5, p[1] >> 5, p[2] >> 5]).or_default();
        *n += 1;
        (0..3).for_each(|i| sum[i] += u32::from(p[i]));
    }
    let mut groups: Vec<_> = buckets.into_values().collect();
    groups.sort_by(|a, b| b.0.cmp(&a.0));
    let mut chosen: Vec<[u8; 3]> = Vec::new();
    for (n, sum) in groups {
        let c = sum.map(|s| (s / n) as u8);
        let far = |o: &[u8; 3]| (0..3).map(|i| (f32::from(o[i]) - f32::from(c[i])).powi(2)).sum::<f32>().sqrt() > 80.0;
        if chosen.iter().all(far) {
            chosen.push(c);
        }
        if chosen.len() == 3 {
            break;
        }
    }
    chosen.into_iter().map(|[r, g, b]| Color32::from_rgb(r, g, b)).collect()
}

/// Mezcla los colores con `bg` lo justo para que cada texto de `fgs` pase 4.5:1 sobre el degradado.
/// Revisa 49 puntos, que incluyen los 7 escalones de `pixel_gradient`.
pub fn readable(stops: &[Color32], bg: Color32, fgs: &[Color32]) -> Vec<Color32> {
    (0..=20)
        .map(|k| k as f32 / 20.0)
        .map(|k| stops.iter().map(|&c| lerp(c, bg, k)).collect::<Vec<_>>())
        .find(|mixed| (0..=48).map(|i| sample(mixed, i as f32 / 48.0)).all(|c| fgs.iter().all(|&fg| contrast(fg, c) >= 4.5)))
        .unwrap_or_else(|| vec![bg])
}

/// Color del degradado en `x` (0 a 1), interpolado entre los colores.
fn sample(stops: &[Color32], x: f32) -> Color32 {
    match stops {
        [] => Color32::TRANSPARENT,
        [only] => *only,
        _ => {
            let pos = x.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
            let i = (pos as usize).min(stops.len() - 2);
            lerp(stops[i], stops[i + 1], pos - i as f32)
        }
    }
}

/// Matriz de Bayer 4x4: el tramado ordenado de los juegos de 8 y 16 bits.
const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

/// Degradado pixel art que cubre `rect`, de izquierda a derecha: celdas cuadradas de `cell` px, el
/// color en `BANDS` escalones, y tramado Bayer en el borde entre un escalón y el siguiente.
pub fn pixel_gradient(rect: Rect, stops: &[Color32], cell: f32) -> Vec<Shape> {
    const BANDS: f32 = 7.0;
    let (cols, rows) = ((rect.width() / cell).ceil() as usize, (rect.height() / cell).ceil() as usize);
    let mut shapes = Vec::with_capacity(cols * rows);
    for col in 0..cols {
        let x = col as f32 / (cols.max(2) - 1) as f32 * (BANDS - 1.0);
        let (band, frac) = (x.floor(), x.fract());
        for row in 0..rows {
            let step = band + f32::from(frac > (BAYER[row % 4][col % 4] + 0.5) / 16.0);
            let min = pos2(rect.left() + col as f32 * cell, rect.top() + row as f32 * cell);
            let cell_rect = Rect::from_min_size(min, vec2(cell, cell)).intersect(rect);
            shapes.push(Shape::rect_filled(cell_rect, 0, sample(stops, step / (BANDS - 1.0))));
        }
    }
    shapes
}

#[test]
fn picks_main_colors_and_keeps_text_readable() {
    let img = image::RgbImage::from_fn(40, 40, |x, _| if x < 24 { image::Rgb([220, 30, 30]) } else { image::Rgb([20, 60, 200]) });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let colors = dominant(&png);
    assert_eq!(colors.len(), 2);
    assert!(colors[0].r() > 200 && colors[1].b() > 180, "{colors:?}");

    let (bg, text, muted) = (Color32::from_rgb(0x10, 0x12, 0x16), Color32::from_rgb(0xe6, 0xe8, 0xec), Color32::from_rgb(0x8b, 0x91, 0x9d));
    let stops = readable(&colors, bg, &[text, muted]);
    assert!((0..=16).all(|i| contrast(muted, sample(&stops, i as f32 / 16.0)) >= 4.5));
}
