//! Jorgenio Text — binario principal: ventana + event loop + orquestación.

mod commands;
mod editor;
mod palette;

use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::Editor;
use jorgenio_ui::gpu::GpuContext;
use palette::Palette;
use winit::dpi::LogicalPosition;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::Key;
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
    event_loop.run_app(&mut Application::new(editor::cli_path()))?;
    Ok(())
}

struct Application {
    editor: Editor,
    /// Paleta de comandos (Ctrl+Shift+P).
    palette: Palette,
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
    fn new(path: Option<std::path::PathBuf>) -> Self {
        Self {
            editor: Editor::from_cli(path),
            palette: Palette::default(),
            gpu: None,
            clipboard: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            last_title: String::new(),
            cursor: LogicalPosition::new(0.0, 0.0),
            caret_blink_deadline: Instant::now() + editor::CARET_BLINK,
            last_click: None,
            click_count: 0,
            mouse_down: false,
            drag_origin: None,
            drag_active: false,
        }
    }
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

/// Ejecuta un comando por id (keymap o paleta de comandos).
/// `true` = pide redraw.
fn run_command(
    editor: &mut Editor,
    clipboard: &mut Option<arboard::Clipboard>,
    palette: &mut Palette,
    id: &str,
) -> bool {
    match id {
        "palette.show" => {
            palette.open();
            true
        }
        "file.save" => {
            match editor.save() {
                Ok(()) => log::info!("guardado"),
                Err(err) => log::error!("error al guardar: {err}"),
            }
            true
        }
        "edit.undo" => {
            editor.undo();
            true
        }
        "edit.redo" => {
            editor.redo();
            true
        }
        "edit.copy" => {
            copy_selection(editor, clipboard);
            true
        }
        "edit.cut" => {
            cut_selection(editor, clipboard);
            true
        }
        "edit.paste" => {
            paste_clipboard(editor, clipboard);
            true
        }
        "edit.select_all" => {
            editor.select_all();
            true
        }
        "edit.select_next_occurrence" => {
            editor.select_next_occurrence();
            true
        }
        "edit.delete_line" => {
            editor.delete_current_line();
            true
        }
        "edit.duplicate_line" => {
            editor.duplicate_current_line();
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
            .with_title(self.editor.title())
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
        self.last_title = self.editor.title();
        self.gpu = Some(gpu);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            return;
        }
        if self.editor.focused() {
            let now = Instant::now();
            if now >= self.caret_blink_deadline {
                // Si el caret está fuera de pantalla no hay nada que
                // parpadear: se conserva el estado y se ahorra el repintar
                // completo (escena + glifos + quads) cada 530 ms.
                if self.editor.caret_in_view() {
                    self.editor.toggle_caret();
                    if let Some(gpu) = &self.gpu {
                        gpu.request_redraw();
                    }
                }
                self.caret_blink_deadline = now + editor::CARET_BLINK;
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
                self.editor.set_focused(focused);
                gpu.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let frame = Instant::now();
                let viewport = gpu.logical_size();
                self.editor.set_viewport(viewport.0, viewport.1);
                let advance = gpu.advance_ratio();
                let mut scene = self.editor.build_scene(viewport, advance);
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
                let title = self.editor.title();
                if title != self.last_title {
                    gpu.set_title(&title);
                    self.last_title = title;
                }
                self.caret_blink_deadline = Instant::now() + editor::CARET_BLINK;
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
                        &mut self.editor,
                        &mut self.clipboard,
                        &mut self.palette,
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
                                &mut self.editor,
                                &mut self.clipboard,
                                &mut self.palette,
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
                if self.editor.on_key(&key_event, self.modifiers) {
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
                        self.editor.select_to(self.cursor.x, self.cursor.y, advance);
                        gpu.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Clic fuera de la paleta la cierra (sigue colocando caret).
                if self.palette.is_open() {
                    self.palette.close();
                }
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
                if self.modifiers.alt_key() && count == 1 {
                    self.editor
                        .add_cursor_at(self.cursor.x, self.cursor.y, advance);
                } else {
                    self.editor
                        .on_click(self.cursor.x, self.cursor.y, advance, count, shift);
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
                self.editor.on_wheel(dy);
                gpu.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.palette.is_open() {
                    self.palette.push_text(&text);
                } else {
                    self.editor.insert_text(&text);
                }
                gpu.request_redraw();
            }
            _ => {}
        }
    }
}
