//! GPU front-end (experimental): one PTY session in a `winit` window, text
//! rendered by `glyphon` on `wgpu`.
//!
//! The engine is shared with the TUI — the same `Terminal`, `Session` and key
//! decoding rules — and only the drawing differs. The grid is turned into
//! runs of cells that share a style; each run becomes one background quad and
//! one shaped text area. The cursor is a translucent block drawn after the
//! text. Mouse selection, splits and the palette are later slices.

use std::fs::File;
use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use glyphon::cosmic_text::{
    Attrs, AttrsList, Buffer, BufferLine, Color as TextColor, Family, LineEnding, Metrics, Shaping,
    Style, Weight,
};
use glyphon::{
    Cache, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use pollster::block_on;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use weft_term::Session;
use weft_term::grid::{Color, Wide};
use weft_term::vt::Terminal;

use crate::theme::{self, Theme};

const READ_BUFFER: usize = 16 * 1024;
const MARGIN: f32 = 8.0;
/// Maximum quads a frame may draw; each quad is six vertices of six floats.
const MAX_QUADS: usize = 32 * 1024;

/// One styled span of the grid: concrete colors, text and the attributes the
/// text renderer needs.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    /// Column where the run starts.
    pub x: usize,
    /// Row of the run.
    pub y: usize,
    /// Screen text, wide characters collapsed to their first cell.
    pub text: String,
    /// Foreground as RGB bytes.
    pub fg: (u8, u8, u8),
    /// Background as RGB bytes; the theme background means "no quad".
    pub bg: (u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Resolves a grid color to concrete RGB bytes for drawing.
pub fn color_of(color: Color, foreground: bool, theme: &Theme) -> (u8, u8, u8) {
    match color {
        Color::Default => {
            if foreground {
                theme.foreground
            } else {
                theme.background
            }
        }
        Color::Indexed(index) if index < 16 => theme.palette[usize::from(index)],
        Color::Indexed(index) => {
            let level = |step: u8| -> u8 {
                if step == 0 {
                    0
                } else {
                    u8::try_from(40 * u16::from(step) + 55).unwrap_or(255)
                }
            };
            if index < 232 {
                let n = u16::from(index - 16);
                let (r, g, b) = (n / 36, (n / 6) % 6, n % 6);
                (level(r as u8), level(g as u8), level(b as u8))
            } else {
                let gray = u8::try_from(8 + 10 * (u16::from(index) - 232)).unwrap_or(238);
                (gray, gray, gray)
            }
        }
        Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// Blends a foreground halfway into the background for the dim attribute.
fn dim(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| u8::try_from((u16::from(f) + u16::from(b)) / 2).unwrap_or(b);
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// Groups a grid row into runs of cells sharing one style. Hidden cells are
/// dropped; trailing halves of wide characters are painted by the leader.
pub fn cell_runs(terminal: &Terminal, theme: &Theme) -> Vec<Run> {
    let mut runs = Vec::new();
    for y in 0..terminal.grid.rows() {
        let mut x = 0usize;
        while x < terminal.grid.cols() {
            let cell = terminal.grid.cell(x, y);
            if cell.wide == Wide::Trailing {
                x += 1;
                continue;
            }
            let mut fg = color_of(cell.fg, true, theme);
            let mut bg = color_of(cell.bg, false, theme);
            if cell.attrs.reverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.attrs.dim {
                fg = dim(fg, bg);
            }
            let key = (fg, bg, cell.attrs.bold, cell.attrs.italic);
            let start = x;
            let mut text = String::new();
            while x < terminal.grid.cols() {
                let cell = terminal.grid.cell(x, y);
                if cell.wide == Wide::Trailing || cell.attrs.hidden {
                    x += 1;
                    continue;
                }
                let mut run_fg = color_of(cell.fg, true, theme);
                let mut run_bg = color_of(cell.bg, false, theme);
                if cell.attrs.reverse {
                    std::mem::swap(&mut run_fg, &mut run_bg);
                }
                if cell.attrs.dim {
                    run_fg = dim(run_fg, run_bg);
                }
                let run_key = (run_fg, run_bg, cell.attrs.bold, cell.attrs.italic);
                if run_key != key {
                    break;
                }
                text.push(cell.ch);
                x += 1;
            }
            runs.push(Run {
                x: start,
                y,
                text,
                fg,
                bg,
                bold: terminal.grid.cell(start, y).attrs.bold,
                italic: terminal.grid.cell(start, y).attrs.italic,
                underline: terminal.grid.cell(start, y).attrs.underline,
                strike: terminal.grid.cell(start, y).attrs.strike,
            });
        }
    }
    runs.truncate(runs.len());
    runs
}

/// Appends the six clip-space vertices of an axis-aligned quad. The geometry
/// and the frame size make eight parameters; a struct would only rename them.
#[expect(clippy::too_many_arguments)]
fn push_quad(
    out: &mut Vec<u8>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: (u8, u8, u8),
    width: f32,
    height: f32,
) {
    if out.len() + 6 * 6 * 4 > MAX_QUADS * 6 * 6 * 4 {
        return;
    }
    let (r, g, b) = (
        f32::from(color.0) / 255.0,
        f32::from(color.1) / 255.0,
        f32::from(color.2) / 255.0,
    );
    let clip = |vx: f32, vy: f32| [(2.0 * vx / width) - 1.0, 1.0 - (2.0 * vy / height)];
    // Two triangles: top-left, top-right, bottom-left; top-right, bottom-right, bottom-left.
    let quad = [
        [x, y],
        [x + w, y],
        [x, y + h],
        [x + w, y],
        [x + w, y + h],
        [x, y + h],
    ];
    for [vx, vy] in quad {
        let [cx, cy] = clip(vx, vy);
        for value in [cx, cy, r, g, b, 1.0] {
            out.extend_from_slice(&value.to_ne_bytes());
        }
    }
}

/// State the event loop owns for the window.
struct GpuState {
    window: Option<std::sync::Arc<Window>>,
    session: Session,
    terminal: Terminal,
    receiver: Receiver<Vec<u8>>,
    theme: &'static Theme,
    renderer: Option<GpuRenderer>,
    font_size: f32,
    line_height: f32,
    cell_width: f32,
    ctrl: bool,
}

struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    atlas: TextAtlas,
    swash: SwashCache,
    text: TextRenderer,
    font_system: glyphon::FontSystem,
    viewport: Viewport,
    buffers: Vec<Buffer>,
    quad_pipeline: wgpu::RenderPipeline,
    quad_buffer: wgpu::Buffer,
    cell_width: f32,
}

impl GpuRenderer {
    fn new(
        window: &std::sync::Arc<Window>,
        size: (u32, u32),
        font_size: f32,
        line_height: f32,
    ) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // `Arc<Window>` implements `SurfaceTarget<'static>`, so the surface can
        // be stored without unsafe lifetime surgery.
        let surface = instance
            .create_surface(std::sync::Arc::clone(window))
            .expect("window surface");
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no GPU adapter");
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("weft gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
        }))
        .expect("no GPU device");
        let config = surface
            .get_default_config(&adapter, size.0.max(1), size.1.max(1))
            .expect("surface is presentable");
        surface.configure(&device, &config);

        let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("weft quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu/quad.wgsl").into()),
        });
        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("weft quad layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("weft quads"),
            layout: Some(&quad_layout),
            vertex: wgpu::VertexState {
                module: &quad_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 6 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            format: wgpu::VertexFormat::Float32x2,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            offset: 2 * 4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let quad_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("weft quad buffer"),
            size: u64::try_from(MAX_QUADS * 6 * 6 * 4).expect("quad buffer size"),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let mut font_system = glyphon::FontSystem::new();
        let swash = SwashCache::new();
        let viewport = Viewport::new(&device, &cache);
        let cell_width = measure_cell(&mut font_system, font_size, line_height);

        Self {
            device,
            queue,
            surface,
            adapter,
            atlas,
            swash,
            text,
            font_system,
            viewport,
            buffers: Vec::new(),
            quad_pipeline,
            quad_buffer,
            cell_width,
        }
    }

    fn resize(&mut self, size: (u32, u32)) {
        let Some(config) =
            self.surface
                .get_default_config(&self.adapter, size.0.max(1), size.1.max(1))
        else {
            return;
        };
        self.surface.configure(&self.device, &config);
    }

    fn draw(
        &mut self,
        theme: &Theme,
        runs: &[Run],
        cell: (f32, f32),
        cursor: Option<(usize, usize)>,
    ) {
        let (cell_width, line_height) = cell;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return,
        };
        let width = f32::from(u16::try_from(frame.texture.width()).unwrap_or(u16::MAX));
        let height = f32::from(u16::try_from(frame.texture.height()).unwrap_or(u16::MAX));

        self.buffers.clear();
        // Quads: backgrounds, underlines, strikes and the cursor block.
        let mut quads: Vec<u8> = Vec::new();
        for run in runs {
            let x = MARGIN + cell_width * run.x as f32;
            let y = MARGIN + line_height * run.y as f32;
            let span = cell_width * run.text.chars().count() as f32;
            if run.bg != theme.background {
                push_quad(&mut quads, x, y, span, line_height, run.bg, width, height);
            }
            if run.underline {
                push_quad(
                    &mut quads,
                    x,
                    y + line_height - 2.0,
                    span,
                    2.0,
                    run.fg,
                    width,
                    height,
                );
            }
            if run.strike {
                push_quad(
                    &mut quads,
                    x,
                    y + line_height * 0.55,
                    span,
                    2.0,
                    run.fg,
                    width,
                    height,
                );
            }
        }
        if let Some((cx, cy)) = cursor {
            let x = MARGIN + cell_width * cx as f32;
            let y = MARGIN + line_height * cy as f32;
            let (r, g, b) = (
                f32::from(theme.foreground.0) / 255.0,
                f32::from(theme.foreground.1) / 255.0,
                f32::from(theme.foreground.2) / 255.0,
            );
            // Translucent block so the character underneath stays readable.
            let clip = |vx: f32, vy: f32| [(2.0 * vx / width) - 1.0, 1.0 - (2.0 * vy / height)];
            let block = [
                [x, y],
                [x + cell_width, y],
                [x, y + line_height],
                [x + cell_width, y],
                [x + cell_width, y + line_height],
                [x, y + line_height],
            ];
            for [vx, vy] in block {
                let [cx, cy] = clip(vx, vy);
                for value in [cx, cy, r, g, b, 0.55] {
                    quads.extend_from_slice(&value.to_ne_bytes());
                }
            }
        }

        // Text areas: one per run, positioned by cell.
        let mut text_areas: Vec<TextArea<'_>> = Vec::with_capacity(runs.len());
        for run in runs {
            if run.text.trim().is_empty() {
                continue;
            }
            let mut buffer = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.cell_width, line_height),
            );
            buffer.set_size(Some(width), Some(line_height * 2.0));
            let attrs = Attrs::new()
                .family(Family::Monospace)
                .weight(if run.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                })
                .style(if run.italic {
                    Style::Italic
                } else {
                    Style::Normal
                });
            buffer.lines.push(BufferLine::new(
                run.text.clone(),
                LineEnding::Lf,
                AttrsList::new(&attrs),
                Shaping::Advanced,
            ));
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.buffers.push(buffer);
        }
        for (index, run) in runs.iter().enumerate() {
            if run.text.trim().is_empty() {
                continue;
            }
            let buffer = &self.buffers[index];
            text_areas.push(TextArea {
                buffer,
                left: MARGIN + cell_width * run.x as f32,
                top: MARGIN + line_height * run.y as f32,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: i32::from(u16::try_from(width as u32).unwrap_or(u16::MAX)),
                    bottom: i32::from(u16::try_from(height as u32).unwrap_or(u16::MAX)),
                },
                default_color: TextColor::rgba(run.fg.0, run.fg.1, run.fg.2, 255),
                custom_glyphs: &[],
            });
        }

        self.viewport.update(
            &self.queue,
            Resolution {
                width: u32::try_from(width as u64).unwrap_or(1),
                height: u32::try_from(height as u64).unwrap_or(1),
            },
        );
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("weft frame"),
            });
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("weft background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(theme.background.0) / 255.0,
                            g: f64::from(theme.background.1) / 255.0,
                            b: f64::from(theme.background.2) / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            drop(pass);
        }
        if !quads.is_empty() {
            self.queue.write_buffer(&self.quad_buffer, 0, &quads);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("weft quads"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.quad_pipeline);
            pass.set_vertex_buffer(0, self.quad_buffer.slice(..));
            let count = u32::try_from(quads.len() / (6 * 4)).unwrap_or(0);
            pass.draw(0..6, 0..count.min(MAX_QUADS as u32));
            drop(pass);
        }
        if self
            .text
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash,
            )
            .is_ok()
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("weft text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let _ = self.text.render(&self.atlas, &self.viewport, &mut pass);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

/// Measures the advance width of a monospace glyph by shaping a short line.
fn measure_cell(font_system: &mut glyphon::FontSystem, font_size: f32, line_height: f32) -> f32 {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(Some(400.0), Some(line_height * 2.0));
    buffer.lines.push(BufferLine::new(
        "MMMMMMMMMMMM".to_string(),
        LineEnding::Lf,
        AttrsList::new(&Attrs::new().family(Family::Monospace)),
        Shaping::Advanced,
    ));
    buffer.shape_until_scroll(font_system, false);
    let Some(layout) = buffer.layout_runs().next() else {
        return font_size * 0.6;
    };
    let Some(first) = layout.glyphs.first() else {
        return font_size * 0.6;
    };
    let Some(last) = layout.glyphs.last() else {
        return font_size * 0.6;
    };
    ((last.x + last.w) - first.x) / 12.0
}

impl ApplicationHandler for GpuState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = winit::window::WindowAttributes::default()
            .with_title("weft")
            .with_inner_size(LogicalSize::new(900u32, 560u32));
        let window =
            std::sync::Arc::from(event_loop.create_window(attributes).expect("create window"));
        let size = window.inner_size();
        let renderer = GpuRenderer::new(
            &window,
            (size.width, size.height),
            self.font_size,
            self.line_height,
        );
        self.cell_width = renderer.cell_width;
        self.renderer = Some(renderer);
        self.apply_size(size.width, size.height);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => self.apply_size(size.width, size.height),
            WindowEvent::ModifiersChanged(event) => {
                self.ctrl = event.state().contains(ModifiersState::CONTROL);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let bytes = key_bytes(&event, self.ctrl);
                if !bytes.is_empty() {
                    let _ = self.session.write(&bytes);
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                let pumped = self.pump_pty();
                if let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) {
                    let _ = pumped;
                    let runs = cell_runs(&self.terminal, self.theme);
                    let cursor = self.terminal.grid.cursor_visible.then(|| {
                        let cursor = self.terminal.grid.cursor();
                        (cursor.x, cursor.y)
                    });
                    renderer.draw(
                        self.theme,
                        &runs,
                        (self.cell_width, self.line_height),
                        cursor,
                    );
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.pump_pty()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        let _ = event_loop;
    }
}

impl GpuState {
    /// Applies a pixel size to the surface, the grid and the PTY in cells.
    fn apply_size(&mut self, width: u32, height: u32) {
        let cell_width = self.cell_width;
        let width_f = f32::from(u16::try_from(width).unwrap_or(u16::MAX));
        let height_f = f32::from(u16::try_from(height).unwrap_or(u16::MAX));
        let columns = ((width_f - 2.0 * MARGIN) / cell_width).max(2.0) as usize;
        let rows = ((height_f - 2.0 * MARGIN) / self.line_height).max(2.0) as usize;
        self.terminal.resize(columns, rows);
        let _ = self.session.resize(columns, rows);
        if let Some(renderer) = &mut self.renderer {
            renderer.resize((width.max(1), height.max(1)));
        }
    }

    /// Feeds buffered PTY output to the terminal. Returns whether anything
    /// arrived, so the caller can ask for a redraw.
    fn pump_pty(&mut self) -> bool {
        let mut pumped = false;
        loop {
            match self.receiver.try_recv() {
                Ok(bytes) => {
                    let responses = self.terminal.feed(&bytes);
                    if !responses.is_empty() {
                        let _ = self.session.write(&responses);
                    }
                    pumped = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        pumped
    }
}

/// Translates a `winit` key into the byte stream the shell expects. Returns an
/// empty vector for keys the terminal does not map.
pub fn key_bytes(event: &winit::event::KeyEvent, ctrl: bool) -> Vec<u8> {
    if event.state != winit::event::ElementState::Pressed {
        return Vec::new();
    }
    match &event.logical_key {
        WKey::Character(text) => {
            if ctrl {
                let Some(c) = text.as_str().chars().next() else {
                    return Vec::new();
                };
                let code = c.to_ascii_uppercase() as u8;
                if (0x40..=0x5f).contains(&code) {
                    return vec![code - 0x40];
                }
                return Vec::new();
            }
            text.as_str().as_bytes().to_vec()
        }
        WKey::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab => b"\t".to_vec(),
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::ArrowUp => b"\x1b[A".to_vec(),
            NamedKey::ArrowDown => b"\x1b[B".to_vec(),
            NamedKey::ArrowRight => b"\x1b[C".to_vec(),
            NamedKey::ArrowLeft => b"\x1b[D".to_vec(),
            NamedKey::Home => b"\x1b[H".to_vec(),
            NamedKey::End => b"\x1b[F".to_vec(),
            NamedKey::Delete => b"\x1b[3~".to_vec(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Runs the GPU front-end until the window closes.
pub fn run() -> io::Result<()> {
    let config = weft_config::Config::load(crate::config_path()).unwrap_or_default();
    let shell = config
        .terminal
        .shell
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(crate::platform::default_shell);
    let theme: &'static Theme = theme::resolve(&config.terminal.theme);

    let (sender, receiver) = mpsc::channel();
    let session = Session::spawn(&shell, &[], 80, 24)?;
    let master: File = session.try_clone_master()?;
    thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER];
        let mut master = master;
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut state = GpuState {
        window: None,
        session,
        terminal: Terminal::new(80, 24),
        receiver,
        theme,
        renderer: None,
        font_size: 14.0,
        line_height: 18.0,
        cell_width: 8.4,
        ctrl: false,
    };
    event_loop.run_app(&mut state).map_err(io::Error::other)?;
    println!("weft: gpu session ended");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbors_with_one_style_merge_into_one_run() {
        let theme = theme::resolve("weft-dark");
        let mut terminal = Terminal::new(3, 1);
        terminal.feed(b"\x1b[31mab\x1b[0mc");
        let runs = cell_runs(&terminal, theme);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "ab");
        assert_eq!(runs[0].fg, theme.palette[1]);
        assert_eq!(runs[0].bg, theme.background);
        assert_eq!(runs[1].text, "c");
        assert_eq!(runs[1].fg, theme.foreground);
    }

    #[test]
    fn reverse_swaps_foreground_and_background() {
        let theme = theme::resolve("weft-dark");
        let mut terminal = Terminal::new(1, 1);
        terminal.feed(b"\x1b[7mr");
        let runs = cell_runs(&terminal, theme);
        assert_eq!(runs[0].fg, theme.background);
        assert_eq!(runs[0].bg, theme.foreground);
    }

    #[test]
    fn bold_runs_separate_from_plain() {
        let theme = theme::resolve("weft-dark");
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[1mab\x1b[0mcd");
        let runs = cell_runs(&terminal, theme);
        assert_eq!(runs.len(), 2);
        assert!(runs[0].bold);
        assert!(!runs[1].bold);
    }

    #[test]
    fn indexed_cube_and_grays_resolve() {
        let theme = theme::resolve("weft-dark");
        assert_eq!(color_of(Color::Indexed(196), true, theme), (255, 0, 0));
        assert_eq!(color_of(Color::Indexed(21), true, theme), (0, 0, 255));
        assert_eq!(color_of(Color::Indexed(232), true, theme), (8, 8, 8));
        assert_eq!(color_of(Color::Indexed(255), true, theme), (238, 238, 238));
        assert_eq!(color_of(Color::Default, true, theme), theme.foreground);
    }

    #[test]
    fn dim_blends_toward_the_background() {
        assert_eq!(dim((255, 255, 255), (0, 0, 0)), (127, 127, 127));
    }
}
