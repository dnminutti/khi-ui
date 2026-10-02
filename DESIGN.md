# Dirección de diseño de KHI-UI

Este archivo guarda las decisiones de diseño del dueño del proyecto. Lo que no está acá todavía no
se decidió, y se marca como «por definir» en lugar de inventarlo.

Dial: ENERGY 2 / RHYTHM 2 / MOTION 3

## Personalidad

Nostálgica de videojuegos: la app hace guiños a la era de las consolas y a los menús de los juegos,
con más carácter visual que un reproductor genérico.

## Dials

- **ENERGY 2 (equilibrada):** algunos momentos tienen presencia, como el álbum abierto y el
  reproductor; el resto se mantiene sobrio.
- **RHYTHM 2 (con algunos quiebres):** las pantallas comparten una base consistente, y algunas se
  destacan, como el álbum abierto y el visualizador.
- **MOTION 3 (coreografía):** las animaciones tienen protagonismo en varias partes de la app.

## Paleta

- Se mantienen los tres temas incluidos: Oscuro, Claro y Catppuccin Mocha.
- **Oscuro** es el tema predeterminado.
- Cada tema cumple WCAG AA. El test `builtin_themes_contrast` de `src/theme.rs` lo verifica.

## Tipografía

Inter, por la razón escrita en el README: está pensada para pantallas, se lee bien entre 11 y 15 px
(el rango que más usa la app) y cubre los acentos de los títulos del catálogo.

## Motivo de identidad

Ventanas de diálogo de RPG: un borde grueso por fuera y uno fino por dentro, como las cajas de texto
de los juegos de consola. Enmarca el título de cada sección y el panel del álbum abierto. En el código
es `dialog_box` (`src/main.rs`).

En el álbum abierto, el fondo de esa caja es un degradado pixel art con los colores principales de la
portada: celdas cuadradas de 8 px, 7 escalones de color y tramado Bayer entre escalones, como en los
juegos de 8 y 16 bits. El degradado se mezcla con el fondo del tema lo justo para que el texto pase
4.5:1 en cada celda. Sobre él, los textos secundarios usan el color de texto principal. El código está
en `src/palette.rs`.

## Movimiento (MOTION 3)

- **Cursor que se desliza:** el resaltado de la entrada activa del menú lateral se desliza hasta la
  entrada nueva al cambiar de sección.
- **Tarjetas escalonadas:** al cargar una lista, las portadas aparecen una tras otra con un fundido y
  un leve desplazamiento hacia arriba.
- **Transición de vista:** al abrir un álbum, volver a la lista o cambiar de sección, el contenido
  entra con un fundido y un leve desplazamiento hacia arriba.
- La opción «Reducir animaciones» de Opciones apaga todas estas animaciones, para quien el movimiento le
  molesta.
