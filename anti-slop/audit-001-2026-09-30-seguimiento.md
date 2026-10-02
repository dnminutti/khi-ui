# Seguimiento de la auditoría 001 (2026-09-30)

Se aprobaron los once hallazgos y los once quedaron atendidos. El 6 se cerró con la dirección que diste, transcrita en `DESIGN.md`.

Verificación: `mise run build` pasa (9 tests). Cada cambio visual se revisó con capturas renderizadas
por `egui_kittest` en los temas Claro y Catppuccin Mocha, con tests temporales que después se borraron.

| # | Regla | Estado | Qué se hizo |
|---|---|---|---|
| 1 | R-32 | Arreglado | Nuevo `focus_ring` en `src/main.rs`: anillo de 2 px del color de texto alrededor de cada control pintado a mano (menú, tarjetas, filas de pista, ♥, ⬇, botones del reproductor, Reproducir y Donar). En los widgets de egui, el estilo `active` (que egui usa para el foco) tiene borde de acento. Un clic con el mouse no da foco, así que el anillo aparece solo con Tab. Captura: el anillo rodea el botón de reproducir después de tres Tab. |
| 2 | R-25 | Arreglado | Tema Claro: `muted` pasa de `#6b7280` a `#5b6270` y `accent` de `#2563eb` a `#1d4ed8`. Todos los pares medidos quedan en 4.85:1 o más. |
| 3 | R-27 | Arreglado | Nuevo `error_state`: frase simple («No se pudo conectar con KHInsider. Revisa tu conexión a internet.»), el detalle técnico debajo en chico y un botón «Reintentar» que vuelve a pedir la lista, el álbum o el catálogo. La barra del reproductor usa la misma frase. Cada tema tiene ahora su propio rojo de error (`Theme::error`), porque el rojo de egui no pasaba sobre blanco. |
| 4 | R-27 | Arreglado | La carga muestra el spinner con texto («Cargando álbumes…», «Cargando el álbum…», «Cargando el catálogo…»). Los estados vacíos explican el caso: favoritos dice cómo agregar uno, la búsqueda repite el término y la biblioteca dice cómo descargar. Con la lista vacía ya no aparece un «0» suelto. |
| 5 | R-25 (no textual) | Arreglado | Nuevo `Theme::border`: los botones, desplegables y campos tienen un borde de 3:1 o más contra el fondo en los tres temas. |
| 6 | R-37 | Arreglado | `DESIGN.md` transcribe tus respuestas: personalidad nostálgica de videojuegos, dials ENERGY 2 / RHYTHM 2 / MOTION 3, los tres temas con **Oscuro** como predeterminado (ya aplicado en `src/theme.rs`) e Inter confirmada. Quedan por definir el motivo de identidad y la coreografía de MOTION 3: hoy la app tiene menos movimiento que el dial declarado. |
| 7 | R-04 | Arreglado | «Álbumes favoritos» usa 💿 en lugar de ★: los dos favoritos se marcan con el mismo ♥, y el ícono del menú dice qué guarda cada lista. La razón quedó escrita en el código. |
| 8 | R-06 | Arreglado | El README explica por qué Inter: está pensada para pantallas, se lee bien entre 11 y 15 px (el rango que más usa la app) y cubre los acentos de los títulos. Confirmada en `DESIGN.md`. |
| 9 | R-19 | Arreglado | El título del reproductor se corta con «…» y se desplaza solo con el mouse encima, siempre desde el principio. Ya no se mueve solo mientras suena la pista. |
| 10 | core Parte 3 | Arreglado | El párrafo de la pantalla Donar pasa a color de texto; el acento queda en el botón «Donar», que es el foco de esa pantalla. |
| 11 | C-4 | Arreglado | «1 pista» en singular. |

## Verificación que queda fuera del test

- El orden de Tab lo decide egui según el orden en que se dibujan los paneles: primero el reproductor,
  después la barra lateral y al final el contenido. Funciona, pero no sigue el orden visual de arriba
  hacia abajo.
- Un test nuevo, `builtin_themes_contrast` en `src/theme.rs`, falla si un tema incluido deja de
  cumplir los mínimos de contraste. Los temas propios del usuario no pasan por ese chequeo.
