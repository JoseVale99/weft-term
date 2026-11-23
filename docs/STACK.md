# Stack técnico

Criterios de selección, en orden: todo el código propio en Rust sin runtime
interpretado; licencia permisiva; mantenimiento activo; soporte real de Linux,
macOS y Windows; y una ruta de salida clara si un crate deja de servir.

## Estado del build actual

El binario de hoy no usa ninguna dependencia externa: PTY, intérprete de escape,
grid y render ANSI están implementados sobre la biblioteca estándar y llamadas
POSIX. Los crates de esta página entran con la ventana GPU (M1) y con las capas
de configuración, persistencia y agente.

## Política de versiones

No se escriben versiones a mano. `scripts/bootstrap-deps.sh` las resuelve con
`cargo add`, se revisa la compatibilidad entre crates y se versiona el
`Cargo.lock`. Las restricciones que sí se fijan explícitamente son:

- `glyphon` debe compartir la versión mayor de `wgpu`.
- `alacritty_terminal` fija su propia versión de `vte`; no se agrega `vte` por
  separado si genera dos versiones incompatibles.
- La versión de `winit` define la API del bucle de eventos (0.30 usa
  `ApplicationHandler`).
- MSRV: Rust 1.88, edición 2024.

## Núcleo de terminal

| Crate | Función | Motivo | Alternativa |
| --- | --- | --- | --- |
| `winit` | Ventana, entrada, IME, arrastrar y soltar | Estándar de facto y soporte de las tres plataformas | `tao` |
| `wgpu` | Render en GPU | Un solo código sobre Vulkan, Metal, DX12 y GL | `ash` (solo Vulkan) |
| `glyphon` | Texto en GPU | Atlas de glifos sobre `cosmic-text` | Atlas propio con `swash` |
| `cosmic-text` | Shaping, BiDi, fallback, emoji | Rust puro, cobertura CJK y RTL | `rustybuzz` + `fontdb` |
| `taffy` | Layout flexbox de bloques | Layout real para la interfaz de bloques | Layout propio |
| `vte` | Parser de secuencias de escape | Probado en Alacritty | Parser propio |
| `alacritty_terminal` | Grid, scrollback, selección, reflow | Apache-2.0, sin ventana ni render | `wezterm-term` |
| `portable-pty` | PTY y ConPTY | API común en Unix y Windows | `nix` + `libc` |

## Datos, sistema y plataforma

| Crate | Función | Nota multiplataforma |
| --- | --- | --- |
| `redb` | Base embebida ACID | Rust puro; sustituye a SQLite y evita compilar C |
| `serde`, `toml`, `serde_json` | Configuración y protocolo | Config en XDG, Application Support o AppData |
| `directories` | Rutas del sistema | Resuelve las tres plataformas |
| `arboard` | Portapapeles | X11/Wayland, macOS y Windows |
| `notify` | Recarga de configuración | inotify, FSEvents y ReadDirectoryChangesW |
| `keyring` | Claves de proveedores | Secret Service, Keychain y Credential Manager |
| `tracing`, `tracing-subscriber` | Logs estructurados | `RUST_LOG=weft=debug` |
| `thiserror` / `anyhow` | Errores | `thiserror` en librerías, `anyhow` solo en `main` |
| `rustls` | TLS | Rust puro, evita OpenSSL |
| `reqwest` | HTTP del agente | Con `rustls-tls`, `http2` y `stream` |
| `futures-util`, `eventsource-stream` | Streaming SSE | Tokens a medida que llegan |
| `uuid`, `sha2`, `humantime` | Identificadores y utilidades | |

## Backends por plataforma

| Capa | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Ventana | Wayland / X11 | Cocoa | Win32 |
| Render | Vulkan u OpenGL | Metal | DX12 o Vulkan |
| PTY | `forkpty` | `forkpty` | ConPTY |
| Portapapeles | Wayland / X11 | NSPasteboard | Win32 Clipboard |
| Credenciales | Secret Service | Keychain | Credential Manager |
| Modificador de atajos | Ctrl | Cmd | Ctrl |

Ningún `cfg(target_os)` debe aparecer fuera de `crates/weft/src/platform/` y de
los módulos equivalentes de `weft-store` y `weft-agent`. El resto del código
trabaja con interfaces comunes.

## Desarrollo y CI

- `insta`: snapshots de render y de eventos de bloques.
- `proptest`: editor y parser.
- `criterion`: benchmarks de parseo, layout y render headless.
- `expectrl`: sesiones PTY reales en tests.
- `tempfile`, `cargo-nextest`, `cargo-deny`, `cargo-dist`.
- `xtask`: tareas del proyecto escritas en Rust.
- `rust-anything`: herramienta de desarrollo (MIT, Rust), no dependencia de la
  aplicación.

CI ejecuta formato, Clippy con `-D warnings`, tests y build en Linux, macOS y
Windows.

## Decisiones descartadas

- **Electron, Tauri o wry**: la aplicación sería web; se necesita una grid
  propia a 60 fps sin DOM.
- **egui o iced como framework principal**: válidos para herramientas, pero no
  para control por celda, selección e IME de un terminal. `egui` puede usarse en
  un panel de depuración temporal.
- **SQLite**: compila C y complica la regla de Rust puro; `redb` cubre el caso.
- **OpenSSL**: se usa `rustls`.
- **Un shell embebido en Rust**: el terminal usa el `$SHELL` del usuario; la
  integración va por scripts, igual que en iTerm2 o VS Code.
- **Node o npm en el build**: las tareas viven en `xtask`.

## Perfiles de build

```toml
[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"
strip = true

[profile.dev]
opt-level = 1
debug = 1

[profile.dev.package."*"]
opt-level = 3
```

El perfil de desarrollo mantiene compilación rápida sin sacrificar el rendimiento
de las dependencias gráficas.
