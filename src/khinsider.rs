//! Scraping de downloads.khinsider.com.
//!
//! Cloudflare bloquea la huella TLS/HTTP2 de clientes comunes (curl, reqwest, OpenSSL):
//! por eso `wreq` emula a Safari. Con la emulación de Chrome también responde 403.

use crate::i18n::tr;
use regex::Regex;
use std::sync::LazyLock;

const BASE: &str = "https://downloads.khinsider.com";

#[derive(Clone)]
pub struct Album {
    pub slug: String,
    pub name: String,
    pub thumb: Option<String>,
    /// Portada +18: el sitio la tapa y la lista no trae su miniatura.
    pub nsfw: bool,
}

#[derive(Clone)]
pub struct Track {
    pub path: String,
    pub name: String,
    pub duration: String,
}

/// Una opción para filtrar el catálogo (una letra, plataforma, tipo o año) y la lista que abre.
#[derive(Clone, PartialEq)]
pub struct Filter {
    pub path: String,
    pub name: String,
    pub count: Option<u32>,
}

pub struct AlbumDetail {
    pub title: String,
    pub cover: Option<String>,
    pub tracks: Vec<Track>,
    /// Hay pistas que se pueden descargar en FLAC además de MP3.
    pub flac: bool,
    /// Portada +18; `cover` igual trae la real cuando el sitio la enlaza.
    pub nsfw: bool,
    /// Id del álbum en el sitio, para marcarlo como favorito en la cuenta.
    pub id: Option<String>,
}

/// Formato de audio de una descarga.
#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Mp3,
    Flac,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::Flac => "flac",
        }
    }
}

pub type Result<T> = std::result::Result<T, String>;

pub fn client() -> wreq::Client {
    wreq::Client::builder()
        .emulation(wreq_util::Emulation::Safari26)
        // La sesión de usuario vive en las cookies de XenForo (`xf_session`, `xf_user`).
        .cookie_store(true)
        .build()
        .expect("cliente HTTP")
}

// Las listas (Latest, Top 40, Top 1000, Most Favorites...) comparten el formato de fila:
// una celda con la miniatura enlazada y otra con el nombre enlazado.
static THUMB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<a href="/game-soundtracks/album/([^"?]+)">\s*<img src="([^"]+)""#).unwrap());
static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<td>\s*<a href="/game-soundtracks/album/([^"?]+)">([^<]+)</a>"#).unwrap());
static FILTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<a href="(/game-soundtracks/[^"]+)">([^<]+)</a> \((\d+)\)"#).unwrap());
static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/game-soundtracks/album/([^?]+)").unwrap());
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<h2>([^<]+)</h2>").unwrap());
// El enlace lleva a la portada en tamaño completo y la imagen es su miniatura, salvo en los álbumes
// +18, donde la imagen es un aviso del sitio.
static COVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<div class="albumImage">\s*<a href="([^"]+)"[^>]*>\s*<img src="([^"]+)""#).unwrap());
static TRACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<td class="clickable-row"><a href="([^"]+)">([^<]+)</a></td>\s*<td class="clickable-row" align="right"><a [^>]*>([^<]+)</a>"#).unwrap()
});
static XF_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"name="_xfToken" value="([^"]+)""#).unwrap());
static XF_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)blockMessage--error[^>]*>(.*?)</div>"#).unwrap());
// Mis playlists: miniatura (opcional si está vacía) y nombre, ambos enlazados a la playlist.
static PLAYLIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a href="(/playlist/\w+)">(?:<img src="([^"]+)">)?</a></td>\s*<td><a href="/playlist/\w+">([^<]+)</a>"#).unwrap()
});
static PLAYLIST_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"id="playlistTitle">Playlist: ([^<\t]+)"#).unwrap());
static ALBUM_ICON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<td class="albumIcon"><a [^>]*><img src="([^"]+)""#).unwrap());
// En una playlist cada canción trae su nombre y, debajo, el enlace a su álbum.
static PLAYLIST_TRACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<td class="clickable-row">\s*<a href="([^"]+)">([^<]+)</a><br>\s*<a [^>]*>[^<]*</a>\s*</td>\s*<td class="clickable-row" align="right"><a [^>]*>([^<]+)</a>"#).unwrap()
});
static ALBUM_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"/cp/add_album/(\d+)"#).unwrap());
static AUDIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<audio[^>]*src="([^"]+)""#).unwrap());
static FLAC_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<a href="([^"]+\.flac)"><span class="songDownloadLink">"#).unwrap());

/// Una página de una lista de álbumes, y si existe la página siguiente.
pub async fn list(c: &wreq::Client, path: &str, page: u32) -> Result<(Vec<Album>, bool)> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let html = text(c, &format!("{path}{sep}page={page}")).await?;
    // En la home, la lista está después de este id; antes hay otros enlaces a álbumes.
    let section = html.split_once("homepageLatestSoundtracks").map_or(html.as_str(), |(_, s)| s);
    // El enlace a la página siguiente es `?page=N"` en los tops y `?page=N&orderby=...` en el catálogo.
    let next = format!("href=\"?page={}", page + 1);
    let has_more = html.match_indices(&next).any(|(i, _)| matches!(html.as_bytes().get(i + next.len()), Some(b'"' | b'&')));
    let albums = if path.starts_with("/playlist/") { parse_playlists(section) } else { parse_list(section) };
    Ok((albums, has_more))
}

/// Opciones de páginas índice como `/console-list` o `/album-years`: enlace, nombre y cantidad de álbumes.
pub async fn filters(c: &wreq::Client, path: &str) -> Result<Vec<Filter>> {
    let html = text(c, path).await?;
    let content = html.split_once("pageContent").map_or(html.as_str(), |(_, s)| s);
    Ok(parse_filters(content))
}

fn parse_filters(html: &str) -> Vec<Filter> {
    FILTER.captures_iter(html).map(|m| Filter { path: m[1].into(), name: decode(&m[2]), count: m[3].parse().ok() }).collect()
}

/// Path de la búsqueda de álbumes; se pagina con [`list`] como cualquier otra lista.
pub fn search_path(query: &str) -> String {
    let encoded: String = query
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("/search?search={encoded}")
}

/// `/random-album` responde con un redirect a un álbum al azar; se lee el slug del `Location`.
pub async fn random_album(c: &wreq::Client) -> Result<String> {
    let r = c.get(format!("{BASE}/random-album")).send().await.map_err(|e| e.to_string())?;
    let location = r.headers().get("location").and_then(|l| l.to_str().ok()).unwrap_or_default();
    SLUG.captures(location).map(|m| m[1].to_string()).ok_or_else(|| format!("{}: HTTP {}", tr("redirect inesperado", "unexpected redirect"), r.status()))
}

/// Inicia sesión en el foro (XenForo) con el formulario de login. El sitio contesta con un redirect
/// cuando los datos son correctos, y con la misma página y un mensaje de error cuando no.
pub async fn login(c: &wreq::Client, user: &str, password: &str) -> Result<()> {
    let page = text(c, "/forums/login").await?;
    let token = XF_TOKEN.captures(&page).ok_or(tr("no se encontró el formulario de login", "login form not found"))?[1].to_string();
    let form = [("login", user), ("password", password), ("remember", "1"), ("_xfToken", &token), ("_xfRedirect", BASE)];
    let r = c.post(format!("{BASE}/forums/index.php?login/login")).form(&form).send().await.map_err(|e| e.to_string())?;
    let location = r.headers().get("location").and_then(|l| l.to_str().ok()).unwrap_or_default().to_string();
    let status = r.status();
    if status.is_redirection() {
        return match location.contains("two-step") {
            true => Err(tr("la cuenta usa verificación en dos pasos, que la app todavía no soporta", "the account uses two-step verification, which the app does not support yet").into()),
            false => Ok(()),
        };
    }
    let html = String::from_utf8_lossy(&r.bytes().await.map_err(|e| e.to_string())?).into_owned();
    let message = XF_ERROR.captures(&html).map(|m| strip_tags(&m[1])).filter(|m| !m.is_empty());
    Err(message.unwrap_or_else(|| format!("{}: HTTP {status}", tr("no se pudo iniciar sesión", "could not log in"))))
}

pub async fn album(c: &wreq::Client, slug: &str) -> Result<AlbumDetail> {
    let html = text(c, &format!("/game-soundtracks/album/{slug}")).await?;
    Ok(parse_album(&html, slug))
}

/// Álbumes favoritos de la cuenta, con todas sus páginas.
pub async fn favorites(c: &wreq::Client) -> Result<Vec<Album>> {
    let mut albums = Vec::new();
    for page in 1.. {
        let (more_albums, more) = list(c, "/cp/favorites", page).await?;
        albums.extend(more_albums);
        if !more {
            break;
        }
    }
    Ok(albums)
}

/// Agrega el álbum a los favoritos de la cuenta, o lo quita si ya estaba. Es el mismo pedido que hace
/// el botón «Add to Favorites» del sitio.
pub async fn toggle_favorite(c: &wreq::Client, album_id: &str) -> Result<()> {
    bytes(c, &format!("{BASE}/cp/album_favorite_toggle?albumid={album_id}")).await.map(drop)
}

/// Una playlist del usuario (`/playlist/<id>`), con la misma forma que un álbum.
pub async fn playlist(c: &wreq::Client, path: &str) -> Result<AlbumDetail> {
    Ok(parse_playlist(&text(c, path).await?, path))
}

/// Cada pista tiene su propia página con el enlace directo al MP3; se resuelve y se descarga.
pub async fn audio(c: &wreq::Client, track: &Track) -> Result<Vec<u8>> {
    let html = text(c, &track.path).await?;
    let url = AUDIO.captures(&html).ok_or(tr("no se encontró el audio de la pista", "track audio not found"))?[1].to_string();
    bytes(c, &url).await
}

/// Enlace directo a la pista en el formato pedido y el formato que se consiguió: si la pista no está
/// en FLAC, el MP3.
pub async fn file_url(c: &wreq::Client, track: &Track, format: Format) -> Result<(String, Format)> {
    parse_file_url(&text(c, &track.path).await?, format).ok_or_else(|| tr("no se encontró el audio de la pista", "track audio not found").into())
}

fn parse_file_url(html: &str, format: Format) -> Option<(String, Format)> {
    let flac = FLAC_LINK.captures(html).filter(|_| format == Format::Flac).map(|m| (m[1].to_string(), Format::Flac));
    flac.or_else(|| AUDIO.captures(html).map(|m| (m[1].to_string(), Format::Mp3)))
}

pub async fn bytes(c: &wreq::Client, url: &str) -> Result<Vec<u8>> {
    let r = c.get(encode_non_ascii(url)).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {} {} {url}", r.status(), tr("en", "at")));
    }
    Ok(r.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// Codifica en `%XX` los caracteres que no son ASCII. El sitio los deja tal cual en algunos enlaces
/// (`hÁgua-amanhã`), el cliente HTTP los manda sin codificar y el servidor responde 400.
fn encode_non_ascii(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            c.encode_utf8(&mut buf).bytes().for_each(|b| out.push_str(&format!("%{b:02X}")));
        }
    }
    out
}

async fn text(c: &wreq::Client, path: &str) -> Result<String> {
    // El path ya viene codificado desde el HTML; se usa tal cual.
    let b = bytes(c, &format!("{BASE}{path}")).await?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

fn parse_list(html: &str) -> Vec<Album> {
    let thumbs: std::collections::HashMap<&str, &str> =
        THUMB.captures_iter(html).map(|m| (m.get(1).unwrap().as_str(), m.get(2).unwrap().as_str())).collect();
    let mut seen = std::collections::HashSet::new();
    NAME.captures_iter(html)
        .filter(|m| seen.insert(m[1].to_string()))
        .map(|m| {
            let thumb = thumbs.get(&m[1]).copied();
            Album { slug: m[1].into(), name: decode(&m[2]), thumb: thumb.filter(|t| !is_nsfw(t)).map(str::to_string), nsfw: thumb.is_some_and(is_nsfw) }
        })
        .collect()
}

/// Las playlists se listan como álbumes; su `slug` es el path de la playlist.
fn parse_playlists(html: &str) -> Vec<Album> {
    PLAYLIST
        .captures_iter(html)
        .map(|m| {
            let thumb = m.get(2).map(|t| t.as_str());
            Album { slug: m[1].into(), name: decode(&m[3]), thumb: thumb.filter(|t| !is_nsfw(t)).map(str::to_string), nsfw: thumb.is_some_and(is_nsfw) }
        })
        .collect()
}

fn parse_playlist(html: &str, path: &str) -> AlbumDetail {
    AlbumDetail {
        title: PLAYLIST_TITLE.captures(html).map_or(path.into(), |m| decode(&m[1])),
        cover: ALBUM_ICON.captures(html).map(|m| m[1].to_string()).filter(|c| !is_nsfw(c)),
        tracks: PLAYLIST_TRACK
            .captures_iter(html)
            .map(|m| Track { path: m[1].into(), name: decode(&m[2]), duration: m[3].into() })
            .collect(),
        // Las pistas vienen de álbumes distintos y la página no dice cuáles tienen FLAC: se ofrece
        // igual y las que no lo tienen se bajan en MP3.
        flac: true,
        nsfw: ALBUM_ICON.captures(html).is_some_and(|m| is_nsfw(&m[1])),
        id: None,
    }
}

fn parse_album(html: &str, slug: &str) -> AlbumDetail {
    AlbumDetail {
        title: TITLE.captures(html).map_or(slug.into(), |m| decode(&m[1])),
        cover: COVER.captures(html).map(|m| match is_nsfw(&m[2]) {
            // La miniatura real vive junto a la portada completa, en `thumbs/`.
            true => m[1].rsplit_once('/').map_or(m[1].to_string(), |(dir, file)| format!("{dir}/thumbs/{file}")),
            false => m[2].to_string(),
        }),
        tracks: TRACK
            .captures_iter(html)
            .map(|m| Track { path: m[1].into(), name: decode(&m[2]), duration: m[3].into() })
            .collect(),
        // La tabla de pistas trae una columna de tamaño por formato.
        flac: html.contains("<b>FLAC</b></th>"),
        nsfw: COVER.captures(html).is_some_and(|m| is_nsfw(&m[2])),
        id: ALBUM_ID.captures(html).map(|m| m[1].into()),
    }
}

/// El sitio reemplaza las portadas +18 por `/images/nsfw.png` o `/images/nsfw_small.png`.
fn is_nsfw(src: &str) -> bool {
    src.starts_with("/images/nsfw")
}

/// Convierte "m:ss" (o "h:mm:ss") en segundos.
pub fn seconds(duration: &str) -> f32 {
    duration.split(':').fold(0.0, |acc, p| acc * 60.0 + p.trim().parse::<f32>().unwrap_or(0.0))
}

fn strip_tags(html: &str) -> String {
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
    decode(&TAG.replace_all(html, " ").split_whitespace().collect::<Vec<_>>().join(" "))
}

// ponytail: solo las entidades que aparecen en la práctica; usar un parser HTML si aparecen más.
fn decode(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pages() {
        // Formato de Latest y formato de Top 40, más una fila sin miniatura.
        let lists = r#"<td class="albumIcon"><a href="/game-soundtracks/album/x-gamerip"><img src="https://h/t.png"></a></td>
    <td>
                <a href="/game-soundtracks/album/x-gamerip">X &amp; Y</a>
	<td class="albumIcon">
		<a href="/game-soundtracks/album/minecraft">
			<img src="https://h/m.png">		</a>
	</td>
 	<td>1.</td>
	<td><a href="/game-soundtracks/album/minecraft">Minecraft</a></td>
    <td class="albumIcon"><a href="/game-soundtracks/album/demo"><div class="albumIconDefaultSmall"></div></a></td>
    <td>
                <a href="/game-soundtracks/album/demo">Demo</a>"#;
        let a = parse_list(lists);
        let got: Vec<_> = a.iter().map(|a| (a.slug.as_str(), a.name.as_str(), a.thumb.as_deref())).collect();
        assert_eq!(got, [("x-gamerip", "X & Y", Some("https://h/t.png")), ("minecraft", "Minecraft", Some("https://h/m.png")), ("demo", "Demo", None)]);
        let hidden = parse_list(r#"<td class="albumIcon"><a href="/game-soundtracks/album/h"><img src="/images/nsfw_small.png"></a></td>
    <td>
                <a href="/game-soundtracks/album/h">H</a>"#);
        assert!(hidden[0].nsfw && hidden[0].thumb.is_none() && !a[0].nsfw);
        let adult = parse_album(r#"<div class="albumImage">
            <a href="https://h/s/h/00%20Front.png" target="_blank">
                <img src="/images/nsfw.png">"#, "h");
        assert!(adult.nsfw);
        assert_eq!(adult.cover.as_deref(), Some("https://h/s/h/thumbs/00%20Front.png"));

        let page = r#"<h2>X</h2><div class="albumImage">
            <a href="https://h/C2.png" target="_blank">
                <img src="https://h/thumbs/C2.png">
	 	<td class="clickable-row"><a href="/game-soundtracks/album/x/01.%2520A.mp3">A</a></td>
   		<td class="clickable-row" align="right"><a href="/game-soundtracks/album/x/01.%2520A.mp3" style="font-weight:normal;">1:09</a></td>"#;
        let d = parse_album(page, "x");
        assert!(!d.flac && !d.nsfw && d.id.is_none());
        assert_eq!(parse_album(r#"<a href="/cp/add_album/7472">"#, "x").id.as_deref(), Some("7472"));
        assert!(parse_album(r#"<th width="60px" align="right"><b>FLAC</b></th>"#, "x").flac);
        assert_eq!(d.cover.as_deref(), Some("https://h/thumbs/C2.png"));
        assert_eq!((d.tracks[0].name.as_str(), d.tracks[0].duration.as_str()), ("A", "1:09"));
        assert_eq!(seconds("1:09"), 69.0);
        assert_eq!(search_path("zelda & link"), "/search?search=zelda+%26+link");
        assert_eq!(encode_non_ascii("https://h/hÁgua-amanhã/00%20Front.jpg"), "https://h/h%C3%81gua-amanh%C3%A3/00%20Front.jpg");
        let f = parse_filters(r#"<a href="/game-soundtracks/3do">3DO</a> (170)<br>
		<a href="/game-soundtracks/year/1995">1995</a> (1946)<br>"#);
        assert!(f[0] == Filter { path: "/game-soundtracks/3do".into(), name: "3DO".into(), count: Some(170) });
        assert_eq!(f[1].path, "/game-soundtracks/year/1995");

        let playlists = r#"<td class="albumIcon"><a href="/playlist/567hz"><img src="https://h/f.jpg"></a></td>
	<td><a href="/playlist/567hz">Best &amp; more</a></td>
	<td class="albumIcon"><a href="/playlist/99ab"></a></td>
	<td><a href="/playlist/99ab">Empty</a></td>"#;
        let p: Vec<_> = parse_playlists(playlists).into_iter().map(|a| (a.slug, a.name, a.thumb)).collect();
        assert_eq!(p, [("/playlist/567hz".into(), "Best & more".into(), Some("https://h/f.jpg".into())), ("/playlist/99ab".into(), "Empty".into(), None)]);

        let songs = "<p id=\"playlistTitle\">Playlist: Best\t\t<a href=\"/playlist/delete\">x</a>".to_string() + r#"
		<td class="albumIcon"><a href="/game-soundtracks/album/kof"><img src="https://h/kof.jpg"></a></td>
	 	<td class="clickable-row">
	 		<a href="/game-soundtracks/album/kof/03%2520E.mp3">E-Groove (Dj Turbo&#039;s Mix)</a><br>
	 		<a href="/game-soundtracks/album/kof" style="font-size: 10px;">K.O.F. Dance Trax</a>
	 	</td>
   		<td class="clickable-row" align="right"><a href="/game-soundtracks/album/kof/03%2520E.mp3" style="font-weight:normal;">6:58</a></td>"#;
        let d = parse_playlist(&songs, "/playlist/567hz");
        assert_eq!((d.title.as_str(), d.cover.as_deref()), ("Best", Some("https://h/kof.jpg")));
        let t = &d.tracks[0];
        assert_eq!((t.path.as_str(), t.name.as_str(), t.duration.as_str()), ("/game-soundtracks/album/kof/03%2520E.mp3", "E-Groove (Dj Turbo's Mix)", "6:58"));

        let song = r#"<p><a href="https://h/1.%20Key.mp3"><span class="songDownloadLink">MP3</span></a></p>
		<p><a href="https://h/1.%20Key.flac"><span class="songDownloadLink">FLAC</span></a></p>
<audio id="audio" controls preload="auto" src="https://h/1.%20Key.mp3" ></audio>"#;
        let only_mp3 = r#"<audio id="audio" src="https://h/2.mp3" ></audio>"#;
        let url = |html, f| parse_file_url(html, f).map(|(u, f)| (u, f.ext()));
        assert_eq!(url(song, Format::Flac), Some(("https://h/1.%20Key.flac".into(), "flac")));
        assert_eq!(url(song, Format::Mp3), Some(("https://h/1.%20Key.mp3".into(), "mp3")));
        assert_eq!(url(only_mp3, Format::Flac), Some(("https://h/2.mp3".into(), "mp3")));
    }
}

/// Prueba contra el sitio real: `cargo test -- --ignored`. Detecta cambios de HTML o de Cloudflare.
#[cfg(test)]
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn live_site() {
    let c = client();
    let (albums, _) = list(&c, "/", 1).await.unwrap();
    assert!(albums.len() > 10, "{} álbumes", albums.len());
    assert!(!bytes(&c, albums[0].thumb.as_ref().unwrap()).await.unwrap().is_empty());
    for path in ["/top40", "/last-6-months-top-100", "/top-100-newly-added", "/most-favorites"] {
        assert!(list(&c, path, 1).await.unwrap().0.len() >= 40, "{path}");
    }
    let (top, more) = list(&c, "/all-time-top-1000", 1).await.unwrap();
    assert!(top.len() == 100 && more);
    assert!(!random_album(&c).await.unwrap().is_empty());
    assert!(list(&c, &search_path("zelda"), 1).await.unwrap().0.len() > 50);
    assert!(filters(&c, "/console-list").await.unwrap().len() > 50);
    assert!(filters(&c, "/album-years").await.unwrap().len() > 40);
    let (letter, more) = list(&c, "/game-soundtracks/browse/A", 1).await.unwrap();
    assert!(letter.len() > 100 && more, "{} {more}", letter.len());
    let d = album(&c, &albums[0].slug).await.unwrap();
    assert!(d.cover.is_some() && !d.tracks.is_empty(), "{}", d.title);
    let mp3 = audio(&c, &d.tracks[0]).await.unwrap();
    rodio::Decoder::try_from(std::io::Cursor::new(mp3)).unwrap();
    println!("{} álbumes; '{}' con {} pistas", albums.len(), d.title, d.tracks.len());
}
