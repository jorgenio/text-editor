//! Jorgenio Text — binario principal: ventana + event loop + orquestación.

mod editor;

use std::sync::Arc;
use std::time::Instant;

use editor::Editor;
use jorgenio_ui::gpu::GpuContext;
use winit::dpi::LogicalPosition;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::Key;
use winit::window::Window;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut Application::new(editor::cli_path()))?;
    Ok(())
}

struct Application {
    editor: Editor,
    gpu: Option<GpuContext>,
    modifiers: winit::keyboard::ModifiersState,
    last_title: String,
    cursor: LogicalPosition<f32>,
    caret_blink_deadline: Instant,
}

impl Application {
    fn new(path: Option<std::path::PathBuf>) -> Self {
        Self {
            editor: Editor::from_cli(path),
            gpu: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            last_title: String::new(),
            cursor: LogicalPosition::new(0.0, 0.0),
            caret_blink_deadline: Instant::now() + editor::CARET_BLINK,
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

        let gpu = pollster::block_on(GpuContext::new(window, event_loop));
        gpu.set_ime_allowed(true);
        gpu.request_redraw();
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
                self.editor.toggle_caret();
                self.caret_blink_deadline = now + editor::CARET_BLINK;
                if let Some(gpu) = &self.gpu {
                    gpu.request_redraw();
                }
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
                let viewport = gpu.logical_size();
                self.editor.set_viewport(viewport.0, viewport.1);
                let scene = self.editor.build_scene(viewport, gpu.advance_ratio());
                gpu.render(&scene);
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
                if (self.modifiers.control_key() || self.modifiers.super_key())
                    && key_event.state == ElementState::Pressed
                    && let Key::Character(text) = &key_event.logical_key
                    && text.as_str() == "s"
                {
                    match self.editor.save() {
                        Ok(()) => log::info!("guardado"),
                        Err(err) => log::error!("error al guardar: {err}"),
                    }
                    gpu.request_redraw();
                    return;
                }
                if self.editor.on_key(&key_event, self.modifiers) {
                    gpu.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = gpu.scale_factor();
                self.cursor = position.to_logical::<f32>(scale);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let advance = editor::FONT_SIZE * gpu.advance_ratio();
                let cursor: (f32, f32) = (self.cursor.x, self.cursor.y);
                self.editor.on_click(cursor.0, cursor.1, advance);
                gpu.request_redraw();
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
                self.editor.insert_text(&text);
                gpu.request_redraw();
            }
            _ => {}
        }
    }
}
