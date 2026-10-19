# Jorgenio Text

Editor de texto tipo Sublime Text escrito en Rust, con interfaz en español.
Ventana, GPU y texto rasterizado con `winit` + `wgpu` + `glyphon`; buffer en
`ropey`; resaltado de sintaxis con `syntect`. Multiplataforma: Windows y
Linux (la CI compueba ambos).

## Características

- Buffer en cuerda (rope) con undo/redo, guardar/Guardar como (diálogos
  nativos) y aviso efímero al guardar.
- Resaltado de sintaxis por gramática (Rust, Python, JS… + texto plano) con
  dos paletas: **Monokai** (oscuro, por defecto) e **InspiredGitHub** (claro).
- Pestañas y **división de paneles** (Ctrl+\\): el menú «Ver» cambia el
  reparto (una columna, dos columnas, dos filas o **rejilla 2×2**) y sus
  **divisores arrastrables** reparten el alto y el ancho a medida
  (mínimo 80 px por panel y 62 px por fila); foco por panel con el ratón
  y **× de cierre** en cada pestaña (visible en la activa o bajo el
  ratón, con el cursor en forma de mano sobre ella).
- Selección múltiple: Alt+click, Ctrl+D (siguiente coincidencia) y
  Ctrl+Alt+↑/↓ (cursores en columna).
- Buscar/reemplazar con resaltado incremental (Enter/F3 navegan; Ctrl+Enter
  reemplaza; Ctrl+Shift+Enter reemplaza todo) y **modo regex** con el botón
  «.*»: smart-case también en regex, `$1`/`$name` en el reemplazo, `^`/`$`
  por línea y aviso si el patrón no compila.
- Paleta de comandos (Ctrl+Shift+P) y búsqueda rápida de archivos (Ctrl+P)
  con fuzzy matching.
- Comentarios por lenguaje (Ctrl+/) y snippets con Tab (for, if, fn…).
- Movimiento tipo Sublime: palabras con Ctrl+←/→, Ctrl+Backspace/Delete
  borra palabras, Home inteligente (alterna columna 0 ↔ primer no-blanco),
  Ctrl+L expande a la línea (repetible), Ctrl+J une líneas y
  Ctrl+Shift+↑/↓ mueve la línea entera.
- Tabulación: Tab/Shift+Tab indentan o desindantan la selección (una
  nivel por pulsación); Enter arrastra el sangrado de la línea; la
  preferencia `translate_tabs_to_spaces` hace que Tab escriba espacios.
- Auto-match de pares (por defecto activo): escribir `(`, `[`, `{`, `"`
  o `'` inserta el cierre y deja el caret dentro; escribir el cierre con
  el par ya completo salta por delante; Backspace entre el par borra los
  dos lados.
- **Ajuste de línea** (preferencia `word_wrap`, por defecto desactivado):
  las líneas largas se parten en varias filas de pantalla sin tocar el
  buffer. El número de línea solo aparece en la primera fila, ↑/↓ y
  PageUp/PageDown recorren las filas visibles conservando la columna, y
  caret, scroll, selecciones y búsqueda comparten esa misma geometría.
- **Minimap** (preferencia `minimap`, por defecto activo): franja de vista
  previa a la derecha de cada panel con el código en miniatura (2 px por
  línea, con los colores de sintaxis) y un rectángulo con la ventana
  visible; el click centra en esa línea y arrastrarlo hace scroll (sin
  robar el foco, como la rueda). Se alterna con Ctrl+M o con «Ver →
  Minimap»; los paneles estrechos (menos de 280 px) se quedan sin franja
  para no tapar código.
- **Espacios visibles y recorte al guardar** (M29): con `show_whitespace`
  activo (o «Preferencias → Espacios y tabuladores») cada espacio se dibuja
  como `·` y cada tabulador como `→`, también con ajuste de línea; con
  `trim_on_save`, guardar recorta los espacios/tabuladores finales de cada
  línea en el buffer como UNA sola operación de deshacer (ambos apagados
  por defecto; ver [preferencias](#preferencias-settingsjson)).
- **Builds** (Ctrl+B o «Herramientas → Ejecutar build»): la tarea definida
  para la extensión del archivo actual en `builds` de `settings.json` corre
  en un hilo aparte (la ventana no se congela) y su salida aparece en un
  panel al pie —stderr en rojo, rueda para recorrer, Esc lo cierra—; las
  filas con `ruta:línea` se pulsan para abrir el archivo en esa línea. Sin
  tareas definidas avisa en vez de fallar en silencio.
- Barra de estado inferior con Ln/Col (columna visual), selección,
  tabulado, lenguaje y nombre del archivo; zoom de fuente con
  Ctrl+= / Ctrl+- / Ctrl+0 (solo sesión); Ctrl+Shift+T reabre la última
  pestaña cerrada.
- Preferencias en `settings.json` con comentarios (JSONC) que se aplican al
  guardar el archivo; el tamaño de la ventana se recuerda en `session.json`
  (estado, no preferencia).
- **Hot exit**: al cerrar la ventana, los buffers sin guardar se guardan en
  `session.json` (máx. 20 pestañas y 1 MiB por buffer) y se restauran en el
  próximo arranque como sucios, antes que los archivos pasados por la CLI.
- Tema claro/oscuro con Ctrl+T (sesión) o persistente desde preferencias:
  una paleta **inspirada en Navicat** (acento `#5B8DEF`/`#2F6FED`) cubre
  toda la interfaz —editor, barras, pestañas, paletas y avisos—, la
  pestaña activa se funde con el editor y se apaga cuando su panel pierde
  el foco; la sintaxis conserva sus paletas Monokai/InspiredGitHub.
- **Barra de título propia con menú embebido** (sin marco del sistema):
  menús Archivo/Editar/Ver/Preferencias/Herramientas (Alt los abre,
  Escape los cierra),
  arrastre de la ventana con doble clic para maximizar, botones
  minimizar/maximizar/cerrar y bordes redimensionables en los 4 lados y
  las esquinas; mientras un menú está desplegado, las etiquetas de
  pestaña que quedan debajo no se dibujan (sus textos van por encima de
  todos los fondos).
- **Buscar en archivos** (Ctrl+Shift+F): panel de resultados al pie con
  `ruta:línea: texto`, smart-case (mayúsculas activan distinguir
  mayúsculas), hasta 1000 coincidencias y límites para no colgar en
  binarios o archivos enormes.
- **Archivos grandes y encodings raros**: guardar vuelca el texto por
  trozos (sin materializarlo entero), buscar copia el texto una sola vez
  por revisión en vez de una por tecla, y un archivo que no es UTF-8 se
  abre con `U+FFFD` en los bytes inválidos y marcado como sucio, para que
  el «●» avise de que guardar reemplazaría esos bytes (ver
  [Archivos grandes](#archivos-grandes)).

## Requisitos

- Rust stable con edition 2024 (1.99 o superior).
- **Windows**: Visual Studio Build Tools con la workload "C++ build tools"
  y el Windows SDK (los mismos que necesita cualquier crate con `winapi`).
- **Linux**: entorno gráfico X11 o Wayland con sus librerías habituales
  (xkbcommon, fontconfig…). La CI solo compila: `cargo build` funciona en un
  runner estándar sin dependencias extra.

## Compilar y ejecutar

```sh
# Desarrollo
cargo run

# Release (binario en target/release/)
cargo run --release

# Abrir archivos concretos al arrancar
cargo run --release -- nota.md main.rs
```

Atajo de la ruta de preferencias: la variable de entorno
`JORGENIO_CONFIG_DIR` cambia dónde se lee/escribe `settings.json`
(modo portable y tests).

## Atajos de teclado

Todos los atajos usan **Ctrl** (en macOS: Cmd). AltGr no dispara comandos.

| Acción | Atajo |
| --- | --- |
| Paleta de comandos | Ctrl+Shift+P |
| Búsqueda rápida de archivos | Ctrl+P |
| Abrir "archivo:12" en la línea 12 | Ctrl+P → `ruta:12` |
| Archivo nuevo | Ctrl+N |
| Abrir archivo | Ctrl+O |
| Guardar | Ctrl+S |
| Guardar como… | Ctrl+Shift+S |
| Cerrar pestaña | Ctrl+W |
| Reabrir pestaña cerrada | Ctrl+Shift+T |
| Deshacer / Rehacer | Ctrl+Z / Ctrl+Shift+Z (o Ctrl+Y) |
| Copiar / Cortar / Pegar | Ctrl+C / Ctrl+X / Ctrl+V |
| Seleccionar todo | Ctrl+A |
| Seleccionar siguiente coincidencia | Ctrl+D |
| Palabra izquierda / derecha | Ctrl+← / Ctrl+→ |
| Borrar palabra | Ctrl+Backspace / Ctrl+Delete |
| Expandir selección a la línea | Ctrl+L |
| Expandir selección al par | Ctrl+Shift+A |
| Unir con la línea siguiente | Ctrl+J |
| Mover línea arriba / abajo | Ctrl+Shift+↑ / Ctrl+Shift+↓ |
| Borrar línea | Ctrl+Shift+K |
| Duplicar línea | Ctrl+Shift+D |
| Comentar / destacar línea | Ctrl+/ |
| Comentar bloque | Ctrl+Shift+/ |
| Buscar / Reemplazar | Ctrl+F / Ctrl+H |
| Buscar en archivos | Ctrl+Shift+F |
| Dividir panel / Unir paneles | Ctrl+\\ / Ctrl+Shift+\\ |
| Cambiar tema claro/oscuro | Ctrl+T |
| Zoom de fuente (+ / − / normal) | Ctrl+= / Ctrl+- / Ctrl+0 |
| Alternar minimap | Ctrl+M |
| Ejecutar build | Ctrl+B |
| Abrir preferencias | Ctrl+, |

**Selección múltiple**: Alt+click añade un cursor; Ctrl+Alt+↑/↓ añade
cursores arriba/abajo; doble click selecciona la palabra y triple la línea.
Ctrl+Shift+A crece la selección al par de corchetes `()[]{}` que la
envuelve (vacío → su contenido, contenido → englobando los corchetes, y
así sucesivamente hacia fuera; con varios cursores cada uno crece solo).

**Dentro de la barra de buscar/reemplazar**: Enter o F3 van a la siguiente
coincidencia (con Shift, a la anterior), ↑/↓ también navegan, Tab salta de
buscar a reemplazar (con el modo reemplazo activo), Ctrl+Enter reemplaza la
actual, Ctrl+Shift+Enter reemplaza todas y Esc cierra. El botón «.*» a la
derecha alterna la búsqueda por regex (se conserva al cerrar la barra; un
patrón inválido muestra «Regex inválida» en vez del contador; en el
reemplazo, `$1`…`$9` expanden los captures de cada coincidencia).

**Dentro del panel de buscar en archivos**: la consulta se recoge de la
selección de una línea si la hay; Enter busca (o, con la búsqueda ya
hecha, abre la coincidencia seleccionada), ↑/↓ recorren los resultados y
Esc cierra. Click en una fila abre el archivo en esa línea; click fuera
del panel lo cierra y sigue su curso hasta el editor. Buscar (Ctrl+F) y
Buscar en archivos se excluyen mutuamente.

**Dentro del panel de build**: la rueda lo recorre, Esc lo cierra sin robar
el resto de las teclas al editor, click en una fila con `ruta:línea` abre
el archivo en esa línea y click en la cabecera también lo cierra. El panel
de build y el de buscar en archivos compiten por el mismo pie (lanzar uno
cierra el otro), mientras que la barra de buscar/reemplazar flota por
encima y recibe los clicks primero.

**Movimiento**: flechas, Home/End (Home alterna columna 0 ↔ primer
no-blanco), PageUp/PageDown, Ctrl+←/→ por palabras, Backspace/Delete,
Ctrl+Backspace/Delete borra palabras, Tab (expande snippet, con selección
indenta; Shift+Tab desindenta) y Enter (arrastra el sangrado).

## Preferencias (`settings.json`)

Ubicación: `%APPDATA%\jorgenio\settings.json` en Windows,
`$XDG_CONFIG_HOME/jorgenio/settings.json` (o `~/.config/jorgenio/…`) en el
resto. Ctrl+, crea el archivo con comentarios y lo abre; se vuelve a leer
cada vez que lo guardas con Ctrl+S. Los campos desconocidos se ignoran y
los fuera de rango se recortan.

```jsonc
{
    // Ancho de tabulador en celdas (1-32).
    "tab_width": 4,

    // ¿Mostrar los números de línea en el gutter?
    "show_line_numbers": true,

    // Parpadeo del caret en milisegundos (50-10000).
    "caret_blink_ms": 530,

    // Tamaño de la fuente en píxeles del editor (8-72).
    "font_size": 14,

    // ¿Tabulador escribe espacios (true) o carácter de tabulación (false)?
    "translate_tabs_to_spaces": false,

    // ¿Auto-cerrar pares: () [] {} "" ''?
    "auto_match": true,

    // Tema de colores: "dark" (Monokai) o "light" (fondo blanco).
    "theme": "dark",

    // ¿Ajustar las líneas largas al ancho del panel (word wrap)?
    "word_wrap": false,

    // ¿Mostrar la franja de vista previa lateral (minimap)?
    "minimap": true,

    // ¿Dibujar los espacios (·) y tabuladores (→) del código?
    "show_whitespace": false,

    // ¿Recortar espacios/tabuladores finales de cada línea al guardar?
    // (una sola transacción de deshacer; por defecto apagado)
    "trim_on_save": false,

    // Tareas de build (Ctrl+B): se lanzan en un shell (cmd /C o sh -c) con
    // «working_dir» como directorio de trabajo (vacío = el del archivo
    // activo). «ext» empareja con la extensión del archivo abierto (vacío
    // = comodín); gana la primera que encaje.
    // "builds": [
    //     { "name": "cargo check", "ext": ["rs"],
    //       "cmd": "cargo check", "working_dir": "" }
    // ],
    "builds": []
}
```

El cambio de tema con Ctrl+T es **solo de sesión**: para que perviva,
escribe `"theme": "light"` en el archivo y guárdalo. La paleta de comandos
y los avisos se pintan siempre en tono oscuro por encima del tema activo.

El reparto de paneles lo decide el menú **Ver** (una columna, dos
columnas, dos filas o rejilla 2×2; crece con editores en blanco y funde
los sobrantes en el primer grupo). Sus **divisores** se arrastran con el
ratón: el vertical reparte el ancho y el horizontal la altura, y cada
reparto se recuerda para el siguiente Ctrl+\. Al cerrar la ventana se
guarda su tamaño en `session.json` (junto a `settings.json`; es estado de
sesión, no una preferencia editable), junto a los buffers sin guardar del
hot exit.

## Archivos grandes

El diseño apunta a que un archivo de **decenas de megabytes** se abra, edite
y guarde sin sorpresas. Con un equipo de 8 GB los límites prácticos son,
aproximadamente:

- **≤ 50 MB**: uso cómodo (apertura, edición, buscar/reemplazar).
- **~100 MB**: tirones al teclear en la búsqueda: cada revisión nueva vuelve
  a copiar el texto completo para la caché de `find`, y esa copia ocupa lo
  mismo que el archivo mientras la barra está abierta.
- **≥ 1 GB**: no es viable en 8 GB (la cuerda, el resaltado y la caché de
  búsqueda conviven en memoria); se abre, pero la experiencia no es la de un
  editor.

Cómo se consigue: guardar escribe `rope.chunks()` sobre un `BufWriter` en
vez de construir un `String` con todo el texto; `find` materializa el texto
una sola vez por revisión y lo suelta al cerrar la barra; y cargar un
archivo que no es UTF-8 no lo abre vacío, sino con `U+FFFD` en los bytes
inválidos y el buffer sucio, así el «●» recuerda que guardar reemplazaría
esos bytes.

## Desarrollo

Workspace con cuatro crates:

| Crate | Contenido |
| --- | --- |
| `jorgenio-core` | Buffer (rope), historial, movimiento y selección |
| `jorgenio-syntax` | Resaltado syntect conmutable (Monokai / InspiredGitHub) |
| `jorgenio-ui` | Ventana, GPU (wgpu), capa de texto (glyphon) y escena |
| `jorgenio` | El binario: editor, pestañas, splits, paleta, búsqueda, settings |

Comandos de verificación (los mismos que la CI en Windows y Ubuntu):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

La CI (`.github/workflows/ci.yml`) ejecuta esos tres pasos en `push` a
`main` y en pull requests.

## Licencia

Bajo licencia dual **MIT OR Apache-2.0** (ver `Cargo.toml`).
