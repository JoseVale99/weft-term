# Arquitectura

## Crate graph

| Crate | Responsabilidad | No puede depender de |
| --- | --- | --- |
| `weft-term` | PTY, parser VT, grid, scrollback, bloques, sesión | UI, `winit`, `wgpu` |
| `weft-config` | TOML, temas, workflows con parámetros, valores por defecto, validación | UI |
| `weft-store` | Persistencia local: historial y sesiones en archivos atómicos (`redb` pendiente) | UI, `weft-term` |
| `weft-agent` | Conversación, parser JSON, protocolo OpenAI/SSE, compuerta de aprobación | UI, `winit` |
| `weft` | Ventana, render, layout, entrada, composición | — |
| `xtask` | Tareas de build, test y paridad | — |

Un test de arquitectura lee `cargo metadata` y falla si aparece una dependencia
prohibida. Se activa en M4 y corre en CI.

## Flujo de una sesión

```
tecla
  |
  v
editor de entrada (weft)
  |
  v
session.write(comando + "\n")  --->  PTY master
                                        |
shell del usuario  <--------------------+
  |
  |  OSC 133;A ... salida ... OSC 133;D;<code>   (bytes)
  v
hilo lector: read() -> Parser (weft-term) -> eventos
                                   |
                       +-----------+-----------+
                       v                       v
                 vte::Parser -> Term      Blocks
                     (grid)              (modelo)
                       |                       |
                       +----------+------------+
                                  v
                     canal acotado, coalescing por frame
                                  v
                 hilo UI: daño -> layout (taffy) -> wgpu/glyphon
```

Decisiones:

1. El comando lo conoce el editor y se guarda con `Blocks::set_command`. El PTY
   confirma cuándo empieza la salida, pero no se usa para reconstruir el texto
   del comando.
2. El PTY es la única fuente de salida. Grid y bloques consumen el mismo flujo,
   de modo que copiar un bloque copia exactamente lo que se mostró.
3. La lectura del PTY nunca bloquea el hilo UI. Un canal acotado y el
   procesamiento en lote absorben ráfagas sin frenar al shell.
4. Un hilo lector por sesión. `read()` bloqueante es más simple y predecible que
   E/S asíncrona por descriptor, y el coste es un hilo por terminal.

## Modelo de bloques

`Block` en `crates/weft-term/src/blocks.rs` es la unidad que ve la interfaz.

```
+---------------------------------------------+
| ~/code/weft   git status        0 - 42 ms  |   cabecera
| +-----------------------------------------+ |
| | On branch main                          | |   salida
| +-----------------------------------------+ |
+---------------------------------------------+
```

- `Prompt`: prompt actual, contiene el editor.
- `Running`: salida en curso, con duración en vivo.
- `Done`: cerrado, con exit code y duración; entra en `weft-store`.

Cuando el shell no emite marcas, la sesión entra en modo degradado: agrupa por
prompt heurístico y la interfaz lo indica con un icono y una nota. Las sesiones
remotas por `ssh` y los paneles de `tmux` se muestran como bloque opaco hasta que
el usuario lo cierra.

## Front-ends

El motor no sabe cómo se dibuja. Hoy existen dos front-ends en el crate `weft`:

- **ANSI (implementado, Unix).** Terminal crudo, render por frame y barra de
  estado. Reserva una fila para la barra, por lo que el PTY recibe `filas - 1`.
- **GPU (experimental, `weft gpu`).** Ventana `winit` (0.30), superficie `wgpu`
  (30) y glifos con `glyphon` (0.12). Primer corte: un único panel, sin
  divisiones ni paleta. Comparte con el TUI el motor (`Terminal`, `Session`,
  decoder de teclas) y solo sustituye el dibujo: el PTY se lee en un hilo
  aparte, la salida entra en el grid por la misma vía y por frame se colapsan
  las filas visibles (caracteres anchos una vez, blancos finales recortados) y
  se dibujan como áreas de texto por línea sobre el fondo del tema. El color
  por celda (ANSI, estilos) y los quads de fondo llegan en cortes posteriores.
  La ventana se crea a 80x24 y se redimensiona en celdas con el PTY; el evento
  `ModifiersChanged` alimenta el estado de Ctrl para los bytes de control.
  La superficie se crea sobre `Arc<Window>` (`SurfaceTarget<'static>`), por lo
  que no hay cirugía de lifetimes ni `unsafe`.

Ambos consumen `Grid` y `Blocks` y nada más. La diferencia está únicamente en
el dibujo.

## Render

Orden por frame:

1. Fondo del tema y separadores de bloque.
2. Glifos mediante `glyphon`, con atlas por tamaño y estilo; solo celdas con daño.
3. Cursor y selección como geometría, no como texto.
4. Interfaz: cabeceras, paleta y panel del agente por el mismo pipeline.

Presupuesto del front-end GPU: menos de 8 ms en el hilo UI para 200x60 celdas
con 10 000 líneas de scrollback a 60 fps. Se mide con `criterion` en modo
headless y con un contador de frames en la aplicación.

## Plataforma

`crates/weft/src/platform/` contiene un módulo por sistema con la misma interfaz:

```rust
pub trait Platform {
    fn clipboard(&self) -> &dyn Clipboard;
    fn credentials(&self) -> &dyn Credentials;
    fn modifier(&self) -> Modifier;       // Ctrl, Cmd
    fn paths(&self) -> &Paths;
    fn ime(&self) -> &dyn Ime;
}
```

Reglas:

- Ningún `cfg(target_os)` fuera de `platform/` y de sus equivalentes en
  `weft-store` y `weft-agent`.
- Los atajos se declaran por acción, no por combinación de teclas: la capa de
  plataforma traduce `Accion::NuevaPestana` al atajo que corresponda.
- Las rutas de configuración y datos salen de `directories`, nunca de rutas
  literales.
- El PTY es `portable-pty` en las tres plataformas; en Windows usa ConPTY y no
  hay rutas alternativas mantenidas por nosotros.

## Editor de entrada

Buffer de grafemas, cursor por índice, undo por transacciones y soporte de IME
desde el primer día. El editor no es un widget de terminal: es un editor de texto
con sintaxis de shell, lo que permite multilínea real, resaltado y autocompletado
sin trucos sobre `readline`. Si el buffer crece, se evalúa `ropey` con datos de
M3.

## Seguridad del agente

- Las herramientas de lectura usan una lista de rutas permitidas: el directorio
  de trabajo, sus subdirectorios, la configuración de Weft y lo que el usuario
  agregue.
- `proponer_comando` devuelve una propuesta; ejecutarla requiere aprobación
  explícita con vista previa del comando y del directorio.
- Las claves se leen del almacén de credenciales del sistema y nunca se escriben
  en TOML ni en logs.
- El agente recibe solo bloques recientes y la selección, con un presupuesto de
  tokens explícito y recorte determinista.

## Estado implementado

- `weft-store` persiste historial y sesiones con líneas escapadas y escritura
  atómica. No usa base de datos todavía.
- `weft-agent` implementa conversación acotada, parser JSON estricto, petición
  OpenAI con streaming y lectura SSE, compuerta de aprobación, herramientas con
  sandbox de rutas y un ciclo acotado que devuelve los resultados al modelo. Su
  transporte HTTP vive en `weft`: TCP plano y, para `https://`, TLS con
  `rustls` usando las raíces webpki o el PEM indicado en `[agent] ca_file`
  (para servidores con raíz propia). Un test e2e comprueba el intercambio TLS
  contra `openssl s_server` con certificado autofirmado.
- `weft workflow` lista y ejecuta comandos guardados con `{{parámetro}}`.
- El front-end Unix tiene temas propios (`weft-dark`, `weft-light`), paleta de
  comandos en F1, pestañas y un árbol binario de splits. Cada panel tiene su
  propio PTY y su propio `Terminal`; el enfocado recibe la entrada y muestra el
  cursor. Un split anida donde está el foco, así que filas y columnas se
  combinan en la misma pestaña. La paleta crea, cambia y cierra pestañas y
  paneles, alterna el tema, recorre los paneles y ajusta el reparto con Grow
  pane y Shrink pane. Cada split guarda un porcentaje (50/50 inicial, entre 10 y
  90). Cerrar un panel promueve a su hermano. Un split se rechaza si deja un
  panel con menos de 3 filas o 20 columnas.
- En Windows el front-end completo funciona sobre ConPTY (FFI propia a
  `kernel32` sin dependencias externas): modo raw y VT por consola, sesión
  spawn/read/write/resize/kill en `weft-term::conpty` y el mismo TUI de Unix.
  El test de ConPTY corre en el CI de Windows; en Unix el código es inerte.
- La ventana GPU sigue pendiente y requiere red para `winit`/`wgpu`/`glyphon`.

## Calidad

`cargo clippy --all-targets -- -D warnings` bloquea `clippy::all`. `pedantic`
queda como opt-in por crate: en la capa FFI (`sys.rs`) y en el intérprete de
escape produce decenas de avisos de estilo (`must_use_candidate`,
`missing_errors_doc`, conversiones numéricas) que no señalan defectos y con
`-D warnings` convierten el gate en ruido. Se puede reactivar por lint concreto
cuando el código lo justifique.
