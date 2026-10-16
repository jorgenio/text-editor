//! Pipeline instanciado de rectángulos (bajo el texto, encima y capa superior).
//!
//! Un quad por instancia: posición/tamaño en px físicos + color RGBA.
//! Tres draws con el mismo pipeline en orden: `0..under` antes del texto,
//! `under..over` después (carets) y `over..top` para la capa superior
//! (paneles), que va tras los carets.

use std::ops::Range;

use wgpu::{
    BlendState, ColorTargetState, Device, FragmentState, MultisampleState,
    PipelineLayoutDescriptor, PrimitiveState, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, VertexState,
};

use crate::scene::Quad;

const WGSL: &str = r#"
struct Res {
    res: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> res: Res;

struct Quad {
    pos: vec2<f32>,
    size: vec2<f32>,
    color: vec4<f32>,
};

@group(0) @binding(1) var<storage, read> quads: array<Quad>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let corner = corners[vi];
    let quad = quads[ii];
    let px = quad.pos + corner * quad.size;

    var out: VsOut;
    out.position = vec4<f32>(
        px.x / res.res.x * 2.0 - 1.0,
        1.0 - px.y / res.res.y * 2.0,
        0.0,
        1.0,
    );
    out.color = quad.color;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Bytes por quad instanciado: pos(2) + size(2) + color(4) = 8 f32 = 32 B.
const BYTES_PER_QUAD: u64 = 32;
const INITIAL_CAPACITY: usize = 256;

pub struct QuadRenderer {
    pipeline: RenderPipeline,
    uniform: wgpu::Buffer,
    quads: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    /// Capacidad actual en nº de quads.
    capacity: usize,
    resolution: [f32; 4],
}

impl QuadRenderer {
    pub fn new(device: &Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("quads"),
            source: ShaderSource::Wgsl(WGSL.into()),
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad resolution"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quads"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let quads = create_quad_buffer(device, INITIAL_CAPACITY);
        let bind_group = create_bind_group(device, &layout, &uniform, &quads);

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform,
            quads,
            layout,
            bind_group,
            capacity: INITIAL_CAPACITY,
            resolution: [0.0; 4],
        }
    }

    /// Sube los quads a la GPU en px físicos en el orden del render pass:
    /// `under` (bajo el texto) en `0..under.len()`, `over` (carets) a
    /// continuación y `top` (paneles) al final.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &wgpu::Queue,
        layers: (&[Quad], &[Quad], &[Quad]),
        scale: f64,
        resolution: (u32, u32),
    ) {
        let (under, over, top) = layers;
        let total = under.len() + over.len() + top.len();
        if total > self.capacity {
            self.capacity = total.next_power_of_two();
            self.quads = create_quad_buffer(device, self.capacity);
            self.bind_group = create_bind_group(device, &self.layout, &self.uniform, &self.quads);
        }

        for (group, offset) in [
            (under, 0),
            (over, under.len()),
            (top, under.len() + over.len()),
        ] {
            if group.is_empty() {
                continue;
            }
            queue.write_buffer(
                &self.quads,
                offset as u64 * BYTES_PER_QUAD,
                bytemuck::cast_slice(&to_instance_data(group, scale)),
            );
        }

        self.resolution = [resolution.0 as f32, resolution.1 as f32, 0.0, 0.0];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&self.resolution));
    }

    pub fn draw(&self, pass: &mut RenderPass<'_>, instances: Range<u32>) {
        if instances.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..6, instances);
    }
}

fn create_quad_buffer(device: &Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quads storage"),
        size: capacity as u64 * BYTES_PER_QUAD,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Conviete quads lógicos a datos de instancia (px físicos + color 0-1).
fn to_instance_data(quads: &[Quad], scale: f64) -> Vec<f32> {
    let mut data: Vec<f32> = Vec::with_capacity(quads.len() * 8);
    for quad in quads {
        data.extend_from_slice(&[
            quad.x * scale as f32,
            quad.y * scale as f32,
            quad.w * scale as f32,
            quad.h * scale as f32,
            quad.color.0 as f32 / 255.0,
            quad.color.1 as f32 / 255.0,
            quad.color.2 as f32 / 255.0,
            quad.color.3 as f32 / 255.0,
        ]);
    }
    data
}

fn create_bind_group(
    device: &Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    quads: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("quads"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: quads.as_entire_binding(),
            },
        ],
    })
}
