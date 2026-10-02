<p align="center"><img src="assets/logo.svg" width="128" alt="Logo de KHI-UI"></p>

# KHI-UI

KHI-UI (se pronuncia como *kiwi*) es un cliente de escritorio para [downloads.khinsider.com](https://downloads.khinsider.com/). Muestra los
últimos soundtracks publicados, la portada de cada álbum y reproduce sus pistas.

Please consider [donating to the KHInsider team](https://downloads.khinsider.com/forums/index.php?account/upgrades). Every bit helps!

## Stack

- **Rust + egui/eframe:** la interfaz se dibuja en modo inmediato con OpenGL, así que arranca rápido
  y solo repinta cuando hay eventos o música sonando.
- **wreq:** cliente HTTP que imita la huella TLS y HTTP/2 de Safari. Cloudflare rechaza las páginas de
  álbum cuando el cliente es curl, reqwest u OpenSSL, y este cliente evita ese bloqueo tanto en macOS
  como en Linux.
- **rodio:** reproduce MP3 y FLAC. El dispositivo de audio se abre en la primera reproducción para no
  demorar el arranque.
- **tokio:** corre las descargas en dos hilos de fondo y entrega los resultados a la interfaz por un canal.

## Navegación

La barra lateral tiene arriba lo propio: **Inicio** (los últimos soundtracks), **Biblioteca** y, con la
sesión iniciada, **Mis favoritos** y **Mis playlists**. Debajo, en **Explorar**, están **Tops**,
**Catálogo** (por letra, plataforma, tipo o año) y **Álbum al azar**. En Tops y Catálogo, los
desplegables junto al título cambian la lista. Al pie están **Donar**, que explica cómo apoyar a
KHInsider y abre su página de donaciones, y **Opciones**.

Cada sección muestra sus álbumes en una cuadrícula de portadas. Al abrir uno aparece su vista con la
tabla de pistas, y la flecha de arriba vuelve a la cuadrícula. El buscador está arriba a la derecha.

## Opciones

El botón **Opciones**, al pie de la barra lateral, abre la pantalla de ajustes. Cada cambio se guarda
al momento en un archivo de `~/.config/khi-ui/`.

- **Tema:** se guarda en `theme`. La sección siguiente explica cómo crear temas propios.
- **Idioma:** español o inglés, guardado en `language`. En el primer arranque la app usa el idioma del
  sistema (la variable `LANG`).
- **Carpeta de descargas:** se guarda en `download-dir` y por defecto es `~/Music/KHI-UI`. Se puede
  escribir la ruta o elegirla con el botón **Elegir…**, que abre el diálogo de carpetas del sistema.
  El botón **Descargar** de cada álbum guarda sus pistas, la portada y un `album.tsv` (título, nombres y
  duraciones) en una subcarpeta con el nombre del álbum. Si una descarga se corta, repetirla retoma
  desde la primera pista que falta. Cada pista tiene un botón ⬇, al lado de la duración, para bajar solo
  esa canción a la misma carpeta; `album.tsv` lista únicamente las pistas que ya están en disco.
- **Portadas +18:** se guarda en `show-nsfw` y por defecto está apagada. El sitio tapa esas portadas
  con un aviso; apagada, la app muestra un recuadro con «+18». Encendida, muestra la portada real. En
  las listas el sitio no trae esa portada, así que la app la busca en la página del álbum, una vez por
  álbum. Al descargar un álbum +18 se crea un archivo `.nsfw` en su carpeta, para que la biblioteca
  también respete la opción.
- **Reducir animaciones:** se guarda en `reduce-motion` y por defecto está apagada. Encendida, la app
  cambia de pantalla sin transiciones y muestra las listas y el menú sin movimiento.
- **Formato de descarga:** se guarda en `download-format`. Con **Preguntar** (el valor por defecto), la
  app pregunta entre MP3 y FLAC cada vez que el álbum tiene FLAC. Con **FLAC**, las pistas que no
  existen en FLAC se bajan en MP3. Los álbumes sin FLAC siempre se bajan en MP3. En las playlists
  siempre se ofrece FLAC, porque sus pistas vienen de álbumes distintos y el sitio no dice cuáles lo
  tienen.
- **Marca de descargado:** al abrir un álbum, la app busca sus pistas en la carpeta de descargas. Las
  que ya están aparecen con ✔, y si están todas, el botón del álbum muestra **Descargado**, aunque se
  hayan bajado en otra sesión.

## Cuenta

En **Opciones** se puede iniciar sesión con la cuenta de KHInsider. Con la sesión iniciada, la
barra lateral muestra **Mis playlists**, que se reproducen y descargan como cualquier álbum.

- El usuario se guarda en `~/.config/khi-ui/user`.
- La contraseña se guarda **en texto plano** en `~/.config/khi-ui/password`, con permisos que solo
  dejan leerla al usuario (`0600`).
- Al arrancar, la app inicia sesión sola con esos datos. **Cerrar sesión** borra el archivo de la
  contraseña y descarta las cookies.
- Las cuentas con verificación en dos pasos todavía no se soportan.

## Favoritos

El botón ♥ de un álbum y el de cada pista los agregan a favoritos. La barra lateral tiene dos
secciones para verlos: **Canciones favoritas**, que abre directo la lista para reproducirla o
descargarla como un álbum, y **Álbumes favoritos**.

- **Álbumes:** con la sesión iniciada se guardan en los favoritos de la cuenta de KHInsider, los
  mismos del botón «Add to Favorites» del sitio. Sin sesión se guardan en local.
- **Canciones:** el sitio no tiene canciones favoritas, así que siempre se guardan en local.
- Los favoritos locales viven en `~/.config/khi-ui/favorites.tsv`. Al quitar un álbum de favoritos se
  quita de los dos lados, por si quedó guardado en local antes de iniciar sesión.

## Biblioteca

La sección **Biblioteca** de la barra lateral muestra los álbumes de la carpeta de descargas y los
reproduce desde el disco, sin conexión. También lee carpetas que no descargó la app: en ese caso el
título es el nombre de la carpeta, las pistas son sus archivos MP3 y FLAC, y no se conoce su duración.

## Temas

La app trae tres temas: **Oscuro** (el predeterminado), **Claro** y **Catppuccin Mocha**. Se cambian
desde la pantalla de opciones.

Un tema es un archivo TOML de nueve líneas. Para crear uno:

1. Copia `themes/claro.toml` a `~/.config/khi-ui/themes/mi-tema.toml`. Ese archivo explica qué pinta
   cada color.
2. Cambia `name` y los colores. La clave opcional `name_en` da el nombre que se muestra cuando la app
   está en inglés. Las claves (`base`, `mantle`, `crust`, `surface`, `text`...) siguen la
   paleta de [Catppuccin](https://catppuccin.com/palette), así que se puede portar cualquiera de sus
   variantes copiando los valores.
3. Reinicia la app: el tema aparece en la pantalla de opciones. Si el archivo tiene un error, la app lo omite y lo
   explica en la terminal.

La tipografía es [Inter](https://rsms.me/inter/), con licencia OFL (`assets/fonts/Inter-LICENSE.txt`). Se eligió
porque está diseñada para pantallas y se lee bien a los tamaños chicos que más usa la app (duraciones,
conteos y listas largas de títulos entre 11 y 15 px), y porque cubre los acentos del español y del
portugués que aparecen en muchos títulos del catálogo. Inter
no trae caracteres japoneses, chinos ni coreanos. Para mostrarlos, la app usa las fuentes del sistema:
Hiragino y AppleGothic en macOS, Yu Gothic, Microsoft YaHei y Malgun Gothic en Windows, y Noto Sans CJK
en Linux (paquete `fonts-noto-cjk` en Debian y Ubuntu). Si no encuentra ninguna, esos caracteres se
ven como cuadrados.

## Requisitos

- Rust estable (`cargo`).
- `cmake` y `clang`, que se usan para compilar BoringSSL (la librería TLS de `wreq`).
- Solo en Linux: `libasound2-dev` y `pkg-config` para el audio (ALSA). El diálogo para elegir carpeta
  usa el portal de escritorio (`xdg-desktop-portal`), que ya viene en GNOME y KDE.

## Uso

```sh
mise run run     # compila y abre la app
mise run build   # compila en release y corre los tests offline
mise run live    # prueba contra el sitio real
```

Sin `mise`, los comandos equivalentes son `cargo run --release` y `cargo test`.

## Estructura

- `src/khinsider.rs` descarga las páginas y extrae álbumes, portadas y pistas con expresiones regulares.
- `src/main.rs` contiene la interfaz y el reproductor.
- `src/theme.rs` lee los temas, los traduce a estilos de egui y carga la tipografía.
- `src/favorites.rs` guarda las canciones favoritas y los álbumes favoritos sin sesión.
- `src/account.rs` guarda el usuario y lee o borra la contraseña guardada.
- `src/library.rs` lee los álbumes descargados para la biblioteca local.
- `src/i18n.rs` guarda el idioma elegido. Cada texto de la interfaz se escribe en español y en inglés
  donde se usa, con `tr("Hola", "Hello")`.
- `cargo test screenshots -- --ignored` renderiza la app con cada tema en `target/theme-*.png`, sin abrir
  una ventana.
