use std::sync::Arc;

use wgpu::{
    CompositeAlphaMode, DeviceDescriptor, Instance, InstanceDescriptor, LoadOp, Operations,
    PresentMode, RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions,
    SurfaceColorSpace, SurfaceConfiguration, TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};

use crate::quads::QuadRenderer;
use crate::scene::Scene;
use crate::text::TextLayer;

/// Contexto gráfico: instancia wgpu, device/queue, surface, capa de texto y quads.
///
/// Invariante de drop: `window` va al final del struct para que se dropee
/// *después* que la surface (bug conocido de wgpu: al revés crashea al cerrar).
pub struct GpuContext {
    instance: Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: SurfaceConfiguration,
    text: TextLayer,
    quads: QuadRenderer,
    window: Arc<Window>,
}

impl GpuContext {
    pub async fn new(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Self {
        let size = window.inner_size();

        let instance = Instance::new(InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        )));
        let adapter = instance
            .request_adapter(&RequestAdapterOptions::default())
            .await
            .expect("no se encontró un adaptador GPU compatible");
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default())
            .await
            .expect("request_device falló");

        let surface = instance
            .create_surface(window.clone())
            .expect("create_surface falló");

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: TextureFormat::Bgra8UnormSrgb,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let text = TextLayer::new(
            &device,
            &queue,
            config.format,
            (config.width, config.height),
        );
        let quads = QuadRenderer::new(&device, config.format);

        Self {
            instance,
            device,
            queue,
            surface,
            config,
            text,
            quads,
            window,
        }
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Tamaño lógico actual de la ventana (px lógicos, no físicos).
    pub fn logical_size(&self) -> (f32, f32) {
        let size = self.window.inner_size();
        let scale = self.scale_factor();
        (
            size.width as f32 / scale as f32,
            size.height as f32 / scale as f32,
        )
    }

    /// Avance (advance) de la fuente monoespaciada en emisiones.
    pub fn advance_ratio(&self) -> f32 {
        self.text.advance_ratio()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    /// Presenta un frame: clear color + quads bajo + textos + quads encima.
    pub fn render(&mut self, scene: &Scene) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .expect("readmitear la surface falló");
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                panic!("error de validación de wgpu en la surface");
            }
        };

        let scale = self.window.scale_factor();
        let resolution = (self.config.width, self.config.height);
        let phase = std::time::Instant::now();
        self.text
            .prepare(&self.device, &self.queue, scale, resolution, &scene.texts);
        if !scene.top_texts.is_empty() {
            self.text.prepare_top(
                &self.device,
                &self.queue,
                scale,
                resolution,
                &scene.top_texts,
            );
        }
        self.quads.prepare(
            &self.device,
            &self.queue,
            (&scene.quads, &scene.overlays, &scene.top_quads),
            scale,
            resolution,
        );
        let prepared = phase.elapsed();

        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(scene.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let under = scene.quads.len() as u32;
            let over = under + scene.overlays.len() as u32;
            let top = over + scene.top_quads.len() as u32;
            self.quads.draw(&mut pass, 0..under);
            self.text.render(&mut pass);
            self.quads.draw(&mut pass, under..over);
            self.quads.draw(&mut pass, over..top);
            if !scene.top_texts.is_empty() {
                self.text.render_top(&mut pass);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.text.trim();
        // Diagnóstico de rendimiento (CPU por fase; ver también main.rs).
        let total = phase.elapsed();
        if total > std::time::Duration::from_millis(16) {
            log::warn!(
                "gpu frame lenta: prepare={prepared:?} encode+submit={:?}",
                total - prepared
            );
        }
    }
}

/// Convierte un color sRGB 0-255 a lineal, que es lo que espera `wgpu::Color`.
pub fn srgb(r: u8, g: u8, b: u8) -> wgpu::Color {
    fn chan(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    wgpu::Color {
        r: chan(r),
        g: chan(g),
        b: chan(b),
        a: 1.0,
    }
}
