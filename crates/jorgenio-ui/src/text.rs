//! Capa de texto sobre glyphon.
//!
//! - [`FontSystem`] con las fuentes del sistema **más** JetBrains Mono embebida
//!   (SIL OFL) como familia monoespaciada determinista en Windows y Linux.
//! - Un [`Buffer`] de cosmic-text por [`TextSpan`], cacheado por
//!   `(texto, tamaño, scale_factor)`: solo se re-shapea lo que cambió.
//! - Se shapea en **píxeles físicos** (`tamaño_lógico × scale_factor`) para que
//!   los glifos se rastericen al tamaño exacto del display (nítido a cualquier DPI).

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::{Device, MultisampleState, Queue, TextureFormat};

use crate::scene::TextSpan;

/// JetBrains Mono (SIL OFL, ver `assets/fonts/OFL.txt`).
const FALLBACK_FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
/// Familia que usa `Family::Monospace` en todos los SO.
const MONO_FAMILY: &str = "JetBrains Mono";
const LINE_HEIGHT_FACTOR: f32 = 1.4;

struct SpanBuffer {
    buffer: Buffer,
    /// Clave de validez del cache: re-shapear solo si algo cambió.
    text: String,
    size: f32,
    scale: f64,
}

pub struct TextLayer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffers: Vec<SpanBuffer>,
    resolution: (u32, u32),
}

impl TextLayer {
    pub fn new(
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        resolution: (u32, u32),
    ) -> Self {
        // Escanea las fuentes del sistema (puede tardar segundos en debug; una sola vez).
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(FALLBACK_FONT.to_vec());
        font_system.db_mut().set_monospace_family(MONO_FAMILY);

        let cache = Cache::new(device);
        let mut viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer = TextRenderer::new(&mut atlas, device, MultisampleState::default(), None);
        let swash_cache = SwashCache::new();

        viewport.update(
            queue,
            Resolution {
                width: resolution.0,
                height: resolution.1,
            },
        );

        Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            buffers: Vec::new(),
            resolution,
        }
    }

    /// Re-shapea lo stale y sube los glifos al atlas. Llamar antes del render pass.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        scale: f64,
        resolution: (u32, u32),
        spans: &[TextSpan],
    ) {
        if self.resolution != resolution {
            self.resolution = resolution;
            self.viewport.update(
                queue,
                Resolution {
                    width: resolution.0,
                    height: resolution.1,
                },
            );
        }

        self.buffers.truncate(spans.len());
        for (index, span) in spans.iter().enumerate() {
            let stale = self.buffers.get(index).is_none_or(|entry| {
                entry.text != span.text || entry.size != span.size || entry.scale != scale
            });
            if !stale {
                continue;
            }

            let size_px = (span.size as f64 * scale) as f32;
            let mut buffer = Buffer::new(
                &mut self.font_system,
                Metrics::new(size_px, size_px * LINE_HEIGHT_FACTOR),
            );
            buffer.set_size(Some(resolution.0 as f32), Some(resolution.1 as f32));
            buffer.set_text(
                &span.text,
                &Attrs::new().family(Family::Monospace),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);

            let entry = SpanBuffer {
                buffer,
                text: span.text.clone(),
                size: span.size,
                scale,
            };
            if index < self.buffers.len() {
                self.buffers[index] = entry;
            } else {
                self.buffers.push(entry);
            }
        }

        let areas: Vec<TextArea<'_>> = spans
            .iter()
            .zip(&self.buffers)
            .map(|(span, entry)| TextArea {
                buffer: &entry.buffer,
                left: span.x * scale as f32,
                top: span.y * scale as f32,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: resolution.0 as i32,
                    bottom: resolution.1 as i32,
                },
                default_color: Color::rgb(span.color.0, span.color.1, span.color.2),
                custom_glyphs: &[],
            })
            .collect();

        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .expect("glyphon prepare falló");
    }

    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .expect("glyphon render falló");
    }

    pub fn trim(&mut self) {
        self.atlas.trim();
    }
}
