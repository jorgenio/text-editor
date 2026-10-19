//! Jorgenio Text — binario principal: ventana + event loop + orquestación.

mod commands;
mod editor;
mod findfiles;
mod minimap;
mod palette;
mod search;
mod session;
mod settings;
mod snippets;
mod tabs;
mod theme;
mod title;

use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::Editor;
use findfiles::FindFiles;
use jorgenio_ui::gpu::GpuContext;
use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use palette::Palette;
use search::SearchBar;
use tabs::{Divider, TAB_BAR_H, TabHit, Tabs};
use theme::Theme;
use title::TitleBar;
use winit::dpi::LogicalPosition;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window};

/// Ventana de doble/triple click (misma "tanda" de clicks).
const CLICK_WINDOW: Duration = Duration::from_millis(450);
/// Tolerancia en px para considerar "el mismo punto" entre clicks.
const CLICK_SLOP2: f32 = 16.0; // (4 px)^2
/// Umbral de movimiento para que un click se convierta en arrastre.
const DRAG_SLOP2: f32 = 16.0;
/// Vida del aviso efímero (guardado, errores) en la esquina superior.
const NOTICE_TTL: Duration = Duration::from_secs(3);
/// Altura de la barra de estado inferior (px lógicos).
const STATUS_H: f32 = 24.0;
/// Tope de archivos recogidos por el Ctrl+P (un monorepo no debe colgar).
const QUICK_OPEN_MAX: usize = 20_000;
/// Directorios ignorados al recorrer el árbol del Ctrl+P.
const QUICK_SKIP: [&str; 9] = [
    ".git",
    "target",
    "node_modules",
    ".vs",
    ".vscode",
    "build",
    "dist",
    "__pycache__",
    ".idea",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let event_loop = EventLoop::new()?;
    // La sesión se carga UNA vez: el tamaño de la ventana en `resumed` y el
    // hot exit (buffers sin guardar) en `Application::new`.
    let session = session::load();
    event_loop.run_app(&mut Application::new(session, editor::cli_paths()))?;
    Ok(())
}

struct Application {
    /// Una pestaña por archivo (M9); la activa es la que recibe los eventos.
    tabs: Tabs,
    /// Paleta de comandos (Ctrl+Shift+P).
    palette: Palette,
    /// Barra de buscar/reemplazar (Ctrl+F / Ctrl+H).
    search: SearchBar,
    /// Buscar en archivos (Ctrl+Shift+F) con panel de resultados.
    findfiles: FindFiles,
    /// Preferencias cargadas de `settings.json` (M11).
    settings: settings::Settings,
    /// Sesión del arranque (tamaño de ventana; los buffers ya se
    /// restauraron en `boot_tabs`).
    session: session::Session,
    gpu: Option<GpuContext>,
    /// Portapapeles del SO, creado en la primera uso (lazy).
    clipboard: Option<arboard::Clipboard>,
    modifiers: winit::keyboard::ModifiersState,
    last_title: String,
    cursor: LogicalPosition<f32>,
    caret_blink_deadline: Instant,
    /// Aviso efímero (texto + cuándo caduca).
    notice: Option<(String, Instant)>,
    /// Clicks para doble/triple click.
    last_click: Option<(Instant, LogicalPosition<f32>)>,
    click_count: u32,
    mouse_down: bool,
    drag_origin: Option<LogicalPosition<f32>>,
    drag_active: bool,
    /// Arrastre del divisor activo: Some(eje, última coordenada en ese eje).
    divider_drag: Option<(Divider, f32)>,
    /// Arrastre del minimap activo (M27): Some(grupo, `first` congelado).
    minimap_drag: Option<(usize, usize)>,
    /// Icono actual del cursor (para no pisarlo en cada movimiento).
    cursor_icon: CursorIcon,
    /// Barra de título propia con menú embebido (M21, sin decoraciones).
    titlebar: TitleBar,
}

impl Application {
    fn new(session: session::Session, paths: Vec<std::path::PathBuf>) -> Self {
        let settings = settings::load();
        let mut tabs = boot_tabs(&session, paths);
        tabs.apply_prefs(&settings);
        let mut search = SearchBar::default();
        search.set_theme(Theme::from_name(&settings.theme));
        let blink = blink_interval(&settings);
        Self {
            tabs,
            palette: Palette::default(),
            search,
            findfiles: FindFiles::default(),
            settings,
            session,
            gpu: None,
            clipboard: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            last_title: String::new(),
            cursor: LogicalPosition::new(0.0, 0.0),
            caret_blink_deadline: Instant::now() + blink,
            notice: None,
            last_click: None,
            click_count: 0,
            mouse_down: false,
            drag_origin: None,
            drag_active: false,
            divider_drag: None,
            minimap_drag: None,
            cursor_icon: CursorIcon::Default,
            titlebar: TitleBar::default(),
        }
    }
}

/// Pestañas al arrancar (M23): primero el hot exit de la sesión (los
/// buffers sucios, en su orden), luego los paths de la CLI que no estén ya
/// restaurados (un path repetido no duplica pestañas: lo no-guardado manda
/// sobre el disco). Sin nada, un editor en blanco.
fn boot_tabs(session: &session::Session, cli: Vec<std::path::PathBuf>) -> Tabs {
    let mut list: Vec<Editor> = session
        .buffers
        .iter()
        .map(|buffer| {
            Editor::from_hot(
                buffer.path.as_deref().map(std::path::PathBuf::from),
                &buffer.text,
            )
        })
        .collect();
    for path in cli {
        if !list
            .iter()
            .any(|editor| editor.path() == Some(path.as_path()))
        {
            list.push(Editor::from_cli(path));
        }
    }
    Tabs::from_editors(list)
}

/// Pestañas sin guardar de TODOS los grupos, para el hot exit al cerrar.
fn collect_hot_buffers(tabs: &Tabs) -> Vec<session::HotBuffer> {
    tabs.iter()
        .filter(|editor| editor.dirty())
        .map(|editor| session::HotBuffer {
            path: editor.path().map(|p| p.to_string_lossy().into_owned()),
            text: editor.text(),
        })
        .collect()
}

/// Valor actual de una preferencia booleana (para el aviso del toggle).
fn field_value(settings: &settings::Settings, field: &str) -> bool {
    match field {
        "show_line_numbers" => settings.show_line_numbers,
        "translate_tabs_to_spaces" => settings.translate_tabs_to_spaces,
        "auto_match" => settings.auto_match,
        "minimap" => settings.minimap,
        _ => false,
    }
}

/// Duración del parpadeo del caret según preferencias (con suelo: un valor
/// diminuto recargaría la escena en bucle).
fn blink_interval(settings: &settings::Settings) -> Duration {
    Duration::from_millis(settings.caret_blink_ms.clamp(50, 10_000))
}

/// Portapapeles lazy (se crea en el primer uso).
fn clipboard_slot(slot: &mut Option<arboard::Clipboard>) -> Option<&mut arboard::Clipboard> {
    if slot.is_none() {
        match arboard::Clipboard::new() {
            Ok(clipboard) => *slot = Some(clipboard),
            Err(err) => {
                log::warn!("portapapeles no disponible: {err}");
                return None;
            }
        }
    }
    slot.as_mut()
}

fn copy_selection(editor: &Editor, slot: &mut Option<arboard::Clipboard>) {
    let Some(text) = editor.copy() else {
        return;
    };
    if let Some(clipboard) = clipboard_slot(slot) {
        match clipboard.set_text(text) {
            Ok(()) => log::info!("copiado al portapapeles"),
            Err(err) => log::warn!("no se pudo copiar: {err}"),
        }
    }
}

fn cut_selection(editor: &mut Editor, slot: &mut Option<arboard::Clipboard>) {
    let Some(text) = editor.cut() else {
        return;
    };
    if let Some(clipboard) = clipboard_slot(slot) {
        match clipboard.set_text(text) {
            Ok(()) => log::info!("cortado al portapapeles"),
            Err(err) => log::warn!("no se pudo cortar: {err}"),
        }
    }
}

fn paste_clipboard(editor: &mut Editor, slot: &mut Option<arboard::Clipboard>) {
    let Some(text) = clipboard_slot(slot).and_then(|clipboard| clipboard.get_text().ok()) else {
        return;
    };
    editor.insert_text(&text);
}

/// Nombre sugerido para "Guardar como…": añade `.txt` si el nombre no tiene
/// ya una extensión.
fn default_save_name(label: &str) -> String {
    if label.contains('.') {
        label.to_owned()
    } else {
        format!("{label}.txt")
    }
}

/// Diálogo "Abrir" (modal nativo; bloquea el event loop). Devuelve varios
/// archivos (uno por pestaña). En Linux usa xdg-portal vía rfd.
fn open_dialog() -> Option<Vec<std::path::PathBuf>> {
    let files = pollster::block_on(
        rfd::AsyncFileDialog::new()
            .set_title("Abrir archivo")
            .add_filter(
                "Texto",
                &[
                    "txt", "rs", "py", "js", "ts", "json", "md", "toml", "yaml", "yml", "html",
                    "css", "c", "h", "cpp", "go", "sh",
                ],
            )
            .add_filter("Todos los archivos", &["*"])
            .pick_files(),
    )?;
    let paths: Vec<_> = files.iter().map(|file| file.path().to_path_buf()).collect();
    (!paths.is_empty()).then_some(paths)
}

/// Diálogo "Guardar como…" con `default_name` precargado (sin filtros: el
/// usuario decide la extensión real del archivo).
fn save_dialog(default_name: &str) -> Option<std::path::PathBuf> {
    pollster::block_on(
        rfd::AsyncFileDialog::new()
            .set_title("Guardar como…")
            .set_file_name(default_name)
            .save_file(),
    )
    .map(|file| file.path().to_path_buf())
}

/// Mueve el foco a otro grupo (panel), conservando el estado de `focused`
/// de la ventana: el editor que lo pierde queda apagado para que su caret no
/// se pinte ni parpadee.
fn focus_group(tabs: &mut Tabs, group: usize) {
    if group == tabs.active_group_index() {
        return;
    }
    let focused = tabs.active().focused();
    tabs.active_mut().set_focused(false);
    tabs.set_active_group(group);
    tabs.active_mut().set_focused(focused);
}

/// Como [`focus_group`] pero también cambia de pestaña (click en la barra).
fn focus_tab(tabs: &mut Tabs, group: usize, tab: usize) {
    if (group, tab) == (tabs.active_group_index(), tabs.active_index()) {
        return;
    }
    let focused = tabs.active().focused();
    if group != tabs.active_group_index() {
        tabs.active_mut().set_focused(false);
        tabs.set_active_group(group);
    }
    tabs.set_active(tab);
    tabs.active_mut().set_focused(focused);
}

/// Si se guardó el `settings.json`, recarga preferencias y las reaplica a
/// TODOS los editores abiertos (tab/gutter/parpadeo/tema se ven al instante).
fn reload_settings_if_needed(
    settings: &mut settings::Settings,
    tabs: &mut Tabs,
    search: &mut SearchBar,
    saved: &std::path::Path,
) {
    if saved != settings::path() {
        return;
    }
    *settings = settings::load();
    tabs.apply_prefs(settings);
    search.set_theme(Theme::from_name(&settings.theme));
    log::info!("preferencias recargadas");
}

/// Fija el aviso efímero que se pinta en la esquina superior derecha.
fn notify(slot: &mut Option<(String, Instant)>, text: String) {
    *slot = Some((text, Instant::now()));
}

/// Acción elegida en el menú de la barra de título: comando o toggle.
/// `true` = pide redraw.
#[allow(clippy::too_many_arguments)]
fn apply_menu_action(
    tabs: &mut Tabs,
    clipboard: &mut Option<arboard::Clipboard>,
    palette: &mut Palette,
    search: &mut SearchBar,
    findfiles: &mut FindFiles,
    settings: &mut settings::Settings,
    notice: &mut Option<(String, Instant)>,
    action: title::Action,
) -> bool {
    match action {
        title::Action::Command(id) => run_command(
            tabs, clipboard, palette, search, findfiles, settings, notice, id,
        ),
        title::Action::Toggle(field) => {
            match field {
                "show_line_numbers" => settings.show_line_numbers = !settings.show_line_numbers,
                "translate_tabs_to_spaces" => {
                    settings.translate_tabs_to_spaces = !settings.translate_tabs_to_spaces;
                }
                "auto_match" => settings.auto_match = !settings.auto_match,
                "minimap" => settings.minimap = !settings.minimap,
                _ => return false,
            }
            tabs.apply_prefs(settings);
            notify(
                notice,
                format!("{field} = {}", field_value(settings, field)),
            );
            true
        }
        title::Action::Layout(columns, rows) => {
            tabs.set_layout(columns, rows);
            notify(notice, format!("layout {columns}×{rows}"));
            true
        }
        title::Action::Separator => false,
    }
}

/// Área de contenido entre la barra de título y la barra de estado, donde se
/// reparten las celdas de los paneles (M24): `(content_top, content_h)`. La
/// altura incluye las barras de pestañas de cada fila.
fn content_area(viewport_h: f32) -> (f32, f32) {
    let top = title::TITLE_H;
    let editor_h = (viewport_h - title::TITLE_H - TAB_BAR_H - STATUS_H).max(50.0);
    (top, editor_h + TAB_BAR_H)
}

/// "archivo.rs:12" → Some(12) (goto de línea del Ctrl+P); sin sufijo
/// numérico claro → None.
fn parse_goto_line(query: &str) -> Option<usize> {
    let (path, line) = query.rsplit_once(':')?;
    if path.is_empty() || line.is_empty() || !line.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    line.parse::<usize>().ok().filter(|n| *n >= 1)
}

/// Archivos del árbol de `root` para el Ctrl+P (sin directorios ruidosos
/// y con tope: caminar un monorepo entero colgaría la interfaz).
fn collect_files(root: &std::path::Path, max: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= max {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if file_type.is_dir() {
                // Sin symlinks no hay ciclos: file_type no sigue enlaces.
                if name.to_str().is_some_and(|n| QUICK_SKIP.contains(&n)) {
                    continue;
                }
                stack.push(entry.path());
            } else if file_type.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// Abre la búsqueda rápida (Ctrl+P) sobre el directorio de trabajo.
fn open_quickopen(palette: &mut Palette) {
    let root = std::env::current_dir().unwrap_or_default();
    let files = collect_files(&root, QUICK_OPEN_MAX);
    if files.is_empty() {
        log::warn!("sin archivos bajo {}", root.display());
    }
    palette.open_files(root, files);
}

/// Abre `path` como pestaña nueva o enfoca la que ya lo tiene.
/// (Un CLI con ruta relativa y el Ctrl+P con la absoluta pueden duplicar;
/// se prefiere duplicar a fallar el foco.)
fn open_path(tabs: &mut Tabs, settings: &mut settings::Settings, path: std::path::PathBuf) {
    if let Some((group, tab)) = tabs.find_path(&path) {
        focus_tab(tabs, group, tab);
    } else {
        tabs.add(Editor::from_cli(path));
        tabs.apply_prefs(settings);
    }
}

/// Ejecuta un comando por id (keymap o paleta de comandos).
/// `true` = pide redraw.
#[allow(clippy::too_many_arguments)]
fn run_command(
    tabs: &mut Tabs,
    clipboard: &mut Option<arboard::Clipboard>,
    palette: &mut Palette,
    search: &mut SearchBar,
    findfiles: &mut FindFiles,
    settings: &mut settings::Settings,
    notice: &mut Option<(String, Instant)>,
    id: &str,
) -> bool {
    match id {
        "palette.show" => {
            palette.open();
            true
        }
        "find.show" => {
            findfiles.close();
            let prefill = tabs.active().single_line_selection();
            search.open(false, prefill.as_deref(), tabs.active_mut());
            true
        }
        "replace.show" => {
            findfiles.close();
            let prefill = tabs.active().single_line_selection();
            search.open(true, prefill.as_deref(), tabs.active_mut());
            true
        }
        "find_in_files.show" => {
            search.close();
            let prefill = tabs.active().single_line_selection();
            let root = std::env::current_dir().unwrap_or_default();
            let files = collect_files(&root, QUICK_OPEN_MAX);
            findfiles.open(prefill.as_deref(), root, files);
            true
        }
        "file.new" => {
            tabs.add(Editor::default());
            tabs.apply_prefs(settings);
            true
        }
        "file.open" => {
            if let Some(paths) = open_dialog() {
                let first = tabs.len();
                for path in paths {
                    tabs.add(Editor::from_cli(path));
                }
                tabs.set_active(first);
                tabs.apply_prefs(settings);
            }
            true
        }
        "file.save" => {
            let saved: Option<std::path::PathBuf> = {
                let editor = tabs.active_mut();
                if editor.has_path() {
                    match editor.save() {
                        Ok(()) => {
                            log::info!("guardado");
                            let path = editor.path().map(|p| p.to_path_buf());
                            if let Some(path) = &path {
                                notify(
                                    notice,
                                    format!(
                                        "Guardado: {}",
                                        path.file_name()
                                            .unwrap_or(path.as_os_str())
                                            .to_string_lossy()
                                    ),
                                );
                            }
                            path
                        }
                        Err(err) => {
                            log::error!("error al guardar: {err}");
                            notify(notice, format!("Error al guardar: {err}"));
                            None
                        }
                    }
                } else {
                    let name = default_save_name(&editor.tab_label());
                    save_dialog(&name).and_then(|path| match editor.save_as(path.clone()) {
                        Ok(()) => {
                            log::info!("guardado");
                            notify(
                                notice,
                                format!(
                                    "Guardado: {}",
                                    path.file_name()
                                        .unwrap_or(path.as_os_str())
                                        .to_string_lossy()
                                ),
                            );
                            Some(path)
                        }
                        Err(err) => {
                            log::error!("error al guardar: {err}");
                            notify(notice, format!("Error al guardar: {err}"));
                            None
                        }
                    })
                }
            };
            if let Some(saved) = &saved {
                reload_settings_if_needed(settings, tabs, search, saved);
            }
            true
        }
        "file.save_as" => {
            let saved: Option<std::path::PathBuf> = {
                let editor = tabs.active_mut();
                let name = default_save_name(&editor.tab_label());
                save_dialog(&name).and_then(|path| match editor.save_as(path.clone()) {
                    Ok(()) => {
                        log::info!("guardado");
                        notify(
                            notice,
                            format!(
                                "Guardado: {}",
                                path.file_name()
                                    .unwrap_or(path.as_os_str())
                                    .to_string_lossy()
                            ),
                        );
                        Some(path)
                    }
                    Err(err) => {
                        log::error!("error al guardar: {err}");
                        notify(notice, format!("Error al guardar: {err}"));
                        None
                    }
                })
            };
            if let Some(saved) = &saved {
                reload_settings_if_needed(settings, tabs, search, saved);
            }
            true
        }
        "quickopen.show" => {
            open_quickopen(palette);
            true
        }
        "file.reopen_closed" => {
            let Some(path) = tabs.take_closed() else {
                notify(notice, "No hay pestañas cerradas".to_owned());
                return true;
            };
            open_path(tabs, settings, path);
            true
        }
        "file.close" => {
            if !tabs.close(None) {
                log::info!("la última pestaña no se cierra");
            }
            true
        }
        "edit.undo" => {
            tabs.active_mut().undo();
            true
        }
        "edit.redo" => {
            tabs.active_mut().redo();
            true
        }
        "edit.copy" => {
            copy_selection(tabs.active(), clipboard);
            true
        }
        "edit.cut" => {
            cut_selection(tabs.active_mut(), clipboard);
            true
        }
        "edit.paste" => {
            paste_clipboard(tabs.active_mut(), clipboard);
            true
        }
        "edit.select_all" => {
            tabs.active_mut().select_all();
            true
        }
        "edit.select_next_occurrence" => {
            tabs.active_mut().select_next_occurrence();
            true
        }
        "edit.delete_line" => {
            tabs.active_mut().delete_current_line();
            true
        }
        "edit.duplicate_line" => {
            tabs.active_mut().duplicate_current_line();
            true
        }
        "edit.expand_selection_line" => {
            tabs.active_mut().expand_to_line();
            true
        }
        "edit.join_lines" => {
            tabs.active_mut().join_lines();
            true
        }
        // Sin binding de tecla (flechas Named no entran al keymap): el
        // atajo Ctrl+Shift+↑/↓ vive en `editor.handle_key`.
        "edit.move_line_up" => {
            tabs.active_mut().move_line_up();
            true
        }
        "edit.move_line_down" => {
            tabs.active_mut().move_line_down();
            true
        }
        "edit.comment" => {
            tabs.active_mut().toggle_line_comment();
            true
        }
        "edit.comment_block" => {
            tabs.active_mut().toggle_block_comment();
            true
        }
        "view.split" => {
            tabs.split();
            // El editor en blanco del grupo nuevo nace con las preferencias.
            tabs.apply_prefs(settings);
            true
        }
        "view.unsplit" => {
            tabs.unsplit();
            true
        }
        "settings.open" => {
            let target = settings::path();
            settings::ensure_exists();
            if let Some((group, tab)) = tabs.find_path(&target) {
                focus_tab(tabs, group, tab);
            } else {
                tabs.add(Editor::from_cli(target));
            }
            // Aplica lo que ya estuviera en disco (ediciones externas).
            *settings = settings::load();
            tabs.apply_prefs(settings);
            search.set_theme(Theme::from_name(&settings.theme));
            true
        }
        "theme.toggle" => {
            settings.theme = if settings.theme == "light" {
                "dark".to_owned()
            } else {
                "light".to_owned()
            };
            tabs.apply_prefs(settings);
            let theme = Theme::from_name(&settings.theme);
            search.set_theme(theme);
            // Solo sesión: para persistir, escríbelo en settings.json.
            notify(notice, format!("Tema: {}", theme.label()));
            true
        }
        "view.zoom_in" => {
            settings.font_size = (settings.font_size + 1).min(72);
            tabs.apply_prefs(settings);
            notify(notice, format!("Fuente: {} px", settings.font_size));
            true
        }
        "view.zoom_out" => {
            settings.font_size = settings.font_size.saturating_sub(1).max(8);
            tabs.apply_prefs(settings);
            notify(notice, format!("Fuente: {} px", settings.font_size));
            true
        }
        "view.zoom_reset" => {
            settings.font_size = settings::Settings::default().font_size;
            tabs.apply_prefs(settings);
            notify(notice, format!("Fuente: {} px", settings.font_size));
            true
        }
        "minimap.toggle" => {
            settings.minimap = !settings.minimap;
            // Solo sesión: para persistir, escríbelo en settings.json.
            notify(
                notice,
                format!("minimap = {}", field_value(settings, "minimap")),
            );
            true
        }
        other => {
            log::warn!("comando desconocido: {other}");
            false
        }
    }
}

impl winit::application::ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        // Tamaño recordado de la sesión anterior (clampeado en Session::new).
        let attrs = Window::default_attributes()
            .with_title(self.tabs.active().title())
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.session.window_width,
                self.session.window_height,
            ));
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("no se pudo crear la ventana: {err}");
                event_loop.exit();
                return;
            }
        };

        let boot = Instant::now();
        let gpu = pollster::block_on(GpuContext::new(window, event_loop));
        gpu.set_ime_allowed(true);
        gpu.request_redraw();
        log::info!("ventana + GPU listos en {:?}", boot.elapsed());
        self.last_title = self.tabs.active().title();
        self.gpu = Some(gpu);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            return;
        }
        // El aviso efímero caduca solo; al desaparecer pide repintar.
        let mut wake: Option<Instant> = None;
        let notice_deadline = self.notice.as_ref().map(|(_, at)| *at + NOTICE_TTL);
        if let Some(deadline) = notice_deadline {
            if Instant::now() >= deadline {
                self.notice = None;
                if let Some(gpu) = &self.gpu {
                    gpu.request_redraw();
                }
            } else {
                wake = Some(deadline);
            }
        }
        if self.tabs.active().focused() {
            let now = Instant::now();
            if now >= self.caret_blink_deadline {
                // Si el caret está fuera de pantalla no hay nada que
                // parpadear: se conserva el estado y se ahorra el repintar
                // completo (escena + glifos + quads) cada 530 ms.
                if self.tabs.active_mut().caret_in_view() {
                    self.tabs.active_mut().toggle_caret();
                    if let Some(gpu) = &self.gpu {
                        gpu.request_redraw();
                    }
                }
                self.caret_blink_deadline = now + blink_interval(&self.settings);
            }
            wake = Some(wake.map_or(self.caret_blink_deadline, |w| {
                w.min(self.caret_blink_deadline)
            }));
        }
        match wake {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        match event {
            WindowEvent::Resized(size) => {
                gpu.resize(size);
            }
            WindowEvent::ScaleFactorChanged { .. } => gpu.request_redraw(),
            WindowEvent::CloseRequested => {
                // Recuerda el tamaño lógico y los buffers sin guardar (hot
                // exit; M23) para el próximo arranque (los errores solo se
                // loguean: cerrar manda).
                let size = gpu.window().inner_size();
                let scale = gpu.scale_factor();
                let mut session = session::Session::new(
                    (size.width as f64 / scale).round() as u32,
                    (size.height as f64 / scale).round() as u32,
                );
                session.set_buffers(collect_hot_buffers(&self.tabs));
                session::save(&session);
                event_loop.exit();
            }
            WindowEvent::Focused(focused) => {
                self.tabs.active_mut().set_focused(focused);
                gpu.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                // El ratón sale de la ventana: sin hover (la × se apaga).
                let before = self.tabs.tab_at_pointer();
                self.tabs.set_pointer(-1.0, -1.0);
                if before != self.tabs.tab_at_pointer() {
                    gpu.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let frame = Instant::now();
                let viewport = gpu.logical_size();
                let advance = gpu.advance_ratio();
                // Preferencias → glifos (no-op si el ancho no cambió).
                gpu.set_tab_width(self.settings.tab_width as u16);
                // Cada celda es un grupo de pestañas; el reparto (columnas y
                // filas) lo deciden los divisores arrastrables.
                let (content_top, content_h) = content_area(viewport.1);
                let geometry = self.tabs.group_geometry(viewport.0, content_top, content_h);
                for (g, &(x, y, w, h)) in geometry.iter().enumerate() {
                    self.tabs.set_group_geometry(g, x, y, w, h);
                }
                // Une las escenas de todos los grupos: sus celdas son
                // disjuntas, así que el orden entre paneles es indiferente.
                let mut scene: Option<Scene> = None;
                for (g, &(_, _, w, h)) in geometry.iter().enumerate() {
                    let editor_h = (h - TAB_BAR_H).max(50.0);
                    let part = self
                        .tabs
                        .group_active_mut(g)
                        .build_scene((w, editor_h), advance);
                    match &mut scene {
                        Some(all) => all.append(part),
                        None => scene = Some(part),
                    }
                }
                let mut scene = scene.expect("siempre hay al menos un grupo");
                // Minimap (M27): franja sobre el código de SU celda (capa
                // superior); las celdas estrechas se quedan sin franja.
                if self.settings.minimap {
                    for (g, &cell) in geometry.iter().enumerate() {
                        if let Some(rect) = minimap::rect(cell) {
                            self.tabs.group_active_mut(g).draw_minimap(&mut scene, rect);
                        }
                    }
                }
                // Tema del frame, calculado UNA vez y compartido por la barra
                // de estado, los avisos, el menú de título y la paleta (M26).
                let theme = Theme::from_name(&self.settings.theme);
                let advance_title = self.settings.font_size as f32 * advance;
                // Puntero para el hover de la barra (× de cierre) y rect del
                // menú abierto: sus etiquetas NO se dibujan debajo (los texts
                // van por encima de todos los quads).
                self.tabs.set_pointer(self.cursor.x, self.cursor.y);
                let occluded =
                    self.titlebar
                        .drop_geometry(advance_title, &self.settings, self.tabs.layout());
                self.tabs.set_occluded(occluded);
                // Barras de pestañas y divisores entre celdas (M24).
                self.tabs
                    .render(&mut scene, viewport, advance_title, content_top, content_h);
                // Barra de estado inferior: Ln/Col, selección, tabulado,
                // lenguaje y archivo; se pinta PRIMERO para que la barra de
                // búsqueda y el panel de resultados floten sobre ella.
                {
                    let status = self.tabs.active().status_text();
                    let char_w = editor::FONT_SIZE * advance;
                    let text_w = status.chars().count() as f32 * char_w;
                    let y = viewport.1 - STATUS_H;
                    scene
                        .top_quads
                        .push(Quad::new(0.0, y, viewport.0, STATUS_H, theme.bar_bg));
                    scene
                        .top_quads
                        .push(Quad::new(0.0, y, viewport.0, 1.0, theme.tab_edge));
                    scene.top_texts.push(TextSpan::new(
                        status,
                        (viewport.0 - text_w - 12.0).max(8.0),
                        y + (STATUS_H - editor::FONT_SIZE) / 2.0,
                        editor::FONT_SIZE,
                        theme.dim,
                    ));
                }
                self.search.render(
                    &mut scene,
                    viewport,
                    editor::FONT_SIZE * advance,
                    STATUS_H,
                    self.tabs.active(),
                );
                self.findfiles.render(
                    &mut scene,
                    viewport,
                    editor::FONT_SIZE * advance,
                    STATUS_H,
                    &theme,
                );
                // Aviso efímero en la esquina superior derecha (los de
                // paleta se pintan después: mandan sobre él).
                if let Some((text, at)) = &self.notice
                    && at.elapsed() < NOTICE_TTL
                {
                    let char_w = editor::FONT_SIZE * advance;
                    let max_chars = ((viewport.0 - 48.0) / char_w).floor().max(4.0) as usize;
                    let mut text = text.clone();
                    if text.chars().count() > max_chars {
                        text.truncate(text.char_indices().nth(max_chars - 1).map_or(0, |(i, _)| i));
                        text.push('…');
                    }
                    let width = text.chars().count() as f32 * char_w + 24.0;
                    let height = 26.0;
                    let x = viewport.0 - width - 12.0;
                    let y = title::TITLE_H + TAB_BAR_H + 8.0;
                    scene
                        .top_quads
                        .push(Quad::new(x, y, width, height, theme.field_bg));
                    scene
                        .top_quads
                        .push(Quad::new(x, y, width, 1.0, theme.panel_edge));
                    scene.top_texts.push(TextSpan::new(
                        text,
                        x + 12.0,
                        y + (height - editor::FONT_SIZE) / 2.0,
                        editor::FONT_SIZE,
                        theme.fg,
                    ));
                }
                // Barra de título con menú (capa superior, debajo de la
                // paleta: su desplegado se pinta antes).
                {
                    let title = self.tabs.active().title();
                    self.titlebar.render(
                        &mut scene,
                        viewport,
                        advance_title,
                        &theme,
                        &title,
                        &self.settings,
                        self.tabs.layout(),
                    );
                }
                self.palette
                    .render(&mut scene, viewport, editor::FONT_SIZE * advance, &theme);
                let built = frame.elapsed();
                gpu.render(&scene);
                // Diagnóstico de rendimiento: el primer frame incluye la
                // carga de syntect; el resto debe ser <16 ms de CPU.
                let cpu = frame.elapsed();
                if cpu > Duration::from_millis(16) {
                    log::warn!("frame lenta (CPU): build={built:?} total={cpu:?}");
                }
                let title = self.tabs.active().title();
                if title != self.last_title {
                    gpu.set_title(&title);
                    self.last_title = title;
                }
                self.caret_blink_deadline = Instant::now() + blink_interval(&self.settings);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                // Menú de la barra de título: Alt lo abre (como en
                // Windows) y Escape lo cierra.
                if key_event.state == ElementState::Pressed {
                    if matches!(key_event.logical_key, Key::Named(NamedKey::Alt))
                        && self.modifiers.is_empty()
                    {
                        self.titlebar.open_first();
                        gpu.request_redraw();
                        return;
                    }
                    if matches!(key_event.logical_key, Key::Named(NamedKey::Escape))
                        && self.titlebar.is_open()
                    {
                        self.titlebar.close();
                        gpu.request_redraw();
                        return;
                    }
                }
                // Atajos del keymap (combinaciones exactas: ni mods extra
                // ni de menos). AltGr (Ctrl+Alt) no llega aquí: sin binding
                // con alt coincide.
                if key_event.state == ElementState::Pressed
                    && (self.modifiers.control_key() || self.modifiers.super_key())
                    && (matches!(key_event.logical_key, Key::Character(_))
                        || matches!(
                            key_event.logical_key,
                            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
                        ))
                    && let Some(id) = commands::command_for(self.modifiers, &key_event.logical_key)
                {
                    if self.palette.is_open() {
                        // Con la paleta abierta solo actúan los toggles; el
                        // resto de comandos se traga.
                        if id == "palette.show" {
                            self.palette.toggle();
                            gpu.request_redraw();
                        } else if id == "quickopen.show" {
                            open_quickopen(&mut self.palette);
                            gpu.request_redraw();
                        }
                    } else if run_command(
                        &mut self.tabs,
                        &mut self.clipboard,
                        &mut self.palette,
                        &mut self.search,
                        &mut self.findfiles,
                        &mut self.settings,
                        &mut self.notice,
                        id,
                    ) {
                        gpu.request_redraw();
                    }
                    return;
                }
                // Paleta abierta: su teclado manda (todo se traga).
                if self.palette.is_open() {
                    match self.palette.on_key(&key_event, self.modifiers) {
                        palette::Action::Run(id) => {
                            self.palette.close();
                            run_command(
                                &mut self.tabs,
                                &mut self.clipboard,
                                &mut self.palette,
                                &mut self.search,
                                &mut self.findfiles,
                                &mut self.settings,
                                &mut self.notice,
                                id,
                            );
                            gpu.request_redraw();
                        }
                        palette::Action::Open(path) => {
                            let goto = parse_goto_line(self.palette.query());
                            self.palette.close();
                            open_path(&mut self.tabs, &mut self.settings, path);
                            if let Some(line) = goto {
                                self.tabs.active_mut().goto_line(line);
                            }
                            gpu.request_redraw();
                        }
                        palette::Action::Close => {
                            self.palette.close();
                            gpu.request_redraw();
                        }
                        palette::Action::None => {}
                    }
                    return;
                }
                // Panel de buscar en archivos: su teclado manda (todo se
                // traga; Enter abre el seleccionado, Esc lo cierra).
                if self.findfiles.is_open() {
                    if key_event.state == ElementState::Pressed {
                        match self.findfiles.on_key(&key_event, self.modifiers) {
                            findfiles::Action::None => {}
                            findfiles::Action::Close => self.findfiles.close(),
                            findfiles::Action::Open(path, line) => {
                                open_path(&mut self.tabs, &mut self.settings, path);
                                self.tabs.active_mut().goto_line(line);
                            }
                        }
                        gpu.request_redraw();
                    }
                    return;
                }
                // Ctrl+Tab / Ctrl+Shift+Tab: ciclar pestañas. Es una tecla
                // Named (fuera del keymap de Character) y se resuelve ANTES
                // que la barra de búsqueda, que se traga el resto.
                if key_event.state == ElementState::Pressed
                    && (self.modifiers.control_key() || self.modifiers.super_key())
                    && !self.modifiers.alt_key()
                    && matches!(key_event.logical_key, Key::Named(NamedKey::Tab))
                {
                    let focused = self.tabs.active().focused();
                    self.tabs.cycle(self.modifiers.shift_key());
                    self.tabs.active_mut().set_focused(focused);
                    gpu.request_redraw();
                    return;
                }
                // Barra de búsqueda abierta: su teclado manda (todo se traga;
                // Enter/F3/flechas llegan aquí porque no son Character y los
                // Ctrl+Enter caen fuera del keymap).
                if self.search.is_open() {
                    if key_event.state == ElementState::Pressed {
                        self.search
                            .on_key(&key_event, self.modifiers, self.tabs.active_mut());
                        gpu.request_redraw();
                    }
                    return;
                }
                if self.tabs.active_mut().on_key(&key_event, self.modifiers) {
                    gpu.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = gpu.scale_factor();
                self.cursor = position.to_logical::<f32>(scale);
                // Hover de la barra de pestañas (× de cierre): repintar SOLO
                // al entrar o salir de una pestaña, no en cada movimiento.
                let hover_before = self.tabs.tab_at_pointer();
                self.tabs.set_pointer(self.cursor.x, self.cursor.y);
                if hover_before != self.tabs.tab_at_pointer() && !self.mouse_down {
                    gpu.request_redraw();
                }
                let width = gpu.logical_size().0;
                let height = gpu.logical_size().1;
                if let Some((axis, last)) = self.divider_drag {
                    // Arrastre del divisor: solo se mueve el filete en SU eje.
                    let (delta, next, extent) = match axis {
                        Divider::Vertical => (self.cursor.x - last, self.cursor.x, width),
                        Divider::Horizontal => {
                            (self.cursor.y - last, self.cursor.y, content_area(height).1)
                        }
                    };
                    self.divider_drag = Some((axis, next));
                    self.tabs.drag_divider(axis, delta, extent);
                    gpu.request_redraw();
                    return;
                }
                // Arrastre del minimap (M27): la ventana sigue al cursor con
                // el `first` congelado del click (los límites no saltan bajo
                // el ratón); no roba foco ni selecciona texto.
                if let Some((g, first)) = self.minimap_drag {
                    let (vw, vh) = gpu.logical_size();
                    let (ct, ch) = content_area(vh);
                    let cells = self.tabs.group_geometry(vw, ct, ch);
                    if let Some(rect) = minimap::rect(cells[g]) {
                        let view = self.tabs.group_active_mut(g).minimap_view(rect);
                        let line = minimap::line_at(
                            view.y_rel(self.cursor.y),
                            first,
                            view.total,
                            view.fit,
                        );
                        self.tabs.group_active_mut(g).scroll_to_center_line(line);
                    }
                    gpu.request_redraw();
                    return;
                }
                // Flechas de resize del marco propio y del divisor (con
                // overlays cerrados; durante un arrastre no se toca).
                if !self.mouse_down {
                    self.titlebar.hover_at(self.cursor.x, self.cursor.y);
                    let edge = title::resize_edge(self.cursor.x, self.cursor.y, width, height)
                        .map(CursorIcon::from);
                    let (ct, ch) = content_area(height);
                    let icon = if self.palette.is_open()
                        || self.search.is_open()
                        || self.findfiles.is_open()
                    {
                        CursorIcon::Default
                    } else if let Some(icon) = edge {
                        icon
                    } else if let Some(divider) =
                        self.tabs
                            .over_divider(self.cursor.x, self.cursor.y, width, ct, ch)
                    {
                        match divider {
                            Divider::Vertical => CursorIcon::EwResize,
                            Divider::Horizontal => CursorIcon::NsResize,
                        }
                    } else if !self.titlebar.is_open()
                        && matches!(
                            self.tabs.hit(self.cursor.x, self.cursor.y),
                            Some(TabHit::Close(..))
                        )
                    {
                        // Sobre la × de cierre de una pestaña (M26).
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    };
                    if icon != self.cursor_icon {
                        gpu.window().set_cursor(icon);
                        self.cursor_icon = icon;
                    }
                    if self.titlebar.is_open() {
                        gpu.request_redraw();
                    }
                }
                if self.mouse_down {
                    if !self.drag_active
                        && let Some(origin) = self.drag_origin
                    {
                        let dx = self.cursor.x - origin.x;
                        let dy = self.cursor.y - origin.y;
                        if dx * dx + dy * dy > DRAG_SLOP2 {
                            self.drag_active = true;
                        }
                    }
                    if self.drag_active {
                        let advance = self.settings.font_size as f32 * gpu.advance_ratio();
                        self.tabs
                            .active_mut()
                            .select_to(self.cursor.x, self.cursor.y, advance);
                        gpu.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let width = gpu.logical_size().0;
                let height = gpu.logical_size().1;
                // Borde de la ventana: resize (sin decoraciones nativas).
                if let Some(dir) = title::resize_edge(self.cursor.x, self.cursor.y, width, height) {
                    let _ = gpu.window().drag_resize_window(dir);
                    gpu.request_redraw();
                    return;
                }
                // Paleta (capa superior): fila → ejecutar; dentro del panel →
                // solo cerrar; fuera → cerrar y que el click siga al editor.
                if self.palette.is_open() {
                    match self.palette.click(self.cursor.x, self.cursor.y) {
                        palette::Click::Command(id) => {
                            self.palette.close();
                            run_command(
                                &mut self.tabs,
                                &mut self.clipboard,
                                &mut self.palette,
                                &mut self.search,
                                &mut self.findfiles,
                                &mut self.settings,
                                &mut self.notice,
                                id,
                            );
                            gpu.request_redraw();
                            return;
                        }
                        palette::Click::File(path) => {
                            let goto = parse_goto_line(self.palette.query());
                            self.palette.close();
                            open_path(&mut self.tabs, &mut self.settings, path);
                            if let Some(line) = goto {
                                self.tabs.active_mut().goto_line(line);
                            }
                            gpu.request_redraw();
                            return;
                        }
                        palette::Click::Panel => {
                            self.palette.close();
                            gpu.request_redraw();
                            return;
                        }
                        palette::Click::Miss => {
                            self.palette.close();
                        }
                    }
                }
                // Barra de búsqueda: campos y botones consumen el click;
                // fuera de la barra llega al editor (la barra se queda abierta).
                if self.search.is_open()
                    && self.search.handle_click(
                        self.cursor.x,
                        self.cursor.y,
                        self.tabs.active_mut(),
                    )
                {
                    gpu.request_redraw();
                    return;
                }
                // Panel de buscar en archivos: fila → abrir; dentro del
                // panel → tragar; fuera → cerrar y que el click siga.
                if self.findfiles.is_open() {
                    match self.findfiles.click(self.cursor.x, self.cursor.y) {
                        findfiles::Click::Row(path, line) => {
                            open_path(&mut self.tabs, &mut self.settings, path);
                            self.tabs.active_mut().goto_line(line);
                            gpu.request_redraw();
                            return;
                        }
                        findfiles::Click::Panel => {
                            gpu.request_redraw();
                            return;
                        }
                        findfiles::Click::Miss => self.findfiles.close(),
                    }
                }
                // Barra de título y desplegado del menú (por encima del
                // divisor y de las pestañas: ocupa la fila superior y su
                // menú se despliega sobre el contenido).
                if self.cursor.y < title::TITLE_H || self.titlebar.is_open() {
                    let (w_logical, h_logical) = gpu.logical_size();
                    match self
                        .titlebar
                        .click(self.cursor.x, self.cursor.y, w_logical, h_logical)
                    {
                        title::Click::Action(action) => {
                            self.titlebar.close();
                            apply_menu_action(
                                &mut self.tabs,
                                &mut self.clipboard,
                                &mut self.palette,
                                &mut self.search,
                                &mut self.findfiles,
                                &mut self.settings,
                                &mut self.notice,
                                action,
                            );
                            gpu.request_redraw();
                            return;
                        }
                        title::Click::Root(i) => {
                            self.titlebar.toggle_root(i);
                            gpu.request_redraw();
                            return;
                        }
                        title::Click::Minimize => {
                            gpu.window().set_minimized(true);
                            return;
                        }
                        title::Click::Maximize => {
                            let window = gpu.window();
                            window.set_maximized(!window.is_maximized());
                            return;
                        }
                        title::Click::Close => {
                            event_loop.exit();
                            return;
                        }
                        title::Click::Drag => {
                            // Arrastre de la ventana; doble clic → maximizar.
                            let mut maximize = false;
                            if let Some((at, point)) = self.last_click {
                                let dx = self.cursor.x - point.x;
                                let dy = self.cursor.y - point.y;
                                if at.elapsed() < CLICK_WINDOW && dx * dx + dy * dy <= CLICK_SLOP2 {
                                    maximize = true;
                                }
                            }
                            self.last_click = Some((Instant::now(), self.cursor));
                            if maximize {
                                let window = gpu.window();
                                window.set_maximized(!window.is_maximized());
                            } else {
                                let _ = gpu.window().drag_window();
                            }
                            gpu.request_redraw();
                            return;
                        }
                        title::Click::Miss => {
                            if self.titlebar.is_open() && self.cursor.y >= title::TITLE_H {
                                self.titlebar.close();
                                gpu.request_redraw();
                                return;
                            }
                            if self.cursor.y < title::TITLE_H {
                                return; // barra: no llega al editor
                            }
                        }
                    }
                }
                // Divisor entre paneles: el click lo inicia; no llega ni a
                // la barra ni al editor (el icono vuelve a flecha al soltar).
                let width = gpu.logical_size().0;
                let (ct, ch) = content_area(gpu.logical_size().1);
                if let Some(axis) =
                    self.tabs
                        .over_divider(self.cursor.x, self.cursor.y, width, ct, ch)
                {
                    let coord = match axis {
                        Divider::Vertical => self.cursor.x,
                        Divider::Horizontal => self.cursor.y,
                    };
                    self.divider_drag = Some((axis, coord));
                    self.mouse_down = false;
                    let icon = match axis {
                        Divider::Vertical => CursorIcon::EwResize,
                        Divider::Horizontal => CursorIcon::NsResize,
                    };
                    gpu.window().set_cursor(icon);
                    self.cursor_icon = icon;
                    gpu.request_redraw();
                    return;
                }
                // Barra de pestañas: seleccionar, o cerrar con su × (M26).
                // Sin arrastre ni caret: el click termina aquí.
                match self.tabs.hit(self.cursor.x, self.cursor.y) {
                    Some(TabHit::Select(group, tab)) => {
                        focus_tab(&mut self.tabs, group, tab);
                        gpu.request_redraw();
                        return;
                    }
                    Some(TabHit::Close(group, tab)) => {
                        // Se enfoca primero (mueve el grupo activo) y luego
                        // cierra: reutiliza "la última pestaña no se cierra".
                        focus_tab(&mut self.tabs, group, tab);
                        self.tabs.close(None);
                        gpu.request_redraw();
                        return;
                    }
                    None => {}
                }
                // Franja del minimap (M27): el click centra la ventana en esa
                // línea y el arrastre la mueve (sin robar el foco, como la
                // rueda del ratón).
                let cells = self.tabs.group_geometry(width, ct, ch);
                if self.settings.minimap
                    && let Some(g) = minimap::group_at(&cells, self.cursor.x, self.cursor.y)
                    && let Some(rect) = minimap::rect(cells[g])
                {
                    let view = self.tabs.group_active_mut(g).minimap_view(rect);
                    let line = minimap::line_at(
                        view.y_rel(self.cursor.y),
                        view.first,
                        view.total,
                        view.fit,
                    );
                    self.tabs.group_active_mut(g).scroll_to_center_line(line);
                    self.minimap_drag = Some((g, view.first));
                    self.mouse_down = false;
                    gpu.request_redraw();
                    return;
                }
                // Click en otro panel: ese grupo toma el foco (como Sublime)
                // y su editor activo coloca el caret.
                let group = self
                    .tabs
                    .group_at(self.cursor.x, self.cursor.y, width, ct, ch);
                focus_group(&mut self.tabs, group);
                // Doble/triple click: mismo punto dentro de CLICK_WINDOW.
                let mut count = 1;
                if let Some((at, point)) = self.last_click {
                    let dx = self.cursor.x - point.x;
                    let dy = self.cursor.y - point.y;
                    if at.elapsed() < CLICK_WINDOW && dx * dx + dy * dy <= CLICK_SLOP2 {
                        count = self.click_count + 1;
                    }
                }
                self.click_count = count;
                self.last_click = Some((Instant::now(), self.cursor));
                self.mouse_down = true;
                self.drag_origin = Some(self.cursor);
                self.drag_active = false;

                let advance = self.settings.font_size as f32 * gpu.advance_ratio();
                let shift = self.modifiers.shift_key();
                let active = self.tabs.active_mut();
                if self.modifiers.alt_key() && count == 1 {
                    active.add_cursor_at(self.cursor.x, self.cursor.y, advance);
                } else {
                    active.on_click(self.cursor.x, self.cursor.y, advance, count, shift);
                }
                gpu.request_redraw();
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if self.divider_drag.take().is_some() {
                    gpu.window().set_cursor(CursorIcon::Default);
                    self.cursor_icon = CursorIcon::Default;
                }
                self.minimap_drag = None;
                self.mouse_down = false;
                self.drag_active = false;
                self.drag_origin = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => {
                        y * editor::line_height_for(self.settings.font_size as f32) * 3.0
                    }
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                };
                // Rueda sobre el panel bajo el cursor (sin robar el foco, como
                // Sublime); si el cursor aún no se movió → grupo 0.
                let (vw, vh) = gpu.logical_size();
                let (ct, ch) = content_area(vh);
                let group = self.tabs.group_at(self.cursor.x, self.cursor.y, vw, ct, ch);
                self.tabs.group_active_mut(group).on_wheel(dy);
                gpu.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.palette.is_open() {
                    self.palette.push_text(&text);
                } else if self.findfiles.is_open() {
                    self.findfiles.push_text(&text);
                } else if self.search.is_open() {
                    self.search.push_text(&text, self.tabs.active_mut());
                } else {
                    self.tabs.active_mut().insert_text(&text);
                }
                gpu.request_redraw();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        boot_tabs, collect_files, collect_hot_buffers, default_save_name, focus_group, focus_tab,
        open_path, parse_goto_line, run_command,
    };
    use crate::editor::Editor;
    use crate::findfiles::FindFiles;
    use crate::palette::Palette;
    use crate::search::SearchBar;
    use crate::session;
    use crate::settings;
    use crate::tabs::Tabs;
    use crate::theme::Theme;
    use std::time::Instant;

    #[test]
    fn default_save_name_keeps_or_adds_extension() {
        assert_eq!(default_save_name("foo.rs"), "foo.rs");
        assert_eq!(default_save_name("sin título"), "sin título.txt");
    }

    #[test]
    fn parse_goto_line_reads_trailing_numbers() {
        assert_eq!(parse_goto_line("main.rs:12"), Some(12));
        assert_eq!(
            parse_goto_line("main.rs:0"),
            None,
            "las líneas empiezan en 1"
        );
        assert_eq!(parse_goto_line("main.rs"), None, "sin sufijo no hay goto");
        assert_eq!(parse_goto_line("a:b:7"), Some(7), "el último ':' manda");
        assert_eq!(parse_goto_line(":5"), None, "sin nombre no hay goto");
        assert_eq!(parse_goto_line("main.rs:x"), None, "no numérico");
    }

    #[test]
    fn collect_files_walks_skipping_noise_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}").expect("a.rs");
        std::fs::create_dir(dir.path().join("sub")).expect("sub");
        std::fs::write(dir.path().join("sub").join("b.rs"), "").expect("b.rs");
        std::fs::create_dir(dir.path().join(".git")).expect(".git");
        std::fs::write(dir.path().join(".git").join("cfg"), "").expect("cfg");
        std::fs::create_dir(dir.path().join("target")).expect("target");
        std::fs::write(dir.path().join("target").join("gen.rs"), "").expect("gen");
        std::fs::create_dir(dir.path().join("node_modules")).expect("nm");
        std::fs::write(dir.path().join("node_modules").join("x.js"), "").expect("x");

        let names: Vec<String> = collect_files(dir.path(), 20_000)
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["a.rs".to_string(), "sub/b.rs".to_string()]);
        assert!(
            !names.iter().any(|n| n.starts_with(".git")
                || n.starts_with("target")
                || n.starts_with("node_modules")),
            "los directorios ruidosos quedan fuera: {names:?}"
        );

        // El tope corta el recorrido.
        assert_eq!(collect_files(dir.path(), 1).len(), 1);
    }

    #[test]
    fn open_path_focuses_existing_or_adds_new() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        let mut tabs = Tabs::new(Editor::from_cli(a.clone()));
        let mut settings = settings::Settings::default();

        open_path(&mut tabs, &mut settings, b.clone());
        assert_eq!(tabs.len(), 2, "archivo nuevo → pestaña nueva");
        assert_eq!(tabs.active().tab_label(), "b.rs", "la nueva queda activa");

        open_path(&mut tabs, &mut settings, a.clone());
        assert_eq!(tabs.len(), 2, "ya abierta → no duplica");
        assert_eq!(tabs.active().tab_label(), "a.rs", "enfoca la existente");
        assert!(tabs.active().focused(), "toma el foco");
    }

    #[test]
    fn save_notifies_success_and_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.txt");
        let mut tabs = Tabs::new(Editor::from_cli(path));
        let mut clipboard = None;
        let mut palette = Palette::default();
        let mut search = SearchBar::default();
        let mut findfiles = FindFiles::default();
        let mut settings = settings::Settings::default();
        let mut notice: Option<(String, Instant)> = None;

        tabs.active_mut().insert_text("hola");
        run_command(
            &mut tabs,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "file.save",
        );
        let (text, _) = notice.take().expect("aviso de guardado");
        assert!(text.starts_with("Guardado: a.txt"), "{text}");
        assert!(!tabs.active().dirty(), "guardar limpia la marca");

        // Error: la ruta no existe como directorio → no se puede escribir.
        let mut broken = Tabs::new(Editor::from_cli(
            dir.path().join("inexistente").join("x.txt"),
        ));
        notice = None;
        run_command(
            &mut broken,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "file.save",
        );
        let (text, _) = notice.take().expect("aviso de error");
        assert!(text.starts_with("Error al guardar"), "{text}");
    }

    #[test]
    fn minimap_toggle_flips_setting_with_notice() {
        let mut tabs = Tabs::new(Editor::default());
        let mut clipboard = None;
        let mut palette = Palette::default();
        let mut search = SearchBar::default();
        let mut findfiles = FindFiles::default();
        let mut settings = settings::Settings::default();
        let mut notice: Option<(String, Instant)> = None;

        assert!(settings.minimap, "por defecto la franja va visible");
        let redraw = run_command(
            &mut tabs,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "minimap.toggle",
        );
        assert!(redraw, "pide repintar");
        assert!(!settings.minimap, "se apaga");
        let (text, _) = notice.take().expect("aviso del toggle");
        assert_eq!(text, "minimap = false", "{text}");

        run_command(
            &mut tabs,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "minimap.toggle",
        );
        assert!(settings.minimap, "vuelve a encender");
        assert_eq!(
            notice.take().map(|(t, _)| t).as_deref(),
            Some("minimap = true")
        );
    }

    #[test]
    fn theme_toggle_flips_editor_bar_and_search() {
        let mut tabs = Tabs::new(Editor::default());
        let mut clipboard = None;
        let mut palette = Palette::default();
        let mut search = SearchBar::default();
        let mut findfiles = FindFiles::default();
        let mut settings = settings::Settings::default();
        let mut notice: Option<(String, Instant)> = None;
        assert_eq!(tabs.active().theme(), Theme::dark(), "arranca oscuro");

        run_command(
            &mut tabs,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "theme.toggle",
        );
        let (text, _) = notice.take().expect("aviso del tema");
        assert_eq!(text, "Tema: claro");
        assert_eq!(settings.theme, "light");
        assert_eq!(tabs.active().theme(), Theme::light());
        assert_eq!(search.theme(), Theme::light());

        run_command(
            &mut tabs,
            &mut clipboard,
            &mut palette,
            &mut search,
            &mut findfiles,
            &mut settings,
            &mut notice,
            "theme.toggle",
        );
        let (text, _) = notice.take().expect("segundo aviso");
        assert_eq!(text, "Tema: oscuro");
        assert_eq!(settings.theme, "dark");
        assert_eq!(tabs.active().theme(), Theme::dark());
        assert_eq!(search.theme(), Theme::dark());
    }

    #[test]
    fn focus_group_and_focus_tab_move_the_focused_flag() {
        let mut tabs = Tabs::new(Editor::default());
        assert!(tabs.active().focused());

        // split transfiere el flag: el grupo nuevo queda con el foco.
        tabs.split();
        assert!(!tabs.group_active_mut(0).focused(), "izq. apagado");
        assert!(tabs.group_active_mut(1).focused(), "der. con foco");

        focus_group(&mut tabs, 0);
        assert_eq!(tabs.active_group_index(), 0);
        assert!(tabs.group_active_mut(0).focused());
        assert!(!tabs.group_active_mut(1).focused(), "el que sale se apaga");

        // Click en la barra del otro grupo: grupo + pestaña.
        focus_tab(&mut tabs, 1, 0);
        assert_eq!(tabs.active_group_index(), 1);
        assert!(tabs.group_active_mut(1).focused());
        assert!(!tabs.group_active_mut(0).focused());

        // Sin cambios no toca nada.
        focus_tab(&mut tabs, 1, 0);
        assert_eq!(tabs.active_group_index(), 1);
        focus_group(&mut tabs, 1);
        assert_eq!(tabs.active_group_index(), 1);
    }

    #[test]
    fn boot_tabs_restores_hot_buffers_and_dedupes_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.rs");
        std::fs::write(&a, "en disco").expect("escribe a.rs");

        let mut session = session::Session::default();
        session.set_buffers(vec![
            session::HotBuffer {
                path: Some(a.to_string_lossy().into_owned()),
                text: "versión sin guardar".to_owned(),
            },
            session::HotBuffer {
                path: None,
                text: "borrador".to_owned(),
            },
        ]);

        // La CLI trae a.rs (ya restaurado) y b.rs (nuevo).
        let b = dir.path().join("b.rs");
        let tabs = boot_tabs(&session, vec![a.clone(), b.clone()]);

        assert_eq!(tabs.len(), 3, "hot exit (2) + b.rs; a.rs no duplica");
        assert_eq!(
            tabs.active().tab_label(),
            "a.rs",
            "el orden: sesión primero"
        );
        assert_eq!(
            tabs.active().text(),
            "versión sin guardar",
            "lo no-guardado manda sobre el disco"
        );
        assert!(tabs.active().dirty(), "el buffer restaurado sigue sucio");
        assert_eq!(tabs.active().path(), Some(a.as_path()));
    }

    #[test]
    fn boot_tabs_falls_back_to_blank_and_cli_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.rs");
        std::fs::write(&a, "fn main() {}").expect("escribe");

        let blank = boot_tabs(&session::Session::default(), Vec::new());
        assert_eq!(blank.len(), 1, "sin sesión ni CLI: editor en blanco");
        assert!(!blank.active().dirty());

        let only_cli = boot_tabs(&session::Session::default(), vec![a.clone()]);
        assert_eq!(only_cli.len(), 1);
        assert_eq!(only_cli.active().tab_label(), "a.rs");
    }

    #[test]
    fn collect_hot_buffers_takes_only_dirty_tabs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.rs");
        std::fs::write(&a, "en disco").expect("escribe");
        let mut tabs = Tabs::from_editors(vec![Editor::from_cli(a.clone())]);
        tabs.active_mut().select_all();
        tabs.active_mut().insert_text("cambio");

        let untitled = Editor::default();
        tabs.add(untitled);
        tabs.active_mut().insert_text("borrador");

        let buffers = collect_hot_buffers(&tabs);
        assert_eq!(buffers.len(), 2, "los dos sucios");
        assert_eq!(
            buffers[0].path.as_deref(),
            Some(a.to_string_lossy().as_ref())
        );
        assert_eq!(buffers[0].text, "cambio");
        assert_eq!(buffers[1].path, None, "sin título va como None");
        assert_eq!(buffers[1].text, "borrador");

        // Guardar limpia la marca: el tab salvo desaparece del hot exit.
        tabs.set_active(0);
        tabs.active_mut().save().expect("guarda a.rs");
        let buffers = collect_hot_buffers(&tabs);
        assert_eq!(buffers.len(), 1, "el que se guardó se descarta");
        assert_eq!(buffers[0].path, None);
    }
}
