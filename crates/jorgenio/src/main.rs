use std::sync::Arc;

use jorgenio_ui::gpu::{self, GpuContext};
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::Window;

/// Fondo tipo Monokai (el tema clásico de Sublime Text): #272822.
const BG: (u8, u8, u8) = (0x27, 0x28, 0x22);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut Application::default())?;
    Ok(())
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
            WindowEvent::RedrawRequested => gpu.render(gpu::srgb(BG.0, BG.1, BG.2)),
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}
