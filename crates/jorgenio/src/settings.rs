//! Preferencias del usuario en `settings.json` (JSONC: se permiten
//! comentarios `//` y `/* */` y comas finales), cargadas al arrancar y
//! recargadas al guardar el archivo (o con Ctrl+, / "Abrir preferencias").
//!
//! Ubicación: `%APPDATA%\jorgenio\settings.json` en Windows;
//! `$XDG_CONFIG_HOME/jorgenio/settings.json` (o `~/.config/jorgenio/…`)
//! en el resto. Un archivo ilegible o con errores NO rompe el arranque:
//! se avisa por log y se usan los defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Preferencias aplicables. Los campos desconocidos se ignoran y los
/// ausentes toman el default: un archivo de otra versión no rompe nada.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ancho de tabulador en celdas (1-32): caret, selección y glifos.
    pub tab_width: usize,
    /// ¿Dibujar los números de línea en el gutter?
    pub show_line_numbers: bool,
    /// Parpadeo del caret en ms (50-10000).
    pub caret_blink_ms: u64,
    /// Tamaño de la fuente en px del editor y la barra (8-72).
    pub font_size: u32,
    /// ¿Tabulador escribe espacios en vez de `\t`? (también afecta al
    /// indentar/desindentar selecciones).
    pub translate_tabs_to_spaces: bool,
    /// ¿Auto-cerrar pares de paréntesis/comillas? (escribe el cierre y
    /// salta por delante; Backspace entre el par borra los dos).
    pub auto_match: bool,
    /// Tema de colores: "dark" (Monokai, por defecto) o "light".
    pub theme: String,
    /// ¿Ajustar las líneas largas al ancho del panel (word wrap)? Las
    /// líneas se parten en varias filas de pantalla sin tocar el buffer.
    pub word_wrap: bool,
    /// ¿Mostrar la franja de vista previa lateral (minimap, M27)? Se
    /// alterna también desde «Ver → Minimap» y la paleta (Ctrl+M).
    pub minimap: bool,
    /// Tareas de build (M28, Ctrl+B): la primera cuya `ext` encaje con el
    /// archivo activo (o la primera comodín, sin `ext`) se lanza en un
    /// shell con su salida en el panel inferior.
    pub builds: Vec<BuildTask>,
}

/// Tarea de compilación/ejecución definida en `settings.json`: `cmd` corre
/// en un shell (`cmd /C` en Windows, `sh -c` en Linux) con `working_dir`
/// como directorio de trabajo.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct BuildTask {
    /// Nombre mostrado en el panel de salida.
    pub name: String,
    /// Extensiones que activan esta tarea, en minúsculas; vacío = comodín
    /// (sirve para cualquier archivo; se usa si ninguna otra encaja).
    pub ext: Vec<String>,
    /// Comando completo del shell (argumentos incluidos).
    pub cmd: String,
    /// Directorio de trabajo; vacío = el del archivo activo (o el de la
    /// aplicación si el archivo no está guardado).
    pub working_dir: String,
}

impl Default for BuildTask {
    fn default() -> Self {
        Self {
            name: "build".to_owned(),
            ext: Vec::new(),
            cmd: String::new(),
            working_dir: String::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            show_line_numbers: true,
            caret_blink_ms: 530,
            font_size: 14,
            translate_tabs_to_spaces: false,
            auto_match: true,
            theme: "dark".to_owned(),
            word_wrap: false,
            minimap: true,
            builds: Vec::new(),
        }
    }
}

impl Settings {
    /// Tarea de build para el archivo `path` (M28): la primera cuya `ext`
    /// contenga la extensión del archivo (sin distinguir mayúsculas); si
    /// ninguna encaja, la primera comodín (`ext` vacío). Sin candidatas →
    /// `None` (Ctrl+B avisa en vez de fallar en silencio).
    pub fn build_for(&self, path: Option<&Path>) -> Option<&BuildTask> {
        let ext = path
            .and_then(|path| path.extension())
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase);
        if let Some(ext) = ext
            && let Some(task) = self.builds.iter().find(|task| task.ext.contains(&ext))
        {
            return Some(task);
        }
        self.builds.iter().find(|task| task.ext.is_empty())
    }
}

/// Directorio de configuración de Jorgenio (`JORGENIO_CONFIG_DIR` manda,
/// para tests y modo portable).
fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JORGENIO_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("jorgenio")
}

/// Ruta completa de `settings.json`.
pub fn path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Quita los comentarios de un texto JSONC preservando los strings (una
/// `//` dentro de comillas es texto) y los saltos de línea (para no
/// descuadrar los mensajes de error de serde_json).
fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                // Escape: el siguiente char es literal (\" no cierra).
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    for c2 in chars.by_ref() {
                        if c2 == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next(); // consume '*'
                    let mut prev = '\0';
                    loop {
                        match chars.next() {
                            Some('/') if prev == '*' => break,
                            Some('\n') => {
                                out.push('\n');
                                prev = '\n';
                            }
                            Some(c2) => prev = c2,
                            None => break, // sin cerrar: se traga hasta EOF
                        }
                    }
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    out
}

/// Se traga las comas finales: `,` seguido solo de blancos y `}` o `]`.
/// (Los comentarios ya están fuera; los strings se respetan: `",}"` es dato.)
fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1; // coma final: fuera
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// JSONC → JSON plano (comentarios y comas finales fuera).
pub fn jsonc_to_json(input: &str) -> String {
    strip_trailing_commas(&strip_comments(input))
}

/// Parsea el contenido de `settings.json`; errores y valores fuera de rango
/// se normalizan (tab 1-32, blink 50-10000 ms, fuente 8-72 px, tema →
/// "dark"/"light").
pub fn parse(text: &str) -> Result<Settings, String> {
    let json = jsonc_to_json(text);
    let mut settings: Settings = serde_json::from_str(&json).map_err(|err| err.to_string())?;
    settings.tab_width = settings.tab_width.clamp(1, 32);
    settings.caret_blink_ms = settings.caret_blink_ms.clamp(50, 10_000);
    settings.font_size = settings.font_size.clamp(8, 72);
    // Solo existen dos temas: cualquier valor desconocido vuelve al oscuro.
    settings.theme = settings.theme.trim().to_lowercase();
    if settings.theme != "light" {
        settings.theme = "dark".to_owned();
    }
    // Tareas de build: sin comando no se lanza nada y las extensiones se
    // normalizan a minúsculas (la comparación no distingue mayúsculas).
    settings.builds.retain(|task| !task.cmd.trim().is_empty());
    for task in &mut settings.builds {
        task.name = task.name.trim().to_owned();
        if task.name.is_empty() {
            task.name = "build".to_owned();
        }
        task.cmd = task.cmd.trim().to_owned();
        task.working_dir = task.working_dir.trim().to_owned();
        for ext in &mut task.ext {
            *ext = ext.trim().trim_start_matches('.').to_lowercase();
        }
        task.ext.retain(|ext| !ext.is_empty());
    }
    Ok(settings)
}

fn read_settings(target: &Path) -> Settings {
    match std::fs::read_to_string(target) {
        Ok(text) => match parse(&text) {
            Ok(settings) => settings,
            Err(err) => {
                log::warn!("{} ilegible ({err}); usando defaults", target.display());
                Settings::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(err) => {
            log::warn!("no se pudo leer {}: {err}", target.display());
            Settings::default()
        }
    }
}

/// Carga `settings.json` (si falta o falla → defaults, nunca pánic).
pub fn load() -> Settings {
    read_settings(&path())
}

/// Plantilla JSONC inicial: se crea con Ctrl+, y documenta cada opción.
pub fn default_template() -> &'static str {
    r#"// Preferencias de Jorgenio Text (settings.json)
// Edita y guarda con Ctrl+S: los cambios se aplican al guardar.
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
"#
}

fn write_default(target: &Path) {
    if target.exists() {
        return;
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(err) = std::fs::write(target, default_template()) {
        log::warn!("no se pudo crear {}: {err}", target.display());
    }
}

/// Crea `settings.json` con la plantilla si no existe (Ctrl+,).
pub fn ensure_exists() {
    write_default(&path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comments_keeps_strings_and_newlines() {
        // Comentario de línea fuera, pero NO el que está dentro del string.
        let jsonc = "// arriba\n{\"url\": \"http://x // no es comentario\"}";
        let json = strip_comments(jsonc);
        assert!(!json.contains("// arriba"));
        assert!(json.contains("// no es comentario"), "string intacto");
        assert!(json.contains('\n'), "se conservan los saltos de línea");

        // Escape de comillas: la \" no cierra el string.
        let escaped = r#"{"s": "a\"//b"}"#;
        assert_eq!(strip_comments(escaped), escaped);

        // Comentario de bloque multilínea (mantiene los \n de dentro) y el
        // sin cerrar se traga hasta EOF.
        let block = "a /* uno\ndos */ b /* sin cerrar";
        assert_eq!(strip_comments(block), "a \n b ");
    }

    #[test]
    fn strip_trailing_commas_respects_strings() {
        assert_eq!(strip_trailing_commas("{\"a\": 1,}"), "{\"a\": 1}");
        assert_eq!(
            strip_trailing_commas("{\"a\": [1, 2, ]}"),
            "{\"a\": [1, 2 ]}"
        );
        // Una coma SEGUIDA de otro valor no es final.
        assert_eq!(
            strip_trailing_commas("{\"a\": 1,\"b\": 2}"),
            "{\"a\": 1,\"b\": 2}"
        );
        // Dentro de un string, ",}" es dato.
        assert_eq!(strip_trailing_commas("{\"s\": \",}\"}"), "{\"s\": \",}\"}");
    }

    #[test]
    fn parse_defaults_overrides_and_errors() {
        assert_eq!(parse("{}").unwrap(), Settings::default());
        let partial = parse("{\"tab_width\": 8}").unwrap();
        assert_eq!(partial.tab_width, 8);
        assert!(partial.show_line_numbers, "los ausentes son default");
        assert_eq!(partial.caret_blink_ms, 530);
        assert_eq!(partial.font_size, 14, "ausente → tamaño por defecto");
        // Campo desconocido: se ignora (archivos de versiones futuras).
        assert!(parse("{\"no_existe\": true}").is_ok());
        // Contenido que no es JSON → error, no pánic.
        assert!(parse("").is_err());
        assert!(parse("{esto no es json}").is_err());
    }

    #[test]
    fn parse_normalizes_out_of_range_values() {
        let low = parse("{\"tab_width\": 0, \"caret_blink_ms\": 1, \"font_size\": 1}").unwrap();
        assert_eq!(low.tab_width, 1, "0 partiría la aritmética de columnas");
        assert_eq!(low.caret_blink_ms, 50, "parpadeo con suelo");
        assert_eq!(low.font_size, 8, "fuente con suelo");
        let high =
            parse("{\"tab_width\": 999, \"caret_blink_ms\": 999999, \"font_size\": 999}").unwrap();
        assert_eq!(high.tab_width, 32);
        assert_eq!(high.caret_blink_ms, 10_000);
        assert_eq!(high.font_size, 72, "fuente con techo");
    }

    #[test]
    fn translate_tabs_to_spaces_defaults_off_and_parses() {
        assert!(
            !Settings::default().translate_tabs_to_spaces,
            "por defecto Tab escribe \\t"
        );
        assert!(
            parse("{\"translate_tabs_to_spaces\": true}")
                .unwrap()
                .translate_tabs_to_spaces,
            "se puede activar"
        );
    }

    #[test]
    fn auto_match_defaults_on_and_parses() {
        assert!(Settings::default().auto_match, "por defecto los pares van");
        assert!(
            !parse("{\"auto_match\": false}").unwrap().auto_match,
            "se puede desactivar"
        );
    }

    #[test]
    fn word_wrap_defaults_off_and_parses() {
        assert!(!Settings::default().word_wrap, "por defecto no se ajusta");
        assert!(
            parse("{\"word_wrap\": true}").unwrap().word_wrap,
            "se puede activar"
        );
        assert!(
            !parse("{\"word_wrap\": false}").unwrap().word_wrap,
            "y desactivar"
        );
    }

    #[test]
    fn minimap_defaults_on_and_parses() {
        assert!(Settings::default().minimap, "por defecto el minimap va");
        assert!(
            !parse("{\"minimap\": false}").unwrap().minimap,
            "se puede apagar"
        );
        assert!(parse("{\"minimap\": true}").unwrap().minimap, "y encender");
    }

    #[test]
    fn builds_default_empty_and_template_keeps_it() {
        assert!(
            Settings::default().builds.is_empty(),
            "sin tareas: Ctrl+B avisa"
        );
        assert!(
            parse(default_template())
                .expect("plantilla")
                .builds
                .is_empty()
        );
    }

    #[test]
    fn build_for_matches_extension_then_wildcard_then_none() {
        let settings = parse(
            r#"{"builds": [
                {"name": "rust", "ext": ["RS"], "cmd": "cargo check"},
                {"name": "todo", "ext": [], "cmd": "make"}
            ]}"#,
        )
        .expect("json");
        assert_eq!(
            settings
                .build_for(Some(Path::new("a.rs")))
                .map(|t| t.name.as_str()),
            Some("rust"),
            "la extensión encaja sin distinguir mayúsculas"
        );
        assert_eq!(
            settings
                .build_for(Some(Path::new("a.py")))
                .map(|t| t.name.as_str()),
            Some("todo"),
            "ninguna encaja → comodín"
        );
        assert_eq!(
            settings.build_for(None).map(|t| t.name.as_str()),
            Some("todo"),
            "archivo sin guardar → comodín"
        );
        assert!(
            parse("{}")
                .unwrap()
                .build_for(Some(Path::new("a.rs")))
                .is_none(),
            "sin tareas → None"
        );
    }

    #[test]
    fn build_tasks_without_cmd_are_dropped_and_exts_normalized() {
        let settings = parse(
            r#"{"builds": [
                {"name": "  ", "ext": [], "cmd": "   "},
                {"name": "", "ext": [".RS", " Py "], "cmd": " run "}
            ]}"#,
        )
        .expect("json");
        assert_eq!(settings.builds.len(), 1, "las tareas sin comando se van");
        let task = &settings.builds[0];
        assert_eq!(task.name, "build", "nombre vacío → «build»");
        assert_eq!(task.cmd, "run", "comando recortado");
        assert_eq!(task.ext, vec!["rs", "py"], "exts en minúsculas y limpias");
    }

    #[test]
    fn parse_normalizes_theme_values() {
        assert_eq!(parse("{}").unwrap().theme, "dark", "ausente → oscuro");
        assert_eq!(
            parse("{\"theme\": \"light\"}").unwrap().theme,
            "light",
            "light se acepta"
        );
        assert_eq!(
            parse("{\"theme\": \"LIGHT\"}").unwrap().theme,
            "light",
            "sin importar mayúsculas"
        );
        assert_eq!(
            parse("{\"theme\": \"azul\"}").unwrap().theme,
            "dark",
            "desconocido → oscuro"
        );
        assert_eq!(Settings::default().theme, "dark");
    }

    #[test]
    fn default_template_parses_back_to_defaults() {
        let parsed = parse(default_template()).expect("la plantilla es JSONC válido");
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn load_missing_file_and_bad_file_fall_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("settings.json");
        assert_eq!(read_settings(&missing), Settings::default(), "sin archivo");

        std::fs::write(&missing, "{ mal").expect("escribe");
        assert_eq!(read_settings(&missing), Settings::default(), "JSON malo");
    }

    #[test]
    fn ensure_exists_writes_template_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("settings.json");

        write_default(&target);
        assert!(target.exists(), "crea el archivo");
        assert_eq!(read_settings(&target), Settings::default());

        // La segunda llamada no pisa un archivo ya editado.
        std::fs::write(&target, "{\"tab_width\": 8}").expect("edita");
        write_default(&target);
        assert_eq!(read_settings(&target).tab_width, 8, "no lo sobreescribe");
    }
}
