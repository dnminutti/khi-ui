//! Biblioteca local: los álbumes descargados en la carpeta de descargas.
//!
//! Cada álbum es una subcarpeta con sus pistas, la portada (`cover.*`) y un `album.tsv` que guarda el
//! título y el nombre y la duración de cada pista. Un álbum +18 lleva además un archivo `.nsfw`.
//! Una carpeta sin `album.tsv` también se lee: el título es el nombre de la carpeta y las pistas son
//! sus archivos de audio, sin duración.

use crate::khinsider::{Album, AlbumDetail, Result, Track};
use std::path::Path;

/// Prefijo de los paths locales en `Album::slug`, `Track::path` y las portadas, para distinguirlos de
/// los del sitio.
pub const LOCAL: &str = "file://";

const AUDIO: [&str; 2] = ["mp3", "flac"];

/// Subcarpetas de la carpeta de descargas, ordenadas por nombre.
pub fn albums(root: &Path) -> Vec<Album> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut albums: Vec<Album> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|dir| Album {
            nsfw: is_nsfw(&dir),
            name: title(&dir),
            thumb: cover(&dir).map(|c| format!("{LOCAL}{}", c.display())),
            slug: format!("{LOCAL}{}", dir.display()),
        })
        .collect();
    albums.sort_by_key(|a| a.name.to_lowercase());
    albums
}

pub fn album(dir: &Path) -> Result<AlbumDetail> {
    let tracks = match std::fs::read_to_string(dir.join("album.tsv")) {
        Ok(tsv) => tsv
            .lines()
            .skip(1)
            .filter_map(|l| {
                let mut cols = l.split('\t');
                let (file, name, duration) = (cols.next()?, cols.next()?, cols.next().unwrap_or_default());
                Some(Track { path: format!("{LOCAL}{}", dir.join(file).display()), name: name.into(), duration: duration.into() })
            })
            .collect(),
        Err(_) => {
            let mut files: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).filter(|p| is_audio(p)).collect();
            files.sort();
            files
                .into_iter()
                .map(|p| Track {
                    name: p.file_stem().unwrap_or_default().to_string_lossy().into(),
                    path: format!("{LOCAL}{}", p.display()),
                    duration: String::new(),
                })
                .collect()
        }
    };
    Ok(AlbumDetail { title: title(dir), cover: cover(dir).map(|c| format!("{LOCAL}{}", c.display())), tracks, flac: false, nsfw: is_nsfw(dir), id: None })
}

/// Escribe `album.tsv` con el título y, por pista, el archivo, el nombre y la duración.
pub fn save(dir: &Path, title: &str, tracks: &[(String, &Track)]) -> std::io::Result<()> {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut tsv = clean(title) + "\n";
    for (file, track) in tracks {
        tsv += &format!("{}\t{}\t{}\n", file, clean(&track.name), track.duration);
    }
    std::fs::write(dir.join("album.tsv"), tsv)
}

/// La descarga de un álbum +18 deja un archivo `.nsfw` vacío junto a la portada.
pub const NSFW_MARK: &str = ".nsfw";

fn is_nsfw(dir: &Path) -> bool {
    dir.join(NSFW_MARK).exists()
}

fn title(dir: &Path) -> String {
    let saved = std::fs::read_to_string(dir.join("album.tsv")).ok().and_then(|t| t.lines().next().map(str::to_string));
    saved.filter(|t| !t.is_empty()).unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into())
}

fn cover(dir: &Path) -> Option<std::path::PathBuf> {
    ["jpg", "jpeg", "png", "webp", "gif"].iter().map(|ext| dir.join(format!("cover.{ext}"))).find(|p| p.is_file())
}

fn is_audio(p: &Path) -> bool {
    p.extension().is_some_and(|x| AUDIO.iter().any(|a| x.eq_ignore_ascii_case(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_saved_and_plain_folders() {
        let root = std::env::temp_dir().join(format!("khi-ui-library-{}", std::process::id()));
        let (saved, plain) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&saved).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        let track = Track { path: String::new(), name: "Intro\tTheme".into(), duration: "1:09".into() };
        save(&saved, "Zelda: OST", &[("01 Intro.mp3".into(), &track)]).unwrap();
        std::fs::write(plain.join("02 B.flac"), b"").unwrap();
        std::fs::write(plain.join("01 A.mp3"), b"").unwrap();
        std::fs::write(plain.join("notes.txt"), b"").unwrap();

        let names: Vec<_> = albums(&root).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["b", "Zelda: OST"]);
        let a = album(&saved).unwrap();
        assert_eq!((a.tracks[0].name.as_str(), a.tracks[0].duration.as_str()), ("Intro Theme", "1:09"));
        assert!(a.tracks[0].path.ends_with("01 Intro.mp3"));
        let b = album(&plain).unwrap();
        assert_eq!(b.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["01 A", "02 B"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
