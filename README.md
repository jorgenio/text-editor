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
- Preferencias en `settings.json` con comentarios (JSONC) que se aplican al
  guardar el archivo; el tamaño de la ventana se recuerda en `session.json`
  (estado, no preferencia).
- Tema claro/oscuro con Ctrl+T (sesión) o persistente desde preferencias.

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
| Archivo nuevo | Ctrl+N |
| Abrir archivo | Ctrl+O |
| Guardar | Ctrl+S |
| Guardar como… | Ctrl+Shift+S |
| Cerrar pestaña | Ctrl+W |
| Deshacer / Rehacer | Ctrl+Z / Ctrl+Shift+Z (o Ctrl+Y) |
| Copiar / Cortar / Pegar | Ctrl+C / Ctrl+X / Ctrl+V |
| Seleccionar todo | Ctrl+A |
| Seleccionar siguiente coincidencia | Ctrl+D |
| Borrar línea | Ctrl+Shift+K |
| Duplicar línea | Ctrl+Shift+D |
| Comentar / destacar línea | Ctrl+/ |
| Comentar bloque | Ctrl+Shift+/ |
| Buscar / Reemplazar | Ctrl+F / Ctrl+H |
| Dividir panel / Unir paneles | Ctrl+\\ / Ctrl+Shift+\\ |
| Cambiar tema claro/oscuro | Ctrl+T |
| Abrir preferencias | Ctrl+, |

**Selección múltiple**: Alt+click añade un cursor; Ctrl+Alt+↑/↓ añade
cursores arriba/abajo; doble click selecciona la palabra y triple la línea.

**Dentro de la barra de buscar/reemplazar**: Enter o F3 van a la siguiente
coincidencia (con Shift, a la anterior), ↑/↓ también navegan, Tab salta de
buscar a reemplazar (con el modo reemplazo activo), Ctrl+Enter reemplaza la
actual, Ctrl+Shift+Enter reemplaza todas y Esc cierra.

**Movimiento**: flechas, Home/End, PageUp/PageDown, Backspace/Delete, Tab
(expande snippet o inserta sangría).

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
