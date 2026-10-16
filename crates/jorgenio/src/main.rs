use std::sync::Arc;

use jorgenio_ui::gpu::{self, GpuContext};
use jorgenio_ui::scene::{Scene, TextSpan};
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::Window;

/// Paleta Monokai (el tema clásico de Sublime Text).
const BG: (u8, u8, u8) = (0x27, 0x28, 0x22);
const FG: (u8, u8, u8) = (0xF8, 0xF8, 0xF2);
const GREEN: (u8, u8, u8) = (0xA6, 0xE2, 0x2E);
const CYAN: (u8, u8, u8) = (0x66, 0xD9, 0xEF);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut Application::default())?;
    Ok(())
}

fn build_scene(scale: f64) -> Scene {
    let mut scene = Scene::new(gpu::srgb(BG.0, BG.1, BG.2));
    scene
        .texts
        .push(TextSpan::new("Jorgenio Text", 24.0, 24.0, 28.0, FG));
    scene.texts.push(TextSpan::new(
        "M1 — texto con glyphon: fuentes del sistema + JetBrains Mono embebida (OFL)",
        24.0,
        72.0,
        14.0,
        GREEN,
    ));
    scene.texts.push(TextSpan::new(
        format!("scale_factor = {scale:.2} · la ventana mantiene el tamaño lógico"),
        24.0,
        98.0,
        14.0,
        CYAN,
    ));
    scene
}

#[derive(Default)]
struct Application {
    gpu: Option<GpuContext>,
}

impl winit::application::ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Jorgenio Text")
            .with_inner_size(LogicalSize::new(1200.0, 800.0));
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("no se pudo crear la ventana: {err}");
                event_loop.exit();
                return;
            }
        };

        let gpu = pollster::block_on(GpuContext::new(window, event_loop));
        gpu.request_redraw();
        self.gpu = Some(gpu);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };

        match event {
            WindowEvent::Resized(size) => gpu.resize(size),
            WindowEvent::ScaleFactorChanged { .. } => gpu.request_redraw(),
            WindowEvent::RedrawRequested => {
                let scale = gpu.scale_factor();
                gpu.render(&build_scene(scale));
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}
