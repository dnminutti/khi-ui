mod account;
mod favorites;
mod i18n;
mod khinsider;
mod library;
mod palette;
mod settings;
mod theme;
mod visualizer;

use eframe::egui::{self, Align, Align2, Color32, FontFamily, FontId, Layout, Rect, RichText, Sense, vec2};
use i18n::{Lang, tr};
use khinsider::{Album, AlbumDetail, Filter, Format, Result, Track};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use theme::Theme;

type Audio = visualizer::Tap<rodio::Decoder<Cursor<Vec<u8>>>>;

/// Qué suena al terminar una pista.
#[derive(Clone, Copy, PartialEq, Default)]
enum Mode {
    #[default]
    Continue,
    Shuffle,
    Repeat,
    RepeatOne,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Continue, Mode::Shuffle, Mode::Repeat, Mode::RepeatOne];

    fn key(self) -> &'static str {
        ["continue", "shuffle", "repeat", "repeat-one"][self as usize]
    }

    fn from_key(key: &str) -> Mode {
        Self::ALL.into_iter().find(|m| m.key() == key).unwrap_or_default()
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Continue => tr("Continuar con la siguiente pista", "Continue with the next track"),
            Mode::Shuffle => tr("Aleatorio", "Shuffle"),
            Mode::Repeat => tr("Repetir el álbum", "Repeat album"),
            Mode::RepeatOne => tr("Repetir la canción", "Repeat track"),
        }
    }
}

/// Secciones de la barra lateral: título en español e inglés, y path de la lista en el sitio.
const SECTIONS: &[(&str, &str, &str)] = &[
    ("Últimos soundtracks", "Latest Soundtracks", "/"),
    ("Top 40", "Top 40", "/top40"),
    ("Top 1000 histórico", "Top 1000 All Time", "/all-time-top-1000"),
    ("Top 100 últimos 6 meses", "Top 100 Last 6 Months", "/last-6-months-top-100"),
    ("Top 100 recién agregados", "Top 100 Newly Added", "/top-100-newly-added"),
    ("Más favoritos", "Most Favorites", "/most-favorites"),
];

/// Valores de `App::section` cuando la lista muestra resultados de búsqueda o una categoría del catálogo.
const SEARCH: usize = usize::MAX;
const BROWSE: usize = usize::MAX - 1;
const LIBRARY: usize = usize::MAX - 2;
const FAVORITES: usize = usize::MAX - 3;
const PLAYLISTS: usize = usize::MAX - 4;
const DONATE: usize = usize::MAX - 5;
/// Canciones favoritas: la sección abre directo su lista, que se muestra como un álbum.
const SONGS: usize = usize::MAX - 6;
const SETTINGS: usize = usize::MAX - 7;
const FAV_SONGS: &str = "favorites:songs";

const DONATE_URL: &str = "https://downloads.khinsider.com/forums/index.php?account/upgrades";

/// Formas de recorrer el catálogo completo: se elige una opción y se abre su lista.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Letter,
    Platform,
    Type,
    Year,
}

impl Category {
    const ALL: [Category; 4] = [Category::Letter, Category::Platform, Category::Type, Category::Year];

    fn label(self) -> &'static str {
        match self {
            Category::Letter => tr("Por letra", "By Letter"),
            Category::Platform => tr("Por plataforma", "By Platform"),
            Category::Type => tr("Por tipo", "By Type"),
            Category::Year => tr("Por año", "By Year"),
        }
    }

    /// Letras y tipos son fijos; plataformas y años se leen de su página índice.
    fn options(self) -> std::result::Result<Vec<Filter>, &'static str> {
        let fixed = |path: String, name: &str| Filter { path, name: name.to_string(), count: None };
        match self {
            Category::Letter => {
                let letters = ('A'..='Z').map(|c| fixed(format!("/game-soundtracks/browse/{c}"), &c.to_string()));
                Ok(std::iter::once(fixed("/game-soundtracks/browse/0-9".into(), "#")).chain(letters).collect())
            }
            Category::Type => Ok([
                ("gamerips", "Gamerips"),
                ("ost", "Soundtracks"),
                ("singles", "Singles"),
                ("arrangements", "Arrangements"),
                ("remixes", "Remixes"),
                ("compilations", "Compilations"),
                ("inspired-by", "Inspired By"),
            ]
            .map(|(slug, name)| fixed(format!("/game-soundtracks/{slug}"), name))
            .into()),
            Category::Platform => Err("/console-list"),
            Category::Year => Err("/album-years"),
        }
    }
}

fn filter_label(f: &Filter) -> String {
    match f.count {
        Some(n) => format!("{}  ·  {n}", f.name),
        None => f.name.clone(),
    }
}

/// Resultados de las tareas de red; llegan al hilo de UI por un canal.
enum Msg {
    /// Generación de la lista que se pidió, página y resultado.
    List(u64, u32, Result<(Vec<Album>, bool)>),
    Random(Result<String>),
    Filters(Category, Result<Vec<Filter>>),
    Album(String, Result<AlbumDetail>),
    Image(String, Vec<u8>),
    /// Colores principales de una portada.
    Palette(String, Vec<Color32>),
    Audio(u64, Result<Audio>),
    /// Avance de una descarga (clave de `download_key`, pistas listas, total) y su resultado final.
    Download(String, usize, usize),
    Downloaded(String, Result<PathBuf>),
    /// Resultado de iniciar sesión con ese usuario.
    Login(String, Result<()>),
    /// Portada de un álbum +18 de una lista (slug y miniatura), sacada de la página del álbum.
    Thumb(String, Option<String>),
    /// Álbumes favoritos de la cuenta.
    SiteFavorites(Result<Vec<Album>>),
    /// Resultado de marcar o desmarcar un álbum en los favoritos de la cuenta.
    FavoriteToggled(Result<()>),
}

/// Estado de la sesión en el sitio.
#[derive(Clone, PartialEq)]
enum Session {
    Out,
    Busy,
    In(String),
    Failed(String),
}

/// Red en segundo plano: un runtime tokio chico y un cliente HTTP compartido.
struct Net {
    rt: tokio::runtime::Runtime,
    http: wreq::Client,
    tx: mpsc::Sender<Msg>,
    ctx: egui::Context,
    requested: HashSet<String>,
    loaded: HashSet<String>,
}

impl Net {
    fn spawn<F: Future<Output = Msg> + Send + 'static>(&self, f: impl FnOnce(wreq::Client) -> F) {
        let (tx, ctx, fut) = (self.tx.clone(), self.ctx.clone(), f(self.http.clone()));
        self.rt.spawn(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    /// Pinta la imagen si ya llegó; si no, la pide una sola vez y pinta un recuadro vacío.
    fn paint_image(&mut self, ui: &egui::Ui, url: &str, rect: Rect, radius: u8) {
        if self.loaded.contains(url) {
            egui::Image::new(format!("bytes://{url}")).corner_radius(radius).paint_at(ui, rect);
            return;
        }
        ui.painter().rect_filled(rect, radius, ui.visuals().faint_bg_color);
        if self.requested.insert(url.to_string()) {
            let url = url.to_string();
            self.spawn(|c| async move {
                let b = match url.strip_prefix(library::LOCAL) {
                    Some(path) => std::fs::read(path).unwrap_or_default(),
                    None => khinsider::bytes(&c, &url).await.unwrap_or_default(),
                };
                Msg::Image(url, b)
            });
        }
    }
}

/// Reproductor: el dispositivo de audio se abre recién en la primera reproducción para no frenar el arranque.
#[derive(Default)]
struct Playback {
    device: Option<(rodio::MixerDeviceSink, rodio::Player)>,
    queue: Arc<[Track]>,
    /// Título y portada del álbum de la cola, para la barra del reproductor.
    album: Option<(String, Option<String>, bool)>,
    index: usize,
    generation: u64,
    loading: bool,
    active: bool,
    drag: Option<f32>,
    /// Volumen entre 0 y 1, y el valor a recuperar al quitar el silencio.
    volume: f32,
    unmuted: f32,
    mode: Mode,
    /// Pistas ya escuchadas de la cola, para que "anterior" funcione también en aleatorio.
    history: Vec<usize>,
}

impl Playback {
    fn player(&mut self) -> &rodio::Player {
        let volume = self.volume;
        &self
            .device
            .get_or_insert_with(|| {
                let mut sink = rodio::DeviceSinkBuilder::open_default_sink().expect("dispositivo de audio");
                sink.log_on_drop(false);
                let player = rodio::Player::connect_new(sink.mixer());
                player.set_volume(volume);
                (sink, player)
            })
            .1
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some((_, p)) = &self.device {
            p.set_volume(self.volume);
        }
    }

    /// Siguiente pista según el modo; `None` cuando la cola terminó.
    fn following(&self) -> Option<usize> {
        let len = self.queue.len();
        match self.mode {
            Mode::Shuffle if len > 1 => {
                let r = random_below(len - 1);
                Some(if r >= self.index { r + 1 } else { r })
            }
            _ if self.index + 1 < len => Some(self.index + 1),
            Mode::Repeat | Mode::RepeatOne | Mode::Shuffle if len > 0 => Some(0),
            _ => None,
        }
    }

    fn current(&self) -> Option<&Track> {
        self.queue.get(self.index)
    }

    fn position(&self) -> f32 {
        self.device.as_ref().map_or(0.0, |(_, p)| p.get_pos().as_secs_f32())
    }

    fn paused(&self) -> bool {
        self.device.as_ref().is_none_or(|(_, p)| p.is_paused())
    }
}

struct App {
    net: Net,
    rx: mpsc::Receiver<Msg>,
    section: usize,
    /// Texto del buscador y búsqueda que se muestra en la lista.
    query: String,
    search: String,
    /// Categoría abierta, sus opciones ya cargadas y la opción elegida.
    category: Category,
    filters: HashMap<Category, Result<Arc<[Filter]>>>,
    filter: Option<Filter>,
    list_generation: u64,
    page: u32,
    has_more: bool,
    loading_list: bool,
    scroll_top: bool,
    albums: Option<Result<Arc<[Album]>>>,
    selected: Option<String>,
    detail: Option<Result<Arc<AlbumDetail>>>,
    playback: Playback,
    feed: visualizer::Feed,
    spectrum: visualizer::Spectrum,
    visual: visualizer::Style,
    /// Índice en `visualizer::PALETTES`.
    palette: usize,
    /// Vista del visualizador a pantalla completa, y última vez que se movió el mouse en ella.
    show_visual: bool,
    last_motion: f64,
    error: Option<String>,
    themes: Vec<Theme>,
    theme: usize,
    /// Con un álbum elegido, el área principal muestra su vista en vez de la cuadrícula.
    show_album: bool,
    /// Último top abierto (índice en `SECTIONS`), para volver a él desde la barra lateral.
    top: usize,
    download_dir: String,
    /// Formato de las descargas; `None` pregunta cada vez que el álbum tiene FLAC.
    download_format: Option<Format>,
    /// Descarga que espera a que se elija el formato: el álbum y, si es una sola, la pista.
    format_prompt: Option<(Arc<AlbumDetail>, Option<usize>)>,
    /// Descargas por `download_key`: pistas listas y total. Al terminar quedan con ambos iguales.
    downloads: HashMap<String, (usize, usize)>,
    /// Mostrar las portadas +18 que el sitio tapa.
    show_nsfw: bool,
    /// Pide volver arriba en la vista del álbum: se activa al abrir uno.
    album_scroll_top: bool,
    /// Apaga las animaciones (transiciones, tarjetas escalonadas y cursor del menú).
    reduce_motion: bool,
    /// Vista que se muestra (sección, álbum abierto y cuál) y desde cuándo, para la transición.
    view: (usize, bool, Option<String>),
    view_since: f64,
    /// Canciones favoritas, y álbumes favoritos marcados sin sesión.
    favorites: favorites::Favorites,
    /// Álbumes favoritos de la cuenta; `None` sin sesión o mientras se cargan.
    site_favorites: Option<Vec<Album>>,
    /// Colores principales de cada portada, para el degradado del álbum. Vacío mientras se calcula o
    /// si no se pudo.
    palettes: HashMap<String, Vec<Color32>>,
    /// Miniaturas de álbumes +18 de las listas, por slug. `None` mientras se busca o si no hay.
    nsfw_thumbs: HashMap<String, Option<String>>,
    /// Qué pistas del álbum abierto ya están en la carpeta de descargas, por índice. `None` pide
    /// revisar el disco otra vez: al abrir otro álbum, al bajar una pista o al cambiar de carpeta.
    on_disk: Option<Vec<bool>>,
    /// Formulario de login. La contraseña se borra de memoria apenas se guarda en disco.
    user: String,
    password: String,
    session: Session,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        i18n::init();
        let themes = theme::load_all();
        let saved = theme::saved();
        let theme = themes.iter().position(|t| Some(&t.name) == saved.as_ref()).unwrap_or(0);
        themes[theme].apply(&cc.egui_ctx);
        let volume = settings::get("volume").and_then(|v| v.parse().ok()).unwrap_or(0.8f32).clamp(0.0, 1.0);
        let (tx, rx) = mpsc::channel();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
        let net = Net { rt, http: khinsider::client(), tx, ctx: cc.egui_ctx.clone(), requested: HashSet::new(), loaded: HashSet::new() };
        let mut app = Self {
            net,
            rx,
            section: 0,
            query: String::new(),
            search: String::new(),
            category: Category::Letter,
            filters: HashMap::new(),
            filter: None,
            list_generation: 0,
            page: 1,
            has_more: false,
            loading_list: false,
            scroll_top: false,
            albums: None,
            selected: None,
            detail: None,
            playback: Playback {
                volume,
                unmuted: volume.max(0.1),
                mode: Mode::from_key(&settings::get("mode").unwrap_or_default()),
                ..Default::default()
            },
            feed: Default::default(),
            spectrum: Default::default(),
            visual: visualizer::Style::from_key(&settings::get("visualizer").unwrap_or_default()),
            palette: visualizer::Palette::from_key(&settings::get("visualizer-palette").unwrap_or_default()),
            show_visual: false,
            last_motion: 0.0,
            error: None,
            themes,
            theme,
            show_album: false,
            top: 1,
            download_dir: settings::get("download-dir").unwrap_or_else(default_download_dir),
            download_format: FORMATS.into_iter().find(|f| settings::get("download-format").as_deref() == Some(format_key(*f))).flatten(),
            format_prompt: None,
            downloads: HashMap::new(),
            on_disk: None,
            show_nsfw: settings::get("show-nsfw").as_deref() == Some("1"),
            album_scroll_top: false,
            reduce_motion: settings::get("reduce-motion").as_deref() == Some("1"),
            view: (0, false, None),
            view_since: 0.0,
            favorites: favorites::Favorites::load(),
            site_favorites: None,
            nsfw_thumbs: HashMap::new(),
            palettes: HashMap::new(),
            user: settings::get("user").unwrap_or_default(),
            password: String::new(),
            session: Session::Out,
        };
        if let Some((user, password)) = account::saved() {
            app.login(user, password);
        }
        app.open_section(0);
        app
    }

    fn open_section(&mut self, section: usize) {
        self.section = section;
        self.show_album = false;
        if (1..SECTIONS.len()).contains(&section) {
            self.top = section;
        }
        self.list_generation += 1;
        self.page = 1;
        self.has_more = false;
        self.scroll_top = true;
        self.albums = None;
        self.fetch_page();
    }

    /// Abre una categoría del catálogo con su primera opción elegida; si las opciones todavía se
    /// están descargando, la primera se elige al llegar.
    fn open_category(&mut self, category: Category) {
        self.category = category;
        self.filter = None;
        self.open_section(BROWSE);
        if !self.filters.contains_key(&category) {
            match category.options() {
                Ok(options) => _ = self.filters.insert(category, Ok(options.into())),
                Err(path) => self.net.spawn(move |c| async move { Msg::Filters(category, khinsider::filters(&c, path).await) }),
            }
        }
        self.choose_first();
    }

    fn choose_first(&mut self) {
        if let Some(Ok(options)) = self.filters.get(&self.category)
            && let Some(first) = options.first().cloned()
        {
            self.choose_filter(first);
        }
    }

    fn choose_filter(&mut self, filter: Filter) {
        self.filter = Some(filter);
        self.open_section(BROWSE);
    }

    /// Abre la sección, o vuelve a su cuadrícula si ya estaba abierta con un álbum a la vista.
    fn go(&mut self, section: usize) {
        // Canciones favoritas no tiene cuadrícula: siempre muestra su lista.
        if self.section == section && section != SONGS {
            self.show_album = false;
        } else {
            self.open_section(section);
        }
    }

    fn fetch_page(&mut self) {
        if matches!(self.section, DONATE | SETTINGS) {
            return;
        }
        if self.section == LIBRARY {
            // La biblioteca se lee del disco al momento: son pocas carpetas.
            self.loading_list = false;
            self.albums = Some(Ok(library::albums(Path::new(&self.download_dir)).into()));
            return;
        }
        if self.section == SONGS {
            self.loading_list = false;
            self.albums = Some(Ok(Vec::new().into()));
            self.select(FAV_SONGS);
            return;
        }
        if self.section == FAVORITES {
            // Los favoritos locales ya están en memoria y los de la cuenta se cargan al iniciar sesión.
            self.loading_list = false;
            self.albums = Some(Ok(self.favorite_albums().into()));
            return;
        }
        let path = match (self.section, &self.filter) {
            (SEARCH, _) => khinsider::search_path(&self.search),
            (BROWSE, Some(f)) => f.path.clone(),
            (BROWSE, None) => return,
            (LIBRARY | DONATE | FAVORITES | SONGS | SETTINGS, _) => unreachable!(),
            (PLAYLISTS, _) => "/playlist/browse".to_string(),
            (s, _) => SECTIONS[s].2.to_string(),
        };
        self.loading_list = true;
        let (generation, page) = (self.list_generation, self.page);
        self.net.spawn(move |c| async move { Msg::List(generation, page, khinsider::list(&c, &path, page).await) });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::List(generation, page, r) if generation == self.list_generation => {
                self.loading_list = false;
                match r {
                    Ok((mut albums, more)) => {
                        self.has_more = more;
                        if let (true, Some(Ok(prev))) = (page > 1, &self.albums) {
                            albums.splice(0..0, prev.iter().cloned());
                        }
                        self.albums = Some(Ok(albums.into()));
                    }
                    Err(e) if page == 1 => self.albums = Some(Err(e)),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::List(..) => {}
            Msg::Random(Ok(slug)) => self.select(&slug),
            Msg::Random(Err(e)) => self.error = Some(e),
            Msg::Filters(category, r) => {
                self.filters.insert(category, r.map(Into::into));
                if self.section == BROWSE && self.category == category && self.filter.is_none() {
                    self.choose_first();
                }
            }
            Msg::Album(slug, r) if self.selected.as_ref() == Some(&slug) => {
                self.detail = Some(r.map(Arc::new));
                self.on_disk = None;
            }
            Msg::Album(..) => {}
            Msg::Thumb(slug, thumb) => _ = self.nsfw_thumbs.insert(slug, thumb),
            Msg::SiteFavorites(r) => {
                match r {
                    Ok(albums) => self.site_favorites = Some(albums),
                    Err(e) => self.error = Some(format!("{}: {e}", tr("Favoritos de la cuenta", "Account favorites"))),
                }
                if self.section == FAVORITES {
                    self.fetch_page();
                }
            }
            Msg::FavoriteToggled(Ok(())) => {}
            Msg::FavoriteToggled(Err(e)) => {
                // La lista en memoria ya cambió: se vuelve a leer la de la cuenta para no quedar desfasada.
                self.error = Some(format!("{}: {e}", tr("No se pudo cambiar el favorito", "Could not change the favorite")));
                self.load_site_favorites();
            }
            Msg::Palette(url, colors) => _ = self.palettes.insert(url, colors),
            Msg::Image(url, bytes) => {
                self.net.ctx.include_bytes(format!("bytes://{url}"), bytes);
                self.net.loaded.insert(url);
            }
            Msg::Audio(generation, r) if generation == self.playback.generation => {
                self.playback.loading = false;
                match r {
                    Ok(source) => {
                        let player = self.playback.player();
                        player.append(source);
                        player.play();
                        self.playback.active = true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Audio(..) => {}
            Msg::Download(key, done, total) => {
                self.downloads.insert(key, (done, total));
                self.on_disk = None;
            }
            Msg::Downloaded(_, Ok(_)) if self.section == LIBRARY => self.open_section(LIBRARY),
            Msg::Downloaded(_, Ok(_)) => {}
            Msg::Login(user, Ok(())) => {
                // Solo se guarda lo que se escribió en el formulario; el login automático ya viene guardado.
                let typed = std::mem::take(&mut self.password);
                if !typed.is_empty()
                    && let Err(e) = account::save(&user, &typed)
                {
                    self.error = Some(format!("{}: {e}", tr("No se pudo guardar la contraseña", "Could not save the password")));
                }
                self.session = Session::In(user);
                self.load_site_favorites();
            }
            Msg::Login(_, Err(e)) => self.session = Session::Failed(e),
            Msg::Downloaded(title, Err(e)) => {
                self.downloads.remove(&title);
                self.error = Some(e);
            }
        }
    }

    fn select(&mut self, slug: &str) {
        self.selected = Some(slug.to_string());
        self.album_scroll_top = true;
        self.show_album = true;
        self.detail = None;
        self.on_disk = None;
        if slug == FAV_SONGS {
            self.detail = Some(Ok(Arc::new(self.favorite_songs())));
            return;
        }
        let slug = slug.to_string();
        self.net.spawn(|c| async move {
            let r = match slug.strip_prefix(library::LOCAL) {
                Some(dir) => library::album(Path::new(dir)),
                None if slug.starts_with("/playlist/") => khinsider::playlist(&c, &slug).await,
                None => khinsider::album(&c, &slug).await,
            };
            Msg::Album(slug, r)
        });
    }

    fn play(&mut self, queue: Arc<[Track]>, index: usize) {
        self.playback.queue = queue;
        self.playback.history.clear();
        self.load(index);
    }

    fn load(&mut self, index: usize) {
        let pb = &mut self.playback;
        pb.index = index;
        pb.generation += 1;
        pb.loading = true;
        pb.active = false;
        pb.player().clear();
        self.error = None;
        let (generation, track, feed) = (pb.generation, pb.queue[index].clone(), self.feed.clone());
        // ponytail: descarga la pista completa antes de sonar (unos cientos de ms);
        // pasar a streaming con el crate stream-download si las pistas largas tardan demasiado.
        self.net.spawn(|c| async move {
            let bytes = match track.path.strip_prefix(library::LOCAL) {
                Some(path) => std::fs::read(path).map_err(|e| format!("{path}: {e}")),
                None => khinsider::audio(&c, &track).await,
            };
            let decoded = bytes.and_then(|b| rodio::Decoder::try_from(Cursor::new(b)).map_err(|e| e.to_string()));
            let r = decoded.map(|d| visualizer::Tap::new(d, feed));
            Msg::Audio(generation, r)
        });
    }

    fn load_site_favorites(&mut self) {
        self.net.spawn(|c| async move { Msg::SiteFavorites(khinsider::favorites(&c).await) });
    }

    /// Álbumes favoritos: los guardados en local y los de la cuenta.
    fn favorite_albums(&self) -> Vec<Album> {
        let site = self.site_favorites.iter().flatten().filter(|a| !self.favorites.has_album(&a.slug)).cloned();
        self.favorites.albums.iter().cloned().chain(site).collect()
    }

    fn favorite_songs(&self) -> AlbumDetail {
        AlbumDetail {
            title: tr("Canciones favoritas", "Favorite tracks").into(),
            cover: None,
            tracks: self.favorites.songs.iter().map(|s| s.track.clone()).collect(),
            // Vienen de álbumes distintos, como en una playlist.
            flac: true,
            nsfw: false,
            id: None,
        }
    }

    fn is_favorite_album(&self, slug: &str) -> bool {
        self.favorites.has_album(slug) || self.site_favorites.iter().flatten().any(|a| a.slug == slug)
    }

    /// Marca o desmarca el álbum. Con sesión va a los favoritos de la cuenta; sin sesión, a los locales.
    /// Al desmarcar se quita de los dos lados, por si quedó marcado en local antes de iniciar sesión.
    fn toggle_favorite_album(&mut self, slug: &str, detail: &AlbumDetail) {
        let album = Album { slug: slug.into(), name: detail.title.clone(), thumb: detail.cover.clone(), nsfw: detail.nsfw };
        let local = self.favorites.has_album(slug);
        let site = self.site_favorites.iter().flatten().any(|a| a.slug == slug);
        if local {
            self.favorites.toggle_album(album.clone());
        }
        match (&mut self.site_favorites, &detail.id) {
            (Some(list), Some(id)) if site => {
                list.retain(|a| a.slug != slug);
                self.toggle_site_favorite(id.clone());
            }
            (Some(list), Some(id)) if !local && matches!(self.session, Session::In(_)) => {
                list.push(album);
                self.toggle_site_favorite(id.clone());
            }
            _ if !local && !site => self.favorites.toggle_album(album),
            _ => {}
        }
        if self.section == FAVORITES {
            self.fetch_page();
        }
    }

    fn toggle_site_favorite(&mut self, id: String) {
        self.net.spawn(|c| async move { Msg::FavoriteToggled(khinsider::toggle_favorite(&c, &id).await) });
    }

    /// Marca o desmarca la pista `i` del álbum abierto como canción favorita.
    fn toggle_favorite_song(&mut self, detail: &AlbumDetail, i: usize) {
        let track = detail.tracks[i].clone();
        // En la lista de canciones favoritas cada una conserva el álbum con el que se guardó.
        let saved = self.favorites.songs.iter().find(|s| s.track.path == track.path).cloned();
        let song = saved.unwrap_or(favorites::Song { track, album: detail.title.clone(), cover: detail.cover.clone() });
        self.favorites.toggle_song(song);
        if self.selected.as_deref() == Some(FAV_SONGS) {
            self.detail = Some(Ok(Arc::new(self.favorite_songs())));
            self.on_disk = None;
        }
    }

    /// Pinta una portada. Mientras la opción está apagada, las +18 se ven como un recuadro con «+18».
    fn paint_cover(&mut self, ui: &egui::Ui, url: Option<&str>, nsfw: bool, rect: Rect, radius: u8) {
        match url {
            Some(url) if !nsfw || self.show_nsfw => self.net.paint_image(ui, url, rect, radius),
            _ => {
                let t = self.theme();
                ui.painter().rect_filled(rect, radius, t.surface);
                if nsfw {
                    let size = (rect.height() / 6.0).clamp(10.0, 32.0);
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "+18", semibold(size), t.muted);
                }
            }
        }
    }

    /// Colores principales de la portada. La primera vez la baja y los calcula en segundo plano.
    fn cover_palette(&mut self, url: &str) -> Option<Vec<Color32>> {
        if let Some(colors) = self.palettes.get(url) {
            return (!colors.is_empty()).then(|| colors.clone());
        }
        self.palettes.insert(url.to_string(), Vec::new());
        let url = url.to_string();
        self.net.spawn(|c| async move {
            let bytes = match url.strip_prefix(library::LOCAL) {
                Some(path) => std::fs::read(path).unwrap_or_default(),
                None => khinsider::bytes(&c, &url).await.unwrap_or_default(),
            };
            Msg::Palette(url, palette::dominant(&bytes))
        });
        None
    }

    /// Miniatura de un álbum +18 de una lista. La lista no la trae: se saca de la página del álbum,
    /// una sola vez por álbum.
    fn nsfw_thumb(&mut self, slug: &str) -> Option<String> {
        if let Some(thumb) = self.nsfw_thumbs.get(slug) {
            return thumb.clone();
        }
        self.nsfw_thumbs.insert(slug.to_string(), None);
        let slug = slug.to_string();
        self.net.spawn(|c| async move {
            let thumb = khinsider::album(&c, &slug).await.ok().and_then(|d| d.cover);
            Msg::Thumb(slug, thumb)
        });
        None
    }

    /// Descarga el álbum completo o una de sus pistas en el formato por defecto. Si el álbum tiene
    /// FLAC y la opción es «Preguntar», primero abre el diálogo para elegir el formato.
    fn request_download(&mut self, album: Arc<AlbumDetail>, track: Option<usize>) {
        match (album.flac, self.download_format) {
            (false, _) => self.download(album, track, Format::Mp3),
            (true, Some(format)) => self.download(album, track, format),
            (true, None) => self.format_prompt = Some((album, track)),
        }
    }

    /// Descarga las pistas del álbum (o solo `only`) en `<carpeta de descargas>/<título>/`. Las
    /// pistas que ya están en ese formato se saltan, así que repetir la descarga retoma una que se
    /// cortó. Si una pista no está en FLAC, se baja en MP3.
    fn download(&mut self, album: Arc<AlbumDetail>, only: Option<usize>, format: Format) {
        let dir = PathBuf::from(&self.download_dir).join(file_name(&album.title));
        let key = download_key(&album, only);
        let picked: Vec<usize> = match only {
            Some(i) => vec![i],
            None => (0..album.tracks.len()).collect(),
        };
        let total = picked.len();
        self.downloads.insert(key.clone(), (0, total));
        let (tx, ctx, http) = (self.net.tx.clone(), self.net.ctx.clone(), self.net.http.clone());
        self.net.rt.spawn(async move {
            let send = |msg| {
                let _ = tx.send(msg);
                ctx.request_repaint();
            };
            let result: Result<PathBuf> = async {
                std::fs::create_dir_all(&dir).map_err(io(&dir))?;
                if let Some(url) = &album.cover {
                    let ext = url.rsplit('.').next().filter(|e| ["jpg", "jpeg", "png", "webp", "gif"].contains(e)).unwrap_or("jpg");
                    let path = dir.join(format!("cover.{ext}"));
                    if !path.exists() {
                        std::fs::write(&path, khinsider::bytes(&http, url).await?).map_err(io(&path))?;
                    }
                }
                if album.nsfw {
                    // Marca para que la biblioteca también tape la portada.
                    let mark = dir.join(library::NSFW_MARK);
                    std::fs::write(&mark, "").map_err(io(&mark))?;
                }
                // Al listar las pistas del disco se prefiere el formato pedido si están las dos.
                let order = match format {
                    Format::Mp3 => [Format::Mp3, Format::Flac],
                    Format::Flac => [Format::Flac, Format::Mp3],
                };
                for (n, &i) in picked.iter().enumerate() {
                    let track = &album.tracks[i];
                    let base = track_file(i, track);
                    if !dir.join(format!("{base}.{}", format.ext())).exists() {
                        let (url, got) = khinsider::file_url(&http, track, format).await?;
                        let path = dir.join(format!("{base}.{}", got.ext()));
                        if !path.exists() {
                            // Se escribe aparte y se renombra al final, así un corte no deja un archivo a medias.
                            let part = path.with_extension(format!("{}.part", got.ext()));
                            let bytes = khinsider::bytes(&http, &url).await?;
                            std::fs::write(&part, bytes).and_then(|_| std::fs::rename(&part, &path)).map_err(io(&path))?;
                        }
                    }
                    // Título, nombres y duraciones de las pistas que ya están en disco quedan en
                    // `album.tsv` para la biblioteca local.
                    let files: Vec<_> = album.tracks.iter().enumerate().filter_map(|(i, t)| Some((saved_file(&dir, i, t, order)?, t))).collect();
                    library::save(&dir, &album.title, &files).map_err(io(&dir))?;
                    send(Msg::Download(key.clone(), n + 1, total));
                }
                Ok(dir)
            }
            .await;
            send(Msg::Downloaded(key, result));
        });
    }

    /// Diálogo para elegir MP3 o FLAC antes de una descarga.
    fn format_dialog(&mut self, ctx: &egui::Context) {
        let Some((album, track)) = self.format_prompt.clone() else { return };
        let t = self.theme().clone();
        let mut chosen = None;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("format")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new(tr("¿En qué formato?", "Which format?")).font(semibold(18.0)));
            ui.add_space(6.0);
            let what = match track {
                Some(i) => &album.tracks[i].name,
                None => &album.title,
            };
            ui.label(RichText::new(what).color(t.muted));
            ui.add_space(6.0);
            ui.label(tr(
                "FLAC no pierde calidad, pero ocupa unas tres veces más que MP3. Las pistas que no están en FLAC se bajan en MP3.",
                "FLAC is lossless, but takes about three times the space of MP3. Tracks without FLAC are saved as MP3.",
            ));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                for format in [Format::Mp3, Format::Flac] {
                    if ui.button(format.ext().to_uppercase()).clicked() {
                        chosen = Some(format);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    cancel = ui.button(tr("Cancelar", "Cancel")).clicked();
                });
            });
            ui.add_space(8.0);
            ui.label(RichText::new(tr("El formato por defecto se elige en Opciones.", "The default format is set in Settings.")).small().color(t.muted));
        });
        if let Some(format) = chosen {
            self.format_prompt = None;
            self.download(album, track, format);
        } else if cancel || modal.should_close() {
            self.format_prompt = None;
        }
    }

    fn next(&mut self) {
        match self.playback.following() {
            Some(i) => {
                self.playback.history.push(self.playback.index);
                self.load(i);
            }
            None => {
                if let Some((_, p)) = &self.playback.device {
                    p.clear();
                }
                self.playback.active = false;
            }
        }
    }

    fn previous(&mut self) {
        if self.playback.position() > 3.0 {
            self.seek(0.0);
        } else if let Some(i) = self.playback.history.pop() {
            self.load(i);
        } else if self.playback.index > 0 {
            self.load(self.playback.index - 1);
        } else {
            self.seek(0.0);
        }
    }

    fn toggle(&mut self) {
        if let (true, Some((_, p))) = (self.playback.active, &self.playback.device) {
            if p.is_paused() { p.play() } else { p.pause() }
        }
    }

    fn seek(&mut self, secs: f32) {
        if let (true, Some((_, p))) = (self.playback.active, &self.playback.device) {
            let _ = p.try_seek(Duration::from_secs_f32(secs));
        }
    }

    fn login(&mut self, user: String, password: String) {
        self.session = Session::Busy;
        self.net.spawn(|c| async move {
            let r = khinsider::login(&c, &user, &password).await;
            Msg::Login(user, r)
        });
    }

    /// Borra la contraseña guardada y descarta las cookies con un cliente HTTP nuevo.
    fn logout(&mut self) {
        account::forget();
        self.net.http = khinsider::client();
        self.session = Session::Out;
        self.site_favorites = None;
        match self.section {
            PLAYLISTS => self.open_section(0),
            FAVORITES => self.fetch_page(),
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.playback.mode = mode;
        settings::set("mode", mode.key());
    }

    fn theme(&self) -> &Theme {
        &self.themes[self.theme]
    }

    fn set_theme(&mut self, index: usize) {
        self.theme = index;
        self.themes[index].apply(&self.net.ctx);
        theme::save(&self.themes[index].name);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add(egui::Image::new(egui::include_image!("../assets/logo-mark.svg")).fit_to_exact_size(vec2(26.0, 34.0)).tint(t.accent));
            ui.add_space(4.0);
            ui.label(RichText::new("KHI-UI").font(semibold(22.0)).color(t.text));
        });
        ui.add_space(28.0);
        // En ventanas bajas la navegación se desplaza en vez de encimarse con Donar y Opciones.
        let nav_height = (ui.available_height() - 110.0).max(80.0);
        // El resaltado de la entrada activa se pinta en este lugar reservado, debajo de los textos, y
        // se desliza hasta la entrada nueva al cambiar de sección.
        let cursor_slot = ui.painter().add(egui::Shape::Noop);
        let mut cursor = None;
        egui::ScrollArea::vertical().max_height(nav_height).show(ui, |ui| {
            let mut mine = vec![
                (0, "🏠", tr("Inicio", "Home")),
                (LIBRARY, "🎵", tr("Biblioteca", "Library")),
                (SONGS, "♥", tr("Canciones favoritas", "Favorite Tracks")),
                // Los dos son favoritos (el ♥ de los botones); el ícono dice qué guarda cada lista.
                (FAVORITES, "💿", tr("Álbumes favoritos", "Favorite Albums")),
            ];
            if matches!(self.session, Session::In(_)) {
                mine.push((PLAYLISTS, "☰", tr("Mis playlists", "My Playlists")));
            }
            for (section, icon, title) in mine {
                if nav_item(ui, &t, icon, title, self.section == section, &mut cursor).clicked() {
                    self.go(section);
                }
            }
            divider(ui, &t);
            ui.label(RichText::new(tr("EXPLORAR", "EXPLORE")).small().color(t.muted));
            ui.add_space(4.0);
            if nav_item(ui, &t, "🏆", "Tops", (1..SECTIONS.len()).contains(&self.section), &mut cursor).clicked() {
                self.go(self.top);
            }
            if nav_item(ui, &t, "📚", tr("Catálogo", "Catalog"), self.section == BROWSE, &mut cursor).clicked() {
                match self.section {
                    BROWSE => self.show_album = false,
                    _ => self.open_category(self.category),
                }
            }
            if nav_item(ui, &t, "🎲", tr("Álbum al azar", "Random Album"), false, &mut cursor).clicked() {
                self.net.spawn(|c| async move { Msg::Random(khinsider::random_album(&c).await) });
            }
        });
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.add_space(4.0);
            if nav_item(ui, &t, "⚙", tr("Opciones", "Settings"), self.section == SETTINGS, &mut cursor).clicked() {
                self.go(SETTINGS);
            }
            if nav_item(ui, &t, "💝", tr("Donar", "Donate"), self.section == DONATE, &mut cursor).clicked() {
                self.go(DONATE);
            }
            divider(ui, &t);
        });
        if let Some(target) = cursor {
            let secs = if self.reduce_motion { 0.0 } else { 0.22 };
            let top = ui.ctx().animate_value_with_time(egui::Id::new("nav-cursor"), target.top(), secs);
            let rect = Rect::from_min_size(egui::pos2(target.left(), top), target.size());
            ui.painter().set(cursor_slot, egui::epaint::RectShape::filled(rect, 8, t.tint()));
        }
    }

    /// Pantalla de opciones: tema, idioma, cuenta, contenido y descargas. Cada cambio se guarda al momento.
    fn settings_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Un ancho acotado: campos de texto de lado a lado se leen mal en ventanas anchas.
            ui.set_max_width(520.0);
            let heading = |ui: &mut egui::Ui, text| {
                ui.add_space(22.0);
                ui.label(RichText::new(text).small().color(t.muted));
                ui.add_space(2.0);
            };

            heading(ui, tr("TEMA", "THEME"));
            let mut chosen = self.theme;
            egui::ComboBox::from_id_salt("theme").width(ui.available_width()).selected_text(t.label()).show_ui(ui, |ui| {
                for (i, theme) in self.themes.iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, theme.label());
                }
            });
            if chosen != self.theme {
                self.set_theme(chosen);
            }

            heading(ui, tr("IDIOMA", "LANGUAGE"));
            ui.horizontal(|ui| {
                for lang in Lang::ALL {
                    if ui.selectable_label(Lang::current() == lang, lang.label()).clicked() {
                        lang.set();
                    }
                }
            });

            heading(ui, tr("CUENTA DE KHINSIDER", "KHINSIDER ACCOUNT"));
            match self.session.clone() {
                Session::In(user) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} {user}", tr("Sesión iniciada como", "Logged in as")));
                        if ui.button(tr("Cerrar sesión", "Log out")).clicked() {
                            self.logout();
                        }
                    });
                }
                session => {
                    let busy = session == Session::Busy;
                    let user = egui::TextEdit::singleline(&mut self.user).hint_text(tr("Usuario o correo", "User name or email"));
                    ui.add_enabled(!busy, user.desired_width(ui.available_width()).margin(vec2(10.0, 6.0)));
                    let password = egui::TextEdit::singleline(&mut self.password).password(true).hint_text(tr("Contraseña", "Password"));
                    let enter = ui.add_enabled(!busy, password.desired_width(ui.available_width()).margin(vec2(10.0, 6.0)));
                    let submit = enter.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        let ready = !busy && !self.user.trim().is_empty() && !self.password.is_empty();
                        if ui.add_enabled(ready, egui::Button::new(tr("Iniciar sesión", "Log in"))).clicked() || (submit && ready) {
                            self.login(self.user.trim().to_string(), self.password.clone());
                        }
                        match &session {
                            Session::Busy => _ = ui.spinner(),
                            Session::Failed(e) => _ = ui.colored_label(ui.visuals().error_fg_color, e),
                            _ => {}
                        }
                    });
                    ui.label(
                        RichText::new(tr(
                            "La contraseña se guarda en texto plano en ~/.config/khi-ui/password.",
                            "The password is stored in plain text at ~/.config/khi-ui/password.",
                        ))
                        .small()
                        .color(t.muted),
                    );
                }
            }

            heading(ui, tr("CONTENIDO", "CONTENT"));
            if ui.checkbox(&mut self.show_nsfw, tr("Mostrar portadas +18", "Show 18+ covers")).changed() {
                settings::set("show-nsfw", if self.show_nsfw { "1" } else { "0" });
            }
            if ui.checkbox(&mut self.reduce_motion, tr("Reducir animaciones", "Reduce motion")).changed() {
                settings::set("reduce-motion", if self.reduce_motion { "1" } else { "0" });
            }

            heading(ui, tr("FORMATO DE DESCARGA", "DOWNLOAD FORMAT"));
            let mut format = self.download_format;
            egui::ComboBox::from_id_salt("download-format").width(ui.available_width()).selected_text(format_label(format)).show_ui(ui, |ui| {
                for option in FORMATS {
                    ui.selectable_value(&mut format, option, format_label(option));
                }
            });
            if format != self.download_format {
                self.download_format = format;
                settings::set("download-format", format_key(format));
            }

            heading(ui, tr("CARPETA DE DESCARGAS", "DOWNLOAD FOLDER"));
            let edit = egui::TextEdit::singleline(&mut self.download_dir).desired_width(ui.available_width()).margin(vec2(10.0, 6.0));
            let mut changed = ui.add(edit).changed();
            ui.horizontal(|ui| {
                if ui.button(tr("Elegir…", "Choose…")).clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&self.download_dir).pick_folder()
                {
                    self.download_dir = dir.display().to_string();
                    changed = true;
                }
                if ui.button(tr("Abrir", "Open")).clicked() {
                    open_folder(Path::new(&self.download_dir));
                }
            });
            if changed {
                settings::set("download-dir", &self.download_dir);
                self.on_disk = None;
            }
            ui.label(
                RichText::new(tr("Cada álbum se guarda en una subcarpeta con su nombre.", "Each album is saved in a subfolder with its name."))
                    .small()
                    .color(t.muted),
            );
            ui.add_space(12.0);
        });
    }

    /// Avance de una animación de `secs` segundos después de `elapsed`, de 0 a 1 con salida suave.
    /// Con «Reducir animaciones» siempre está terminada.
    fn progress(&self, elapsed: f64, secs: f64) -> f32 {
        if self.reduce_motion {
            return 1.0;
        }
        let x = (elapsed / secs).clamp(0.0, 1.0) as f32;
        1.0 - (1.0 - x).powi(3)
    }

    /// Qué decir cuando la lista abierta está vacía: por qué, y cómo llenarla si se puede.
    fn empty_message(&self) -> String {
        match self.section {
            LIBRARY => tr(
                "Todavía no hay álbumes descargados. Abre un álbum y usa «Descargar» para escucharlo sin conexión.",
                "No downloaded albums yet. Open an album and use «Download» to listen offline.",
            )
            .into(),
            FAVORITES => tr(
                "Todavía no hay álbumes favoritos. Abre un álbum y marca su ♥ para guardarlo acá.",
                "No favorite albums yet. Open an album and mark its ♥ to keep it here.",
            )
            .into(),
            SEARCH => format!("{} “{}”.", tr("No hay álbumes que coincidan con", "No albums match"), self.search),
            _ => tr("Esta lista está vacía en KHInsider.", "This list is empty on KHInsider.").into(),
        }
    }

    fn section_title(&self) -> String {
        match self.section {
            SEARCH => format!("“{}”", self.search),
            LIBRARY => tr("Biblioteca", "Library").into(),
            FAVORITES => tr("Álbumes favoritos", "Favorite Albums").into(),
            SONGS => tr("Canciones favoritas", "Favorite Tracks").into(),
            PLAYLISTS => tr("Mis playlists", "My Playlists").into(),
            DONATE => tr("Apoya a KHInsider", "Support KHInsider").into(),
            SETTINGS => tr("Opciones", "Settings").into(),
            BROWSE => self.category.label().into(),
            s => tr(SECTIONS[s].0, SECTIONS[s].1).into(),
        }
    }

    /// Área principal: encabezado con título y buscador, y debajo la vista de la sección.
    fn main_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        ui.horizontal(|ui| {
            // En Inicio el título saluda; en el resto es el nombre de la sección.
            // En Inicio el título saluda; en el resto es el nombre de la sección.
            let title = match (&self.session, self.section) {
                (Session::In(user), 0) => format!("{}, {user}", tr("Hola", "Hello")),
                (_, 0) => tr("Hola", "Hello").into(),
                _ => self.section_title(),
            };
            let max = ui.available_width() * 0.55;
            dialog_box(ui, &t, None, |ui| {
                ui.set_max_width(max);
                ui.add(egui::Label::new(RichText::new(title).font(semibold(30.0))).truncate());
            });
            // Donar y Opciones no listan álbumes: buscar ahí no tiene sentido.
            if matches!(self.section, DONATE | SETTINGS) {
                return;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let search = egui::TextEdit::singleline(&mut self.query)
                    .hint_text(tr("🔍  Buscar álbumes", "🔍  Search albums"))
                    .desired_width(ui.available_width().min(380.0))
                    .margin(vec2(14.0, 10.0));
                if ui.add(search).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.query.trim().is_empty() {
                    self.search = self.query.trim().to_string();
                    self.open_section(SEARCH);
                }
            });
        });
        ui.add_space(18.0);
        // Transición al cambiar de vista (abrir un álbum, volver a la lista, otra sección): el
        // contenido entra con un fundido y subiendo 16 px.
        let view = (self.section, self.show_album, self.selected.clone());
        let now = ui.input(|i| i.time);
        if view != self.view {
            self.view = view;
            self.view_since = now;
        }
        let p = self.progress(now - self.view_since, 0.25);
        if p < 1.0 {
            ui.multiply_opacity(p);
            ui.add_space(16.0 * (1.0 - p));
            ui.ctx().request_repaint();
        }
        match self.section {
            DONATE => self.donate_view(ui),
            SETTINGS => self.settings_view(ui),
            _ if self.show_album && self.selected.is_some() => self.album_view(ui),
            _ => self.album_grid(ui),
        }
    }

    fn album_grid(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        let options = match self.section {
            BROWSE => self.filters.get(&self.category).cloned(),
            _ => None,
        };
        let mut chosen = None;
        let mut open = None;
        // Arriba y no centrada: los desplegables se dibujan desde el borde superior del espacio libre, y en
        // una fila centrada el primero la agranda y el siguiente queda más abajo.
        ui.horizontal_top(|ui| {
            // Subtítulo con lo que se está viendo y, en Tops y Catálogo, desplegables para cambiarlo.
            match self.section {
                0 => _ = ui.label(RichText::new(tr(SECTIONS[0].0, SECTIONS[0].1)).font(semibold(18.0))),
                s if (1..SECTIONS.len()).contains(&s) => {
                    egui::ComboBox::from_id_salt("top").selected_text(tr(SECTIONS[s].0, SECTIONS[s].1)).show_ui(ui, |ui| {
                        for (i, (es, en, _)) in SECTIONS.iter().enumerate().skip(1) {
                            if ui.selectable_label(i == s, tr(es, en)).clicked() {
                                open = Some(i);
                            }
                        }
                    });
                }
                BROWSE => {
                    let mut category = self.category;
                    egui::ComboBox::from_id_salt("category").selected_text(category.label()).show_ui(ui, |ui| {
                        for option in Category::ALL {
                            ui.selectable_value(&mut category, option, option.label());
                        }
                    });
                    if category != self.category {
                        self.open_category(category);
                    }
                    if let (Some(Ok(options)), Some(current)) = (&options, &self.filter) {
                        egui::ComboBox::from_id_salt("filter").selected_text(&current.name).height(420.0).show_ui(ui, |ui| {
                            for option in options.iter() {
                                if ui.selectable_label(option.path == current.path, filter_label(option)).clicked() {
                                    chosen = Some(option.clone());
                                }
                            }
                        });
                    }
                }
                _ => {}
            }
            // Sin álbumes el estado vacío ya lo explica; un «0» suelto no suma.
            if let Some(Ok(a)) = &self.albums
                && !a.is_empty()
            {
                let size = vec2(ui.available_width(), ui.min_rect().height());
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(a.len().to_string()).color(t.muted));
                });
            }
        });
        ui.add_space(12.0);
        if let Some(section) = open {
            return self.open_section(section);
        }
        if let Some(filter) = chosen {
            return self.choose_filter(filter);
        }
        // Mientras llegan las opciones de la categoría todavía no hay lista que mostrar.
        if self.section == BROWSE && self.filter.is_none() {
            match &options {
                Some(Err(e)) if error_state(ui, e) => self.open_category(self.category),
                Some(Err(_)) => {}
                _ => loading_state(ui, &t, tr("Cargando el catálogo…", "Loading the catalog…")),
            }
            return;
        }
        let albums = match self.albums.clone() {
            None => return loading_state(ui, &t, tr("Cargando álbumes…", "Loading albums…")),
            Some(Err(e)) => {
                if error_state(ui, &e) {
                    self.albums = None;
                    self.fetch_page();
                }
                return;
            }
            Some(Ok(a)) if a.is_empty() => return empty_state(ui, &t, &self.empty_message()),
            Some(Ok(a)) => a,
        };
        // Tarjetas de al menos 160 px que se estiran para llenar el ancho.
        let gap = 20.0;
        let width = ui.available_width() - 12.0;
        let cols = ((width + gap) / (160.0 + gap)).floor().max(1.0) as usize;
        let card = (width - gap * (cols - 1) as f32) / cols as f32;
        let row = card + 64.0;
        let mut clicked = None;
        let mut at_end = false;
        // Cada vista con su propio scroll: compartido, la posición de una lista pasaba a la otra.
        let mut area = egui::ScrollArea::vertical().id_salt("album-grid").auto_shrink(false);
        if std::mem::take(&mut self.scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        let rows = albums.len().div_ceil(cols);
        // Momento en que se mostró esta lista, para escalonar la entrada de las tarjetas.
        let now = ui.input(|i| i.time);
        let shown_at = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(egui::Id::new(("cards", self.list_generation)), || now));
        if self.progress(now - shown_at, 1.2) < 1.0 {
            ui.ctx().request_repaint();
        }
        area.show_rows(ui, row, rows, |ui, range| {
            at_end = range.end == rows;
            ui.spacing_mut().item_spacing = vec2(gap, 0.0);
            for r in range {
                ui.horizontal(|ui| {
                    let opacity = ui.opacity();
                    for (j, album) in albums.iter().skip(r * cols).take(cols).enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(card, row), Sense::click());
                        // Las tarjetas de una lista nueva aparecen una tras otra, con fundido y subiendo.
                        let index = r * cols + j;
                        let p = self.progress(now - shown_at - index.min(24) as f64 * 0.035, 0.3);
                        ui.set_opacity(opacity * p);
                        let cover = Rect::from_min_size(rect.min + vec2(0.0, 12.0 * (1.0 - p)), vec2(card, card));
                        // La lista trae miniaturas chicas; la versión `thumbs` se ve nítida en la tarjeta.
                        let thumb = match &album.thumb {
                            Some(url) => Some(url.replace("/thumbs_small/", "/thumbs/")),
                            None if album.nsfw && self.show_nsfw => self.nsfw_thumb(&album.slug),
                            None => None,
                        };
                        self.paint_cover(ui, thumb.as_deref(), album.nsfw, cover, 10);
                        let selected = self.selected.as_ref() == Some(&album.slug);
                        if resp.hovered() {
                            ui.painter().rect_filled(cover, 10, Color32::from_black_alpha(40));
                        }
                        focus_ring(ui, &t, &resp, cover, 10);
                        let color = if selected { t.accent } else { t.text };
                        let mut job = egui::text::LayoutJob::simple(album.name.clone(), FontId::proportional(14.0), color, card);
                        job.wrap.max_rows = 2;
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(cover.left_bottom() + vec2(0.0, 10.0), galley, color);
                        if resp.on_hover_text(&album.name).clicked() {
                            clicked = Some(album.slug.clone());
                        }
                    }
                    ui.set_opacity(opacity);
                });
            }
        });
        if let Some(slug) = clicked {
            self.select(&slug);
        }
        // Scroll infinito: al ver la última fila se pide la página siguiente (Top 1000).
        if at_end && self.has_more && !self.loading_list {
            self.page += 1;
            self.fetch_page();
        }
    }

    fn donate_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        ui.set_max_width(560.0);
        ui.label(
            RichText::new(tr(
                "KHInsider mantiene gratis este archivo de música de videojuegos, y cada álbum que escuchas en KHI-UI viene de ahí.",
                "KHInsider keeps this video game music archive free, and every album you play in KHI-UI comes from it.",
            ))
            .size(16.0),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(tr(
                "Considera donar al equipo de KHInsider. ¡Cada aporte ayuda!",
                "Please consider donating to the KHInsider team. Every bit helps!",
            ))
            .size(16.0),
        );
        ui.add_space(18.0);
        let button = egui::Button::new(RichText::new(tr("💝  Donar", "💝  Donate")).color(t.base).font(semibold(15.0)))
            .fill(t.accent)
            .corner_radius(20)
            .min_size(vec2(150.0, 40.0));
        let r = ui.add(button);
        focus_ring(ui, &t, &r, r.rect, 20);
        if r.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(DONATE_URL));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(DONATE_URL).small().color(t.muted));
    }

    fn album_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        // Canciones favoritas no tiene cuadrícula a la que volver.
        if self.section != SONGS {
            let back = egui::Button::new(RichText::new(format!("←  {}", self.section_title())).color(t.muted)).frame(false);
            if ui.add(back).clicked() {
                self.show_album = false;
                return;
            }
            ui.add_space(8.0);
        }
        let detail = match self.detail.clone() {
            None => return loading_state(ui, &t, tr("Cargando el álbum…", "Loading the album…")),
            Some(Err(e)) => {
                if error_state(ui, &e)
                    && let Some(slug) = self.selected.clone()
                {
                    self.select(&slug);
                }
                return;
            }
            Some(Ok(d)) => d,
        };
        let tracks: Arc<[Track]> = detail.tracks.clone().into();
        let folder = PathBuf::from(&self.download_dir).join(file_name(&detail.title));
        let on_disk = self
            .on_disk
            .get_or_insert_with(|| (0..tracks.len()).map(|i| saved_file(&folder, i, &tracks[i], [Format::Mp3, Format::Flac]).is_some()).collect())
            .clone();
        let total: f32 = tracks.iter().map(|t| khinsider::seconds(&t.duration)).sum();
        let mut play = None;
        let mut download = false;
        let mut download_track = None;
        let mut favorite_album = false;
        let mut favorite_track = None;
        let local = self.selected.as_ref().is_some_and(|s| s.starts_with(library::LOCAL));
        // Fondo del panel: degradado pixel art con los colores de la portada, mezclado con el fondo
        // del tema lo justo para que el texto pase 4.5:1. Sobre el degradado, los textos secundarios
        // usan el color de texto principal (en tamaño chico): con el gris, el fondo tendría que quedar
        // casi del color del tema y la portada no se notaría.
        let gradient = match &detail.cover {
            Some(cover) if !detail.nsfw || self.show_nsfw => self.cover_palette(cover),
            _ => None,
        }
        .map(|colors| palette::readable(&colors, t.mantle, &[t.text]));
        let secondary = if gradient.is_some() { t.text } else { t.muted };
        let mut area = egui::ScrollArea::vertical().id_salt("album-view").auto_shrink(false);
        if std::mem::take(&mut self.album_scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        area.show(ui, |ui| {
            dialog_box(ui, &t, gradient.as_deref(), |ui| {
                // La caja ocupa el ancho completo, alineada con la tabla de pistas de abajo.
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(200.0, 200.0), Sense::hover());
                    self.paint_cover(ui, detail.cover.as_deref(), detail.nsfw, rect, 12);
                    if self.selected.as_deref() == Some(FAV_SONGS) {
                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "♥", FontId::proportional(64.0), t.accent);
                    }
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.add_space(52.0);
                        let kind = match self.selected.as_deref() {
                            Some(FAV_SONGS) => tr("FAVORITOS", "FAVORITES"),
                            Some(s) if s.starts_with("/playlist/") => "PLAYLIST",
                            _ => tr("ÁLBUM", "ALBUM"),
                        };
                        ui.label(RichText::new(kind).small().color(secondary));
                        ui.add(egui::Label::new(RichText::new(&detail.title).heading()).wrap());
                        ui.label(RichText::new(format!("{} {} · {}", tracks.len(), if tracks.len() == 1 { tr("pista", "track") } else { tr("pistas", "tracks") }, long_clock(total))).color(secondary));
                        ui.add_space(10.0);
                        let button = egui::Button::new(RichText::new(tr("▶  Reproducir", "▶  Play")).color(t.base).font(semibold(14.0)))
                            .fill(t.accent)
                            .corner_radius(18)
                            .min_size(vec2(140.0, 36.0));
                        ui.horizontal(|ui| {
                            let r = ui.add_enabled(!tracks.is_empty(), button);
                            focus_ring(ui, &t, &r, r.rect, 18);
                            if r.clicked() {
                                play = Some(0);
                            }
                            if self.selected.as_ref().is_some_and(|s| s.starts_with(library::LOCAL)) {
                                return;
                            }
                            // Un álbum que ya está completo en disco cuenta como descargado aunque se haya
                            // bajado en otra sesión.
                            let complete = !on_disk.is_empty() && on_disk.iter().all(|&d| d);
                            let progress = self.downloads.get(&detail.title).copied().or(complete.then_some((1, 1)));
                            let text = match progress {
                                Some((done, total)) if done == total => tr("✔  Descargado", "✔  Downloaded").to_string(),
                                Some((done, total)) => format!("{} {done}/{total}", tr("Descargando", "Downloading")),
                                None => tr("⬇  Descargar", "⬇  Download").to_string(),
                            };
                            let button = egui::Button::new(RichText::new(text).font(semibold(14.0))).corner_radius(18).min_size(vec2(140.0, 36.0));
                            let busy = progress.is_some_and(|(done, total)| done < total);
                            let r = ui.add_enabled(!tracks.is_empty() && !busy, button);
                            let r = if progress.is_some() { r.on_hover_text(tr("Abrir la carpeta", "Open folder")) } else { r };
                            if r.clicked() {
                                download = true;
                            }
                            // Las playlists y las canciones favoritas no son álbumes del sitio.
                            let slug = self.selected.clone().unwrap_or_default();
                            if slug.starts_with("/playlist/") || slug == FAV_SONGS {
                                return;
                            }
                            let fav = self.is_favorite_album(&slug);
                            let heart = RichText::new("♥").size(18.0).color(if fav { t.accent } else { t.muted });
                            let r = ui.add(egui::Button::new(heart).corner_radius(18).min_size(vec2(36.0, 36.0)));
                            let tip = match fav {
                                true => tr("Quitar de favoritos", "Remove from favorites"),
                                false => tr("Agregar a favoritos", "Add to favorites"),
                            };
                            if r.on_hover_text(tip).clicked() {
                                favorite_album = true;
                            }
                        });
                    });
                })
            });
            ui.add_space(24.0);
            // Tabla de pistas: encabezado de columnas y una fila por pista.
            let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
            let p = ui.painter();
            let font = FontId::proportional(12.0);
            p.text(egui::pos2(head.left() + 30.0, head.center().y), Align2::RIGHT_CENTER, "#", font.clone(), t.muted);
            p.text(egui::pos2(head.left() + 50.0, head.center().y), Align2::LEFT_CENTER, tr("TÍTULO", "TITLE"), font.clone(), t.muted);
            p.text(egui::pos2(head.right() - 16.0, head.center().y), Align2::RIGHT_CENTER, tr("DURACIÓN", "DURATION"), font, t.muted);
            p.hline(head.x_range(), head.bottom(), egui::Stroke::new(1.0, t.surface));
            ui.add_space(6.0);
            if tracks.is_empty() && self.section == SONGS {
                ui.add_space(12.0);
                ui.label(RichText::new(tr("Marca canciones con ♥ para verlas acá.", "Mark tracks with ♥ to see them here.")).color(t.muted));
            }
            let current = self.playback.current().map(|t| t.path.as_str());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, track) in tracks.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                let playing = current == Some(track.path.as_str());
                // Botón para bajar solo esta pista. Se registra antes de pintar para saber si la fila
                // está bajo el mouse, y se pinta después del fondo para que no quede tapado.
                let icon = Rect::from_center_size(egui::pos2(rect.right() - 80.0, rect.center().y), vec2(28.0, 28.0));
                let icon_resp = (!local).then(|| ui.interact(icon, resp.id.with("download"), Sense::click()));
                let heart = icon.translate(vec2(-32.0, 0.0));
                let heart_resp = ui.interact(heart, resp.id.with("favorite"), Sense::click());
                let hovered = resp.hovered() || heart_resp.hovered() || icon_resp.as_ref().is_some_and(|r| r.hovered());
                if hovered || playing {
                    ui.painter().rect_filled(rect, 8, if playing { t.tint() } else { t.hover() });
                }
                focus_ring(ui, &t, &resp, rect, 8);
                focus_ring(ui, &t, &heart_resp, heart, 6);
                let fav = self.favorites.has_song(&track.path);
                let color = match (fav, heart_resp.hovered()) {
                    (true, _) => t.accent,
                    (false, true) => t.text,
                    (false, false) => t.muted,
                };
                ui.painter().text(heart.center(), Align2::CENTER_CENTER, "♥", FontId::proportional(15.0), color);
                let tip = if fav { tr("Quitar de favoritos", "Remove from favorites") } else { tr("Agregar a favoritos", "Add to favorites") };
                if heart_resp.on_hover_text(tip).clicked() {
                    favorite_track = Some(i);
                }
                if let Some(r) = icon_resp {
                    focus_ring(ui, &t, &r, icon, 6);
                    let progress = self.downloads.get(&download_key(&detail, Some(i))).copied().or(on_disk[i].then_some((1, 1)));
                    match progress {
                        Some((done, total)) if done < total => egui::Spinner::new().size(14.0).color(t.muted).paint_at(ui, icon.shrink(7.0)),
                        _ => {
                            let (glyph, tip) = match progress {
                                Some(_) => ("✔", tr("Abrir la carpeta", "Open folder")),
                                None => ("⬇", tr("Descargar canción", "Download track")),
                            };
                            let color = if r.hovered() { t.accent } else { t.muted };
                            ui.painter().text(icon.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(17.0), color);
                            if r.on_hover_text(tip).clicked() {
                                download_track = Some((i, progress.is_some()));
                            }
                        }
                    }
                }
                let p = ui.painter_at(rect);
                let y = rect.center().y;
                let number = if playing { "♪".to_string() } else { (i + 1).to_string() };
                p.text(egui::pos2(rect.left() + 30.0, y), Align2::RIGHT_CENTER, number, FontId::proportional(13.0), if playing { t.accent } else { t.muted });
                p.text(egui::pos2(rect.right() - 16.0, y), Align2::RIGHT_CENTER, &track.duration, FontId::proportional(13.0), t.muted);
                let name = ui.painter_at(Rect::from_x_y_ranges(rect.left() + 50.0..=rect.right() - 130.0, rect.y_range()));
                let color = if playing { t.accent } else { t.text };
                name.text(egui::pos2(rect.left() + 50.0, y), Align2::LEFT_CENTER, &track.name, FontId::proportional(14.0), color);
                if resp.clicked() {
                    play = Some(i);
                }
            }
            ui.add_space(12.0);
        });
        if let Some(i) = play {
            self.playback.album = Some((detail.title.clone(), detail.cover.clone(), detail.nsfw));
            self.play(tracks, i);
        }
        if favorite_album && let Some(slug) = self.selected.clone() {
            self.toggle_favorite_album(&slug, &detail);
        }
        if let Some(i) = favorite_track {
            self.toggle_favorite_song(&detail, i);
        }
        match download_track {
            Some((_, true)) => open_folder(&folder),
            Some((i, false)) => self.request_download(detail.clone(), Some(i)),
            None => {}
        }
        if download {
            let complete = on_disk.iter().all(|&d| d);
            match self.downloads.get(&detail.title) {
                Some(_) => open_folder(&folder),
                None if complete => open_folder(&folder),
                None => self.request_download(detail, None),
            }
        }
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        let full = ui.available_rect_before_wrap();

        // Izquierda: portada, pista y álbum en curso (o el último error).
        let cover = Rect::from_min_size(egui::pos2(full.left(), full.center().y - 30.0), vec2(60.0, 60.0));
        let (url, nsfw) = self.playback.album.as_ref().map_or((None, false), |(_, c, nsfw)| (c.clone(), *nsfw));
        self.paint_cover(ui, url.as_deref(), nsfw, cover, 6);
        // La barra de progreso se centra y deja 330 px a cada lado para la info y el volumen.
        let width = (full.width() - 660.0).clamp(200.0, 620.0);
        let (line1, line2) = match (&self.error, self.playback.current()) {
            // La frase simple arriba y, si es distinta, el detalle técnico debajo.
            (Some(e), _) => {
                let friendly = friendly_error(e);
                let detail = if friendly == *e { String::new() } else { e.clone() };
                (friendly, detail)
            }
            (_, Some(track)) if self.playback.loading => (track.name.clone(), tr("Cargando…", "Loading…").into()),
            (_, Some(track)) => (track.name.clone(), self.playback.album.as_ref().map(|a| a.0.clone()).unwrap_or_default()),
            _ => (tr("Nada sonando", "Nothing playing").into(), String::new()),
        };
        let color = if self.error.is_some() { ui.visuals().error_fg_color } else { t.text };
        let x = cover.right() + 12.0;
        // El texto termina 50 px antes de la barra, donde empieza el tiempo transcurrido.
        let max_width = full.center().x - width / 2.0 - 50.0 - x;
        let y = full.center().y;
        // Los textos largos se cortan con «…» y se desplazan solo con el mouse encima, desde el principio:
        // así se leen completos sin que la barra se mueva sola mientras suena la pista.
        let info = Rect::from_min_max(egui::pos2(full.left(), full.top()), egui::pos2(x + max_width, full.bottom()));
        let hover_id = ui.id().with("marquee-hover");
        let now = ui.input(|i| i.time);
        let since = match ui.rect_contains_pointer(info) {
            true => Some(ui.data_mut(|d| *d.get_temp_mut_or_insert_with(hover_id, || now))),
            false => {
                ui.data_mut(|d| d.remove::<f64>(hover_id));
                None
            }
        };
        let elapsed = since.map(|s| now - s);
        marquee(ui, egui::pos2(x, y - 10.0), line1, FontId::proportional(16.0), color, max_width, elapsed);
        marquee(ui, egui::pos2(x, y + 12.0), line2, FontId::proportional(13.0), t.muted, max_width, elapsed);

        // Centro: controles y barra de progreso.
        let center = full.center().x;
        let y = full.top() + 24.0;
        let mode = self.playback.mode;
        let shuffle = mode == Mode::Shuffle;
        let r = round_button(ui, &t, egui::pos2(center - 112.0, y), 15.0, "🔀", false, shuffle);
        if r.on_hover_text(Mode::Shuffle.label()).clicked() {
            self.set_mode(if shuffle { Mode::Continue } else { Mode::Shuffle });
        }
        if round_button(ui, &t, egui::pos2(center - 56.0, y), 16.0, "⏮", false, false).clicked() {
            self.previous();
        }
        let icon = if self.playback.paused() { "▶" } else { "⏸" };
        if round_button(ui, &t, egui::pos2(center, y), 22.0, icon, true, false).clicked() {
            self.toggle();
        }
        if round_button(ui, &t, egui::pos2(center + 56.0, y), 16.0, "⏭", false, false).clicked() {
            self.next();
        }
        // Repetir recorre: el álbum, la canción y apagado.
        let repeat = matches!(mode, Mode::Repeat | Mode::RepeatOne);
        let spot = egui::pos2(center + 112.0, y);
        let r = round_button(ui, &t, spot, 15.0, "🔁", false, repeat);
        if mode == Mode::RepeatOne {
            // En la fuente de íconos 🔂 casi no se distingue de 🔁; un "1" lo deja claro.
            ui.painter().text(spot + vec2(8.0, 6.0), Align2::CENTER_CENTER, "1", semibold(9.0), t.accent);
        }
        if r.on_hover_text(if repeat { mode.label() } else { Mode::Repeat.label() }).clicked() {
            self.set_mode(match mode {
                Mode::Repeat => Mode::RepeatOne,
                Mode::RepeatOne => Mode::Continue,
                _ => Mode::Repeat,
            });
        }

        let total = self.playback.current().map_or(0.0, |t| khinsider::seconds(&t.duration)).max(1.0);
        let pos = self.playback.drag.unwrap_or_else(|| self.playback.position()).min(total);
        let bar = Rect::from_center_size(egui::pos2(center, full.bottom() - 8.0), vec2(width, 4.0));
        // Al arrastrar solo se mueve la barra; el salto en el audio se hace al soltar.
        let (resp, target) = thin_bar(ui, &t, bar, pos / total, "progress");
        let target = target.map(|f| f * total);
        if resp.dragged() {
            self.playback.drag = target;
        }
        if resp.drag_stopped() || resp.clicked() {
            self.playback.drag = None;
            if let Some(secs) = target {
                self.seek(secs);
            }
        }
        let font = FontId::proportional(12.0);
        let p = ui.painter();
        p.text(bar.left_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, clock(pos), font.clone(), t.muted);
        p.text(bar.right_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, clock(total), font, t.muted);

        // Derecha: volumen. El ícono silencia y recupera el nivel anterior.
        let volume = self.playback.volume;
        let bar = Rect::from_min_size(egui::pos2(full.right() - 110.0, full.center().y - 2.0), vec2(110.0, 4.0));
        let icon = match volume {
            0.0 => "🔇",
            v if v < 0.5 => "🔉",
            _ => "🔊",
        };
        let y = bar.center().y;
        let mute = round_button(ui, &t, egui::pos2(bar.left() - 22.0, y), 14.0, icon, false, false);
        if mute.clicked() {
            if volume > 0.0 {
                self.playback.unmuted = volume;
                self.playback.set_volume(0.0);
            } else {
                self.playback.set_volume(self.playback.unmuted.max(0.1));
            }
        }
        let (resp, target) = thin_bar(ui, &t, bar, volume, "volume");
        if let (true, Some(v)) = (resp.dragged() || resp.clicked(), target) {
            self.playback.set_volume(v);
        }
        let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
        if scroll != 0.0 {
            self.playback.set_volume(volume + scroll / 500.0);
        }
        // Se guarda al terminar cada gesto, no en cada cuadro del arrastre.
        if mute.clicked() || resp.drag_stopped() || resp.clicked() || scroll != 0.0 {
            settings::set("volume", self.playback.volume);
        }

        // Junto al volumen: visualizador.
        let r = round_button(ui, &t, egui::pos2(bar.left() - 58.0, y), 14.0, "📊", false, self.show_visual);
        if r.on_hover_text(tr("Visualizador", "Visualizer")).clicked() {
            self.show_visual = !self.show_visual;
        }
    }

    /// Visualizador a pantalla completa. El reproductor y el menú de estilos flotan encima y se
    /// desvanecen tras 2,5 s sin mover el mouse.
    fn visual_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme().clone();
        let ctx = ui.ctx().clone();
        let time = ui.input(|i| i.time);
        let screen = ctx.content_rect();
        let colors = visualizer::PALETTES[self.palette].colors(t.accent);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.crust)).show(ui, |ui| {
            self.visual.paint(ui.painter(), ui.max_rect(), &self.spectrum, time as f32, &colors);
        });

        let (moved, pointer) = ui.input(|i| (i.pointer.delta() != vec2(0.0, 0.0) || i.pointer.any_down(), i.pointer.hover_pos()));
        if moved {
            self.last_motion = time;
        }
        // Con el mouse sobre los controles no se ocultan.
        let over_controls = pointer.is_some_and(|p| p.y > screen.bottom() - 90.0 || (p.y < 70.0 && p.x > screen.right() - 320.0));
        let awake = time - self.last_motion < 2.5 || over_controls;
        let alpha = ctx.animate_bool_with_time(egui::Id::new("visual-controls"), awake, 0.35);
        if alpha == 0.0 {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        let panel = || egui::Frame::new().fill(t.crust.gamma_multiply(0.88)).inner_margin(12).corner_radius(10);

        egui::Area::new(egui::Id::new("visual-player"))
            .anchor(Align2::LEFT_BOTTOM, vec2(0.0, 0.0))
            .interactable(alpha > 0.05)
            .show(&ctx, |ui| {
                ui.multiply_opacity(alpha);
                panel().corner_radius(0).show(ui, |ui| {
                    ui.set_width(screen.width() - 24.0);
                    ui.set_height(52.0);
                    self.player_bar(ui);
                });
            });

        egui::Area::new(egui::Id::new("visual-menu"))
            .anchor(Align2::RIGHT_TOP, vec2(-16.0, 16.0))
            .interactable(alpha > 0.05)
            .show(&ctx, |ui| {
                ui.multiply_opacity(alpha);
                panel().inner_margin(6).show(ui, |ui| {
                    // Misma altura para botones y desplegables, así quedan alineados.
                    ui.spacing_mut().interact_size.y = 30.0;
                    let icon = |text: &str| egui::Button::new(RichText::new(text).size(16.0)).min_size(vec2(30.0, 30.0));
                    ui.horizontal(|ui| {
                        let mut style = self.visual;
                        if ui.add(icon("‹")).on_hover_text(tr("Estilo anterior (←)", "Previous style (←)")).clicked() {
                            style = style.cycle(-1);
                        }
                        egui::ComboBox::from_id_salt("visual-style").width(160.0).selected_text(style.label()).show_ui(ui, |ui| {
                            for option in visualizer::Style::ALL {
                                ui.selectable_value(&mut style, option, option.label());
                            }
                        });
                        if ui.add(icon("›")).on_hover_text(tr("Estilo siguiente (→)", "Next style (→)")).clicked() {
                            style = style.cycle(1);
                        }
                        if style != self.visual {
                            self.visual = style;
                            settings::set("visualizer", style.key());
                        }
                        ui.separator();
                        let mut palette = self.palette;
                        egui::ComboBox::from_id_salt("visual-palette")
                            .width(140.0)
                            .selected_text(visualizer::PALETTES[palette].label())
                            .show_ui(ui, |ui| {
                                for (i, option) in visualizer::PALETTES.iter().enumerate() {
                                    ui.selectable_value(&mut palette, i, option.label());
                                }
                            })
                            .response
                            .on_hover_text(tr("Colores del visualizador", "Visualizer colors"));
                        if palette != self.palette {
                            self.palette = palette;
                            settings::set("visualizer-palette", visualizer::PALETTES[palette].key);
                        }
                        ui.separator();
                        if ui.add(icon("×")).on_hover_text(tr("Salir del visualizador (Esc)", "Exit visualizer (Esc)")).clicked() {
                            self.show_visual = false;
                        }
                    });
                });
            });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_visual = false;
        }
        let step = ui.input(|i| i.key_pressed(egui::Key::ArrowRight) as isize - i.key_pressed(egui::Key::ArrowLeft) as isize);
        if step != 0 {
            self.visual = self.visual.cycle(step);
            settings::set("visualizer", self.visual.key());
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        // Avanza a la siguiente pista cuando termina la actual.
        let finished = self.playback.device.as_ref().is_some_and(|(_, p)| p.empty());
        if self.playback.active && finished {
            // Repetir la canción solo aplica al terminar; el botón "siguiente" sigue avanzando.
            if self.playback.mode == Mode::RepeatOne {
                self.load(self.playback.index);
            } else {
                self.next();
            }
        }
        if self.playback.active && !self.playback.paused() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if self.show_visual {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.spectrum.update(&self.feed, dt);
            // Animación continua solo mientras la vista está abierta.
            ui.ctx().request_repaint();
        }
        // La barra espaciadora pausa, salvo mientras se escribe en el buscador.
        if ui.memory(|m| m.focused().is_none()) && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle();
        }

        if self.show_visual {
            return self.visual_view(ui);
        }
        let t = self.theme().clone();
        let frame = |fill, margin: i8| egui::Frame::new().fill(fill).inner_margin(margin);
        // El orden define el layout: el reproductor ocupa el ancho completo y la barra lateral va encima.
        egui::Panel::bottom("player").exact_size(96.0).frame(frame(t.crust, 16)).show_separator_line(false).show(ui, |ui| self.player_bar(ui));
        egui::Panel::left("nav").exact_size(230.0).frame(frame(t.mantle, 16)).show_separator_line(false).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().frame(frame(t.base, 28)).show(ui, |ui| self.main_view(ui));
        if self.format_prompt.is_some() {
            self.format_dialog(&ui.ctx().clone());
        }
    }
}

/// Entrada de la barra lateral: ícono y texto en columnas fijas, con fondo al pasar el mouse o al estar activa.
/// `cursor` recibe el rectángulo de la entrada activa, si está a la vista: el resaltado lo pinta
/// `sidebar` para poder deslizarlo.
fn nav_item(ui: &mut egui::Ui, t: &Theme, icon: &str, text: &str, selected: bool, cursor: &mut Option<Rect>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    if selected && ui.clip_rect().contains_rect(rect) {
        *cursor = Some(rect);
    }
    if !selected && resp.hovered() {
        ui.painter().rect_filled(rect, 8, t.hover());
    }
    focus_ring(ui, t, &resp, rect, 8);
    let color = if selected { t.accent } else { t.text };
    let p = ui.painter();
    p.text(rect.left_center() + vec2(22.0, 0.0), Align2::CENTER_CENTER, icon, FontId::proportional(16.0), color);
    p.text(rect.left_center() + vec2(44.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(15.0), color);
    resp
}

fn divider(ui: &mut egui::Ui, t: &Theme) {
    ui.add_space(12.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, t.surface));
    ui.add_space(12.0);
}

/// Botón circular del reproductor; `primary` lo rellena con el color de acento.
/// `on` pinta el ícono con el acento, para botones que alternan un estado.
fn round_button(ui: &mut egui::Ui, t: &Theme, center: egui::Pos2, radius: f32, icon: &str, primary: bool, on: bool) -> egui::Response {
    let rect = Rect::from_center_size(center, vec2(radius, radius) * 2.0);
    let resp = ui.interact(rect, ui.id().with(center.x as i32), Sense::click());
    let fg = if on { t.accent } else { t.text };
    let (fill, fg) = match (primary, resp.hovered()) {
        (true, _) => (t.accent, t.base),
        (false, true) => (t.surface, fg),
        (false, false) => (Color32::TRANSPARENT, fg),
    };
    let p = ui.painter();
    p.circle_filled(center, if primary && resp.hovered() { radius + 1.0 } else { radius }, fill);
    p.text(center, Align2::CENTER_CENTER, icon, FontId::proportional(radius * 0.9), fg);
    focus_ring(ui, t, &resp, rect, radius as u8);
    resp
}

/// Anillo de foco de teclado para los controles que la app pinta a mano: egui solo lo dibuja en sus
/// propios widgets. Un clic con el mouse no da foco, así que el anillo aparece solo al usar Tab.
fn focus_ring(ui: &egui::Ui, t: &Theme, resp: &egui::Response, rect: Rect, radius: u8) {
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), radius, egui::Stroke::new(2.0, t.text), egui::StrokeKind::Outside);
    }
}

/// Caja de diálogo de RPG, el motivo de identidad de `DESIGN.md`: un borde grueso por fuera y uno fino
/// por dentro, como las ventanas de texto de los juegos de consola. Enmarca los títulos de sección y
/// el panel del álbum.
///
/// Con `gradient`, el fondo es un degradado pixel art con esos colores en lugar del color `mantle`.
fn dialog_box<R>(ui: &mut egui::Ui, t: &Theme, gradient: Option<&[Color32]>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let background = ui.painter().add(egui::Shape::Noop);
    let frame = egui::Frame::new()
        .fill(if gradient.is_some() { Color32::TRANSPARENT } else { t.mantle })
        .stroke(egui::Stroke::new(2.0, t.muted))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(18, 12));
    let shown = frame.show(ui, add);
    if let Some(stops) = gradient {
        ui.painter().set(background, egui::Shape::Vec(palette::pixel_gradient(shown.response.rect.shrink(1.0), stops, 8.0)));
    }
    ui.painter().rect_stroke(shown.response.rect.shrink(5.0), 3, egui::Stroke::new(1.0, t.muted), egui::StrokeKind::Inside);
    shown.inner
}

/// Barra delgada del reproductor (progreso y volumen). Devuelve la respuesta y la fracción (0 a 1)
/// bajo el puntero mientras se interactúa.
fn thin_bar(ui: &mut egui::Ui, t: &Theme, bar: Rect, fraction: f32, id: &str) -> (egui::Response, Option<f32>) {
    let resp = ui.interact(bar.expand2(vec2(0.0, 8.0)), ui.id().with(id), Sense::click_and_drag());
    let target = resp.interact_pointer_pos().map(|p| ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0));
    let fill_x = bar.left() + bar.width() * target.filter(|_| resp.dragged()).unwrap_or(fraction).clamp(0.0, 1.0);
    let p = ui.painter();
    p.rect_filled(bar, 2, t.surface);
    p.rect_filled(Rect::from_min_max(bar.min, egui::pos2(fill_x, bar.bottom())), 2, t.accent);
    if resp.hovered() || resp.dragged() {
        p.circle_filled(egui::pos2(fill_x, bar.center().y), 6.0, t.text);
    }
    (resp, target)
}

/// Texto de una línea anclado a la izquierda. Si no cabe en `max_width` se desplaza en bucle
/// (marquesina), con una pausa al inicio de cada vuelta.
/// Texto de una línea. Si no entra en `max_width` se corta con «…»; con `elapsed` (segundos desde que
/// el mouse está encima) se desplaza para mostrarlo completo.
fn marquee(ui: &egui::Ui, left_center: egui::Pos2, text: String, font: FontId, color: Color32, max_width: f32, elapsed: Option<f64>) {
    const GAP: f32 = 48.0;
    const SPEED: f32 = 30.0; // px por segundo
    const PAUSE: f64 = 1.0; // segundos quieto antes de cada vuelta
    let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), color);
    let size = galley.size();
    let pos = left_center - vec2(0.0, size.y / 2.0);
    if size.x <= max_width {
        ui.painter().galley(pos, galley, color);
        return;
    }
    let Some(elapsed) = elapsed else {
        let mut job = egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font, color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
        ui.painter().galley(pos, ui.painter().layout_job(job), color);
        return;
    };
    let lap = size.x + GAP;
    let t = elapsed % (PAUSE + f64::from(lap / SPEED));
    let offset = (t - PAUSE).max(0.0) as f32 * SPEED;
    // Dos copias separadas por GAP: cuando la primera sale por la izquierda, la segunda ya está entrando.
    let p = ui.painter_at(Rect::from_min_size(pos, vec2(max_width.max(0.0), size.y)));
    p.galley(pos - vec2(offset, 0.0), galley.clone(), color);
    p.galley(pos + vec2(lap - offset, 0.0), galley, color);
    // ~30 fps solo mientras haya texto desplazándose.
    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

/// Entero al azar en `0..n` sin sumar una dependencia: `RandomState` trae semillas nuevas en cada llamada.
fn random_below(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    (std::collections::hash_map::RandomState::new().build_hasher().finish() % n as u64) as usize
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::SEMIBOLD.into()))
}

/// Columna centrada un poco arriba de la mitad del área, para los estados de carga, vacío y error.
fn state_column(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui)) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.set_max_width(460.0);
        f(ui);
    });
}

fn loading_state(ui: &mut egui::Ui, t: &Theme, what: &str) {
    state_column(ui, |ui| {
        ui.spinner();
        ui.add_space(8.0);
        ui.label(RichText::new(what).color(t.muted));
    });
}

fn empty_state(ui: &mut egui::Ui, t: &Theme, message: &str) {
    state_column(ui, |ui| _ = ui.label(RichText::new(message).color(t.muted)));
}

/// Estado de error: qué pasó en palabras simples, el detalle técnico debajo y un botón para
/// reintentar. Devuelve `true` si se tocó «Reintentar».
fn error_state(ui: &mut egui::Ui, e: &str) -> bool {
    let mut retry = false;
    state_column(ui, |ui| {
        let friendly = friendly_error(e);
        ui.label(RichText::new(&friendly).size(16.0).color(ui.visuals().error_fg_color));
        if friendly != e {
            ui.add_space(4.0);
            ui.label(RichText::new(e).small().weak());
        }
        ui.add_space(12.0);
        retry = ui.button(tr("Reintentar", "Try again")).clicked();
    });
    retry
}

/// Traduce los errores de red y del sitio a una frase que dice qué pasó. Los demás quedan igual.
fn friendly_error(e: &str) -> String {
    let text = if e.contains("error sending request") || e.contains("(Connect)") || e.contains("timed out") {
        tr("No se pudo conectar con KHInsider. Revisa tu conexión a internet.", "Could not connect to KHInsider. Check your internet connection.")
    } else if e.contains("HTTP 5") {
        tr("KHInsider no está respondiendo. Prueba de nuevo en unos minutos.", "KHInsider is not responding. Try again in a few minutes.")
    } else if e.contains("HTTP 403") {
        tr("KHInsider rechazó el pedido. Suele pasar un momento; prueba de nuevo.", "KHInsider refused the request. It usually passes; try again.")
    } else {
        return e.to_string();
    };
    text.to_string()
}

fn clock(secs: f32) -> String {
    format!("{}:{:02}", secs as u32 / 60, secs as u32 % 60)
}

fn long_clock(secs: f32) -> String {
    let m = (secs / 60.0).round() as u32;
    if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }
}

fn default_download_dir() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Music").join("KHI-UI").display().to_string()
}

/// Opciones de formato de descarga; `None` es «Preguntar».
const FORMATS: [Option<Format>; 3] = [None, Some(Format::Mp3), Some(Format::Flac)];

fn format_key(format: Option<Format>) -> &'static str {
    format.map_or("ask", Format::ext)
}

fn format_label(format: Option<Format>) -> &'static str {
    match format {
        None => tr("Preguntar si el álbum tiene FLAC", "Ask when the album has FLAC"),
        Some(Format::Mp3) => "MP3",
        Some(Format::Flac) => tr("FLAC (MP3 si la pista no lo tiene)", "FLAC (MP3 when the track lacks it)"),
    }
}

/// Clave de una descarga en `App::downloads`: el título del álbum, o el path de la pista si es una sola.
fn download_key(album: &AlbumDetail, track: Option<usize>) -> String {
    track.map_or_else(|| album.title.clone(), |i| album.tracks[i].path.clone())
}

/// Nombre del archivo de una pista, sin extensión: número de dos cifras y nombre.
fn track_file(i: usize, track: &Track) -> String {
    format!("{:02} {}", i + 1, file_name(&track.name))
}

/// Archivo de la pista que ya está en `dir`, buscando los formatos en ese orden.
fn saved_file(dir: &Path, i: usize, track: &Track, order: [Format; 2]) -> Option<String> {
    let base = track_file(i, track);
    order.iter().map(|f| format!("{base}.{}", f.ext())).find(|f| dir.join(f).exists())
}

/// Nombre válido de archivo o carpeta: reemplaza los caracteres que Windows, macOS o Linux no aceptan.
fn file_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    clean.trim().trim_end_matches('.').to_string()
}

/// Error de disco con la ruta que lo causó.
fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// Abre la carpeta en el explorador de archivos del sistema, creándola si todavía no existe.
fn open_folder(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::process::Command::new(opener).arg(path).spawn() {
        eprintln!("no se pudo abrir {}: {e}", path.display());
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KHI-UI")
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("assets/icon.png es un PNG válido"))
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([900.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("KHI-UI", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// Renderiza la app con cada tema contra el sitio real y guarda `target/theme-*.png`:
/// `cargo test screenshots -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(file_name(" AC/DC: Live? "), "AC_DC_ Live_");
        assert_eq!(file_name("Vol. 2..."), "Vol. 2");
    }

    /// Espera a que lleguen las imágenes pedidas y a que egui termine de decodificarlas.
    fn settle(h: &mut egui_kittest::Harness<'_, App>) {
        wait(h, |a| a.net.requested.len() == a.net.loaded.len());
        for _ in 0..20 {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
        // Las imágenes pedidas en esos cuadros también tienen que llegar.
        wait(h, |a| a.net.requested.len() == a.net.loaded.len());
        h.run_steps(4);
    }

    fn wait(h: &mut egui_kittest::Harness<'_, App>, done: impl Fn(&App) -> bool) {
        for _ in 0..800 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("la app no terminó de cargar");
    }

    #[test]
    #[ignore]
    fn screenshots() {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 760.0)).wgpu().build_eframe(|cc| App::new(cc));
        wait(&mut h, |a| matches!(a.albums, Some(Ok(_))));
        let slug = match &h.state().albums {
            Some(Ok(a)) => a[1].slug.clone(),
            _ => unreachable!(),
        };
        h.state_mut().select(&slug);
        wait(&mut h, |a| matches!(a.detail, Some(Ok(_))));
        // Simula una pista en curso (sin abrir el audio) para ver la barra del reproductor.
        let app = h.state_mut();
        let Some(Ok(d)) = app.detail.clone() else { unreachable!() };
        app.playback.queue = d.tracks.clone().into();
        app.playback.index = 1;
        app.playback.album = Some((d.title.clone(), d.cover.clone(), d.nsfw));
        app.playback.mode = Mode::RepeatOne;
        let feed = app.feed.clone();
        // Tonos sintéticos pasados por el mismo `Tap` que usa la reproducción real.
        let pump = || {
            use rodio::Source;
            let tones = [55.0, 110.0, 220.0, 440.0, 1200.0, 3000.0, 7000.0]
                .map(|hz| rodio::source::SineWave::new(hz).amplify(0.15));
            let [a, b, c, d, e, f, g] = tones;
            let mix = a.mix(b).mix(c).mix(d).mix(e).mix(f).mix(g);
            visualizer::Tap::new(mix, feed.clone()).take(8192).for_each(drop);
        };
        let saved = theme::saved();
        for i in 0..h.state().themes.len() {
            h.state_mut().set_theme(i);
            wait(&mut h, |a| a.net.requested.len() == a.net.loaded.len());
            h.run_steps(4);
            let name = h.state().themes[i].name.to_lowercase().replace(' ', "-");
            h.render().unwrap().save(format!("target/theme-{name}.png")).unwrap();
        }
        // Búsqueda real con el primer tema.
        h.state_mut().set_theme(0);
        h.state_mut().go(SETTINGS);
        h.run_steps(4);
        h.render().unwrap().save("target/settings.png").unwrap();
        h.state_mut().go(DONATE);
        h.run_steps(4);
        h.render().unwrap().save("target/donate.png").unwrap();
        h.state_mut().go(0);
        wait(&mut h, |a| matches!(a.albums, Some(Ok(_))));
        settle(&mut h);
        h.render().unwrap().save("target/home.png").unwrap();
        // Descarga real de dos pistas a una carpeta temporal y su lectura desde la biblioteca.
        let tmp = std::env::temp_dir().join(format!("khi-ui-downloads-{}", std::process::id()));
        let app = h.state_mut();
        let Some(Ok(d)) = app.detail.clone() else { unreachable!() };
        let short = AlbumDetail { title: d.title.clone(), cover: d.cover.clone(), tracks: d.tracks[..2].to_vec(), flac: d.flac, nsfw: d.nsfw, id: None };
        app.download_dir = tmp.display().to_string();
        app.download(Arc::new(short), None, Format::Mp3);
        wait(&mut h, |a| a.downloads.values().all(|(done, total)| done == total) || a.error.is_some());
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        h.state_mut().open_section(LIBRARY);
        let slug = match &h.state().albums {
            Some(Ok(a)) if a.len() == 1 => a[0].slug.clone(),
            _ => panic!("la biblioteca no tiene el álbum descargado"),
        };
        h.state_mut().select(&slug);
        wait(&mut h, |a| matches!(&a.detail, Some(Ok(d)) if d.tracks.len() == 2 && d.tracks[0].duration != ""));
        wait(&mut h, |a| a.net.requested.len() == a.net.loaded.len());
        let Some(Ok(local)) = h.state().detail.clone() else { unreachable!() };
        let mp3 = std::fs::read(local.tracks[0].path.strip_prefix(library::LOCAL).unwrap()).unwrap();
        rodio::Decoder::try_from(Cursor::new(mp3)).expect("la pista descargada se decodifica");
        let cover = std::fs::read_dir(tmp.read_dir().unwrap().next().unwrap().unwrap().path()).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("cover."));
        assert!(cover.is_some_and(|c| c.metadata().unwrap().len() > 0), "falta la portada");
        for _ in 0..20 {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
        h.render().unwrap().save("target/library.png").unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
        // Favoritos y playlists, solo si hay una cuenta guardada.
        wait(&mut h, |a| a.session != Session::Busy);
        if matches!(h.state().session, Session::In(_)) {
            for (section, name) in [(FAVORITES, "favorites"), (PLAYLISTS, "playlists")] {
                h.state_mut().open_section(section);
                wait(&mut h, |a| a.albums.is_some());
                let first = match &h.state().albums {
                    Some(Ok(a)) if !a.is_empty() => a[0].slug.clone(),
                    other => panic!("{name}: {:?}", other.as_ref().map(|r| r.as_ref().map(|a| a.len()))),
                };
                h.state_mut().select(&first);
                wait(&mut h, |a| matches!(&a.detail, Some(Ok(d)) if !d.tracks.is_empty()));
                wait(&mut h, |a| a.net.requested.len() == a.net.loaded.len());
                for _ in 0..20 {
                    h.step();
                    std::thread::sleep(Duration::from_millis(50));
                }
                h.render().unwrap().save(format!("target/{name}.png")).unwrap();
            }
        }
        h.state_mut().query = "zelda".into();
        h.state_mut().search = "zelda".into();
        h.state_mut().open_section(SEARCH);
        wait(&mut h, |a| matches!(a.albums, Some(Ok(_))));
        settle(&mut h);
        assert!(matches!(&h.state().albums, Some(Ok(a)) if a.len() > 50));
        h.render().unwrap().save("target/search.png").unwrap();
        // Visualizador a pantalla completa: barras con los controles ocultos, psicodélico con los controles a la vista.
        h.state_mut().show_visual = true;
        for (i, style) in visualizer::Style::ALL.into_iter().enumerate() {
            let awake = style == visualizer::Style::Psychedelic;
            // Cada estilo con una paleta distinta, para revisarlas todas en las capturas.
            h.state_mut().palette = i % visualizer::PALETTES.len();
            h.state_mut().visual = style;
            h.state_mut().last_motion = if awake { f64::MAX } else { f64::MIN };
            for _ in 0..40 {
                pump();
                h.step();
            }
            h.render().unwrap().save(format!("target/visual-{}.png", style.key())).unwrap();
        }
        // Catálogo por plataforma: primero la cuadrícula de opciones, después la lista de una.
        h.state_mut().show_visual = false;
        h.state_mut().open_category(Category::Platform);
        wait(&mut h, |a| matches!(a.albums, Some(Ok(_))));
        assert!(h.state().filter.as_ref().is_some_and(|f| f.name == "3DO"));
        settle(&mut h);
        h.render().unwrap().save("target/browse-platforms.png").unwrap();
        let snes = match h.state().filters.get(&Category::Platform) {
            Some(Ok(f)) => f.iter().find(|f| f.name == "SNES").unwrap().clone(),
            _ => unreachable!(),
        };
        h.state_mut().choose_filter(snes);
        wait(&mut h, |a| matches!(a.albums, Some(Ok(_))));
        settle(&mut h);
        assert!(h.state().has_more);
        h.render().unwrap().save("target/browse-snes.png").unwrap();
        // Deja el tema que tenía el usuario.
        let back = h.state().themes.iter().position(|t| Some(&t.name) == saved.as_ref()).unwrap_or(0);
        h.state_mut().set_theme(back);
    }
}
