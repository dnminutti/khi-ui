//! Favoritos locales, guardados en `~/.config/khi-ui/favorites.tsv`. Las canciones siempre se guardan
//! acá, porque el sitio no tiene canciones favoritas. Los álbumes se guardan acá solo sin sesión
//! iniciada; con sesión van a los favoritos de la cuenta de KHInsider.
//!
//! Cada línea es `album`, slug, nombre, miniatura y marca +18, o `song`, path, nombre, duración,
//! álbum y portada del álbum.

use crate::khinsider::{Album, Track};

/// Una canción favorita con el álbum del que viene, para mostrarlo en el reproductor.
#[derive(Clone)]
pub struct Song {
    pub track: Track,
    pub album: String,
    pub cover: Option<String>,
}

#[derive(Default)]
pub struct Favorites {
    pub albums: Vec<Album>,
    pub songs: Vec<Song>,
}

impl Favorites {
    pub fn load() -> Self {
        let text = crate::settings::dir().and_then(|d| std::fs::read_to_string(d.join("favorites.tsv")).ok()).unwrap_or_default();
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let mut favorites = Favorites::default();
        for line in text.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            let opt = |i: usize| cols.get(i).filter(|s| !s.is_empty()).map(|s| s.to_string());
            match cols.as_slice() {
                ["album", slug, name, ..] => favorites.albums.push(Album {
                    slug: slug.to_string(),
                    name: name.to_string(),
                    thumb: opt(3),
                    nsfw: cols.get(4) == Some(&"1"),
                }),
                ["song", path, name, duration, album, ..] => favorites.songs.push(Song {
                    track: Track { path: path.to_string(), name: name.to_string(), duration: duration.to_string() },
                    album: album.to_string(),
                    cover: opt(5),
                }),
                _ => {}
            }
        }
        favorites
    }

    pub fn save(&self) {
        crate::settings::set("favorites.tsv", self.to_tsv());
    }

    fn to_tsv(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let mut tsv = String::new();
        for a in &self.albums {
            let nsfw = if a.nsfw { "1" } else { "0" };
            tsv += &format!("album\t{}\t{}\t{}\t{nsfw}\n", clean(&a.slug), clean(&a.name), clean(a.thumb.as_deref().unwrap_or_default()));
        }
        for s in &self.songs {
            let t = &s.track;
            tsv += &format!(
                "song\t{}\t{}\t{}\t{}\t{}\n",
                clean(&t.path),
                clean(&t.name),
                clean(&t.duration),
                clean(&s.album),
                clean(s.cover.as_deref().unwrap_or_default())
            );
        }
        tsv
    }

    pub fn has_album(&self, slug: &str) -> bool {
        self.albums.iter().any(|a| a.slug == slug)
    }

    pub fn has_song(&self, path: &str) -> bool {
        self.songs.iter().any(|s| s.track.path == path)
    }

    /// Agrega la canción o la quita si ya estaba, y guarda.
    pub fn toggle_song(&mut self, song: Song) {
        match self.songs.iter().position(|s| s.track.path == song.track.path) {
            Some(i) => _ = self.songs.remove(i),
            None => self.songs.push(song),
        }
        self.save();
    }

    /// Agrega el álbum o lo quita si ya estaba, y guarda.
    pub fn toggle_album(&mut self, album: Album) {
        match self.albums.iter().position(|a| a.slug == album.slug) {
            Some(i) => _ = self.albums.remove(i),
            None => self.albums.push(album),
        }
        self.save();
    }
}

#[test]
fn round_trip() {
    let mut f = Favorites::default();
    f.albums.push(Album { slug: "zelda".into(), name: "Zelda\tOST".into(), thumb: None, nsfw: true });
    let track = Track { path: "/game-soundtracks/album/zelda/01.mp3".into(), name: "Intro".into(), duration: "1:09".into() };
    f.songs.push(Song { track, album: "Zelda OST".into(), cover: Some("https://h/c.jpg".into()) });
    let back = Favorites::parse(&f.to_tsv());
    let a = &back.albums[0];
    assert_eq!((a.slug.as_str(), a.name.as_str(), a.thumb.is_none(), a.nsfw), ("zelda", "Zelda OST", true, true));
    let s = &back.songs[0];
    assert_eq!((s.track.duration.as_str(), s.album.as_str(), s.cover.as_deref()), ("1:09", "Zelda OST", Some("https://h/c.jpg")));
    assert!(back.has_song("/game-soundtracks/album/zelda/01.mp3") && back.has_album("zelda"));
}
