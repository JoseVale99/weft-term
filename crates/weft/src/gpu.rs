//! GPU front-end (experimental): one PTY session in a `winit` window, text
//! rendered by `glyphon` on `wgpu`.
//!
//! This is the first slice of M1: a single pane, no splits, no palette. It
//! shares the engine with the TUI — the same `Terminal`, `Session` and key
//! decoder — and only replaces drawing. Rows are laid out per line with the
//! theme's colors; per-cell styled runs arrive in a later slice.

use std::fs::File;
use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use glyphon::cosmic_text::{
    Attrs, AttrsList, Buffer, BufferLine, Color as TextColor, LineEnding, Metrics, Shaping,
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
use weft_term::grid::Wide;
use weft_term::vt::Terminal;

use crate::theme::{self, Theme};

const READ_BUFFER: usize = 16 * 1024;
const MARGIN: f32 = 8.0;

/// One visible row: text plus the colors used to draw it.
pub struct Row {
    /// Screen text, wide characters collapsed to their first cell.
    pub text: String,
}

/// Collapses a grid into visible rows: wide characters are shown once and
/// trailing blanks trimmed.
pub fn visible_rows(terminal: &Terminal) -> Vec<Row> {
    (0..terminal.grid.rows())
        .map(|y| {
            let mut text = String::new();
            let mut last_non_blank = 0usize;
            for x in 0..terminal.grid.cols() {
                let cell = terminal.grid.cell(x, y);
                if cell.wide == Wide::Trailing {
                    continue;
                }
                if cell.ch != ' ' {
                    last_non_blank = text.len();
                }
                text.push(cell.ch);
            }
            text.truncate(last_non_blank);
            Row { text }
        })
        .collect()
}

/// State the event loop owns for the window.
struct GpuState {
    window: Option<std::sync::Arc<Window>>,
    session: Session,
    terminal: Terminal,
    receiver: Receiver<Vec<u8>>,
    theme: &'static Theme,
    renderer: Option<GpuRenderer>,
    columns: usize,
    rows: usize,
    font_size: f32,
    line_height: f32,
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
}

impl GpuRenderer {
    fn new(window: &std::sync::Arc<Window>, size: (u32, u32)) -> Self {
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

        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let font_system = glyphon::FontSystem::new();
        let swash = SwashCache::new();
        let viewport = Viewport::new(&device, &cache);

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

    fn draw(&mut self, theme: &Theme, rows: &[Row], metrics: (f32, f32)) {
        let (font_size, line_height) = metrics;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return,
        };
        let size = (frame.texture.width(), frame.texture.height());
        self.buffers.clear();
        let mut text_areas: Vec<TextArea<'_>> = Vec::with_capacity(rows.len());
        for row in rows {
            if row.text.is_empty() {
                continue;
            }
            let mut buffer =
                Buffer::new(&mut self.font_system, Metrics::new(font_size, line_height));
            buffer.set_size(Some(size.0 as f32), Some(line_height * 2.0));
            buffer.lines.push(BufferLine::new(
                row.text.clone(),
                LineEnding::Lf,
                AttrsList::new(&Attrs::new()),
                Shaping::Advanced,
            ));
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.buffers.push(buffer);
        }
        for (index, buffer) in self.buffers.iter().enumerate() {
            text_areas.push(TextArea {
                buffer,
                left: MARGIN,
                top: MARGIN + line_height * index as f32,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: size.0 as i32,
                    bottom: size.1 as i32,
                },
                default_color: TextColor::rgba(
                    theme.foreground.0,
                    theme.foreground.1,
                    theme.foreground.2,
                    255,
                ),
                custom_glyphs: &[],
            });
        }

        self.viewport.update(
            &self.queue,
            Resolution {
                width: size.0,
                height: size.1,
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

impl ApplicationHandler for GpuState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = winit::window::WindowAttributes::default()
            .with_title("weft gpu")
            .with_inner_size(LogicalSize::new(800u32, 480u32));
        let window =
            std::sync::Arc::from(event_loop.create_window(attributes).expect("create window"));
        let size = window.inner_size();
        self.renderer = Some(GpuRenderer::new(&window, (size.width, size.height)));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                let columns = usize::try_from(size.width.max(8)).unwrap_or(80) / 9;
                let rows =
                    usize::try_from(size.height.max(16)).unwrap_or(24) / self.line_height as usize;
                self.columns = columns;
                self.rows = rows;
                self.terminal.resize(columns.max(1), rows.max(1));
                let _ = self.session.resize(columns.max(1), rows.max(1));
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize((size.width, size.height));
                }
            }
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
                    let rows = visible_rows(&self.terminal);
                    renderer.draw(self.theme, &rows, (self.font_size, self.line_height));
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
        columns: 80,
        rows: 24,
        font_size: 14.0,
        line_height: 18.0,
        ctrl: false,
    };
    event_loop.run_app(&mut state).map_err(io::Error::other)?;
    println!("weft: gpu session ended");
    Ok(())
}
