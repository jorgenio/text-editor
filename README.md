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
- Pestañas, división vertical de paneles (Ctrl+\\) con **divisor
  arrastrable** (reparto a medida, mínimo 80 px por panel) y foco por
  panel con el ratón.
- Selección múltiple: Alt+click, Ctrl+D (siguiente coincidencia) y
  Ctrl+Alt+↑/↓ (cursores en columna).
- Buscar/reemplazar con resaltado incremental (Enter/F3 navegan; Ctrl+Enter
  reemplaza; Ctrl+Shift+Enter reemplaza todo).
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
- Barra de estado inferior con Ln/Col (columna visual), selección,
  tabulado, lenguaje y nombre del archivo; zoom de fuente con
  Ctrl+= / Ctrl+- / Ctrl+0 (solo sesión); Ctrl+Shift+T reabre la última
  pestaña cerrada.
- Preferencias en `settings.json` con comentarios (JSONC) que se aplican al
  guardar el archivo; el tamaño de la ventana se recuerda en `session.json`
  (estado, no preferencia).
- Tema claro/oscuro con Ctrl+T (sesión) o persistente desde preferencias.
- **Barra de título propia con menú embebido** (sin marco del sistema):
  menús Archivo/Editar/Ver/Preferencias (Alt los abre, Escape los cierra),
  arrastre de la ventana con doble clic para maximizar, botones
  minimizar/maximizar/cerrar y bordes redimensionables en los 4 lados y
  las esquinas.
- **Buscar en archivos** (Ctrl+Shift+F): panel de resultados al pie con
  `ruta:línea: texto`, smart-case (mayúsculas activan distinguir
  mayúsculas), hasta 1000 coincidencias y límites para no colgar en
  binarios o archivos enormes.

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
| Abrir preferencias | Ctrl+, |

**Selección múltiple**: Alt+click añade un cursor; Ctrl+Alt+↑/↓ añade
cursores arriba/abajo; doble click selecciona la palabra y triple la línea.

**Dentro de la barra de buscar/reemplazar**: Enter o F3 van a la siguiente
coincidencia (con Shift, a la anterior), ↑/↓ también navegan, Tab salta de
buscar a reemplazar (con el modo reemplazo activo), Ctrl+Enter reemplaza la
actual, Ctrl+Shift+Enter reemplaza todas y Esc cierra.

**Dentro del panel de buscar en archivos**: la consulta se recoge de la
selección de una línea si la hay; Enter busca (o, con la búsqueda ya
hecha, abre la coincidencia seleccionada), ↑/↓ recorren los resultados y
Esc cierra. Click en una fila abre el archivo en esa línea; click fuera
del panel lo cierra y sigue su curso hasta el editor. Buscar (Ctrl+F) y
Buscar en archivos se excluyen mutuamente.

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
    "theme": "dark"
}
```

El cambio de tema con Ctrl+T es **solo de sesión**: para que perviva,
escribe `"theme": "light"` en el archivo y guárdalo. La paleta de comandos
y los avisos se pintan siempre en tono oscuro por encima del tema activo.

Con dos paneles, el **divisor** (filete vertical) se arrastra con el ratón
para repartir el ancho; el reparto se recuerda para el siguiente Ctrl+\.
Al cerrar la ventana se guarda su tamaño en `session.json` (junto a
`settings.json`; es estado de sesión, no una preferencia editable).

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
