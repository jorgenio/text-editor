//! El "qué dibujar" en un frame, independiente de wgpu/glyphon.
//!
//! Coordenadas y tamaños en **píxeles lógicos**; el [`crate::text::TextLayer`]
//! aplica el scale factor del sistema operativo para llegar a físicos.

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
    pub texts: Vec<TextSpan>,
}

impl Scene {
    pub fn new(clear: wgpu::Color) -> Self {
        Self {
            clear,
            texts: Vec::new(),
        }
    }
}
