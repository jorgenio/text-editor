//! El "qué dibujar" en un frame, independiente de wgpu/glyphon.
//!
//! Coordenadas y tamaños en **píxeles lógicos**; [`crate::text::TextLayer`] y
//! [`crate::quads::QuadRenderer`] aplican el scale factor del sistema.

/// Rectángulo de color (selección, caret, bordes…).
#[derive(Debug, Clone, Copy)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// RGBA 0-255 (el canal A permite selecciones translúcidas).
    pub color: (u8, u8, u8, u8),
}

impl Quad {
    pub fn new(x: f32, y: f32, w: f32, h: f32, color: (u8, u8, u8, u8)) -> Self {
        Self { x, y, w, h, color }
    }
}

pub struct TextSpan {
    pub text: String,
    /// Posición x en px lógicos.
    pub x: f32,
    /// Posición y en px lógicos (línea superior del texto).
    pub y: f32,
    /// Tamaño de fuente en px lógicos.
    pub size: f32,
    pub color: (u8, u8, u8),
}

impl TextSpan {
    pub fn new(text: impl Into<String>, x: f32, y: f32, size: f32, color: (u8, u8, u8)) -> Self {
        Self {
            text: text.into(),
            x,
            y,
            size,
            color,
        }
    }
}

pub struct Scene {
    pub clear: wgpu::Color,
    /// Rectángulos **bajo** el texto (selecciones).
    pub quads: Vec<Quad>,
    pub texts: Vec<TextSpan>,
    /// Rectángulos **encima** del texto (carets).
    pub overlays: Vec<Quad>,
}

impl Scene {
    pub fn new(clear: wgpu::Color) -> Self {
        Self {
            clear,
            quads: Vec::new(),
            texts: Vec::new(),
            overlays: Vec::new(),
        }
    }
}
