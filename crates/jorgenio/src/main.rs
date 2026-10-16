//! Jorgenio Text — binario principal: ventana + event loop + orquestación.

mod commands;
mod editor;
mod palette;
mod search;
mod settings;
mod snippets;
mod tabs;

use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::Editor;
use jorgenio_ui::gpu::GpuContext;
use jorgenio_ui::scene::Scene;
use palette::Palette;
use search::SearchBar;
use tabs::{TAB_BAR_H, Tabs};
use winit::dpi::LogicalPosition;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::Window;

/// Ventana de doble/triple click (misma "tanda" de clicks).
const CLICK_WINDOW: Duration = Duration::from_millis(450);
/// Tolerancia en px para considerar "el mismo punto" entre clicks.
const CLICK_SLOP2: f32 = 16.0; // (4 px)^2
/// Umbral de movimiento para que un click se convierta en arrastre.
const DRAG_SLOP2: f32 = 16.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut Application::new(editor::cli_paths()))?;
    Ok(())
}

struct Application {
    /// Una pestaña por archivo (M9); la activa es la que recibe los eventos.
    tabs: Tabs,
    /// Paleta de comandos (Ctrl+Shift+P).
    palette: Palette,
    /// Barra de buscar/reemplazar (Ctrl+F / Ctrl+H).
    search: SearchBar,
    /// Preferencias cargadas de `settings.json` (M11).
    settings: settings::Settings,
    gpu: Option<GpuContext>,
    /// Portapapeles del SO, creado en la primera uso (lazy).
    clipboard: Option<arboard::Clipboard>,
    modifiers: winit::keyboard::ModifiersState,
    last_title: String,
    cursor: LogicalPosition<f32>,
    caret_blink_deadline: Instant,
    /// Clicks para doble/triple click.
    last_click: Option<(Instant, LogicalPosition<f32>)>,
    click_count: u32,
    mouse_down: bool,
    drag_origin: Option<LogicalPosition<f32>>,
    drag_active: bool,
}

impl Application {
    fn new(paths: Vec<std::path::PathBuf>) -> Self {
        let settings = settings::load();
        let mut tabs = Tabs::from_paths(paths);
        tabs.apply_prefs(&settings);
        let blink = blink_interval(&settings);
        Self {
            tabs,
            palette: Palette::default(),
            search: SearchBar::default(),
            settings,
            gpu: None,
            clipboard: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            last_title: String::new(),
            cursor: LogicalPosition::new(0.0, 0.0),
            caret_blink_deadline: Instant::now() + blink,
            last_click: None,
            click_count: 0,
            mouse_down: false,
            drag_origin: None,
            drag_active: false,
        }
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
/// TODOS los editores abiertos (tab/gutter/parpadeo se ven al instante).
fn reload_settings_if_needed(
    settings: &mut settings::Settings,
    tabs: &mut Tabs,
    saved: &std::path::Path,
) {
    if saved != settings::path() {
        return;
    }
    *settings = settings::load();
    tabs.apply_prefs(settings);
    log::info!("preferencias recargadas");
}

/// Ejecuta un comando por id (keymap o paleta de comandos).
/// `true` = pide redraw.
fn run_command(
    tabs: &mut Tabs,
    clipboard: &mut Option<arboard::Clipboard>,
    palette: &mut Palette,
    search: &mut SearchBar,
    settings: &mut settings::Settings,
    id: &str,
) -> bool {
    match id {
        "palette.show" => {
            palette.open();
            true
        }
        "find.show" => {
            let prefill = tabs.active().single_line_selection();
            search.open(false, prefill.as_deref(), tabs.active_mut());
            true
        }
        "replace.show" => {
            let prefill = tabs.active().single_line_selection();
            search.open(true, prefill.as_deref(), tabs.active_mut());
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
                            editor.path().map(|p| p.to_path_buf())
                        }
                        Err(err) => {
                            log::error!("error al guardar: {err}");
                            None
                        }
                    }
                } else {
                    let name = default_save_name(&editor.tab_label());
                    save_dialog(&name).and_then(|path| match editor.save_as(path.clone()) {
                        Ok(()) => {
                            log::info!("guardado");
                            Some(path)
                        }
                        Err(err) => {
                            log::error!("error al guardar: {err}");
                            None
                        }
                    })
                }
            };
            if let Some(saved) = &saved {
                reload_settings_if_needed(settings, tabs, saved);
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
                        Some(path)
                    }
                    Err(err) => {
                        log::error!("error al guardar: {err}");
                        None
                    }
                })
            };
            if let Some(saved) = &saved {
                reload_settings_if_needed(settings, tabs, saved);
            }
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

        let attrs = Window::default_attributes()
            .with_title(self.tabs.active().title())
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 800.0));
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
        if self.tabs.active().focused() {
            let now = Instant::now();
            if now >= self.caret_blink_deadline {
                // Si el caret está fuera de pantalla no hay nada que
                // parpadear: se conserva el estado y se ahorra el repintar
                // completo (escena + glifos + quads) cada 530 ms.
                if self.tabs.active().caret_in_view() {
                    self.tabs.active_mut().toggle_caret();
                    if let Some(gpu) = &self.gpu {
                        gpu.request_redraw();
                    }
                }
                self.caret_blink_deadline = now + blink_interval(&self.settings);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.caret_blink_deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => {
                self.tabs.active_mut().set_focused(focused);
                gpu.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let frame = Instant::now();
                let viewport = gpu.logical_size();
                let advance = gpu.advance_ratio();
                // Preferencias → glifos (no-op si el ancho no cambió).
                gpu.set_tab_width(self.settings.tab_width as u16);
                // Cada grupo ocupa una columna de ancho igual; su editor vive
                // debajo de la barra (inset superior) y a la derecha de su
                // origen horizontal (inset izquierdo).
                let group_count = self.tabs.group_count();
                let gw = viewport.0 / group_count as f32;
                let editor_h = (viewport.1 - TAB_BAR_H).max(50.0);
                for g in 0..group_count {
                    self.tabs.set_group_geometry(g, g as f32 * gw, gw, editor_h);
                }
                // Une las escenas de todos los grupos: sus franjas en x son
                // disjuntas, así que el orden entre paneles es indiferente.
                let mut scene: Option<Scene> = None;
                for g in 0..group_count {
                    let part = self
                        .tabs
                        .group_active_mut(g)
                        .build_scene((gw, editor_h), advance);
                    match &mut scene {
                        Some(all) => all.append(part),
                        None => scene = Some(part),
                    }
                }
                let mut scene = scene.expect("siempre hay al menos un grupo");
                self.tabs
                    .render(&mut scene, viewport, editor::FONT_SIZE * advance);
                self.search.render(
                    &mut scene,
                    viewport,
                    editor::FONT_SIZE * advance,
                    self.tabs.active(),
                );
                self.palette
                    .render(&mut scene, viewport, editor::FONT_SIZE * advance);
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
                // Atajos del keymap (combinaciones exactas: ni mods extra
                // ni de menos). AltGr (Ctrl+Alt) no llega aquí: sin binding
                // con alt coincide.
                if key_event.state == ElementState::Pressed
                    && (self.modifiers.control_key() || self.modifiers.super_key())
                    && matches!(key_event.logical_key, Key::Character(_))
                    && let Some(id) = commands::command_for(self.modifiers, &key_event.logical_key)
                {
                    if self.palette.is_open() {
                        // Con la paleta abierta solo actúa el toggle; el
                        // resto de comandos se traga.
                        if id == "palette.show" {
                            self.palette.toggle();
                            gpu.request_redraw();
                        }
                    } else if run_command(
                        &mut self.tabs,
                        &mut self.clipboard,
                        &mut self.palette,
                        &mut self.search,
                        &mut self.settings,
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
                                &mut self.settings,
                                id,
                            );
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
                        let advance = editor::FONT_SIZE * gpu.advance_ratio();
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
                                &mut self.settings,
                                id,
                            );
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
                // Barra de pestañas: cambiar de pestaña o de grupo (el click
                // resuelve grupo+tab). Sin arrastre ni caret: termina aquí.
                if let Some((group, tab)) = self.tabs.click(self.cursor.x, self.cursor.y) {
                    focus_tab(&mut self.tabs, group, tab);
                    gpu.request_redraw();
                    return;
                }
                // Click en otro panel: ese grupo toma el foco (como Sublime)
                // y su editor activo coloca el caret.
                let group_count = self.tabs.group_count();
                let gw = gpu.logical_size().0 / group_count as f32;
                let group = ((self.cursor.x / gw).floor().max(0.0) as usize).min(group_count - 1);
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

                let advance = editor::FONT_SIZE * gpu.advance_ratio();
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
                self.mouse_down = false;
                self.drag_active = false;
                self.drag_origin = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * editor::LINE_HEIGHT * 3.0,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                };
                // Rueda sobre el panel bajo el cursor (sin robar el foco, como
                // Sublime); si el cursor aún no se movió → grupo 0.
                let group_count = self.tabs.group_count();
                let gw = gpu.logical_size().0 / group_count as f32;
                let group = ((self.cursor.x / gw).floor().max(0.0) as usize).min(group_count - 1);
                self.tabs.group_active_mut(group).on_wheel(dy);
                gpu.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.palette.is_open() {
                    self.palette.push_text(&text);
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
    use super::{default_save_name, focus_group, focus_tab};
    use crate::editor::Editor;
    use crate::tabs::Tabs;

    #[test]
    fn default_save_name_keeps_or_adds_extension() {
        assert_eq!(default_save_name("foo.rs"), "foo.rs");
        assert_eq!(default_save_name("sin título"), "sin título.txt");
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
}
