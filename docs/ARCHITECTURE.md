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
- **GPU (siguiente hito).** Ventana `winit`, superficie `wgpu`, atlas de glifos
  con `glyphon` y layout de la interfaz con `taffy`.

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
  OpenAI con streaming y lectura SSE, además de la compuerta de aprobación. Su
  transporte HTTP plano vive en `weft`; HTTPS requiere una decisión de
  dependencias (TLS) y hoy se rechaza con un mensaje explícito.
- `weft workflow` lista y ejecuta comandos guardados con `{{parámetro}}`.
- La ventana GPU, las pestañas, los splits y ConPTY siguen pendientes. La
  primera requiere red para `winit`/`wgpu`/`glyphon`; ConPTY requiere las
  dependencias de Windows.
