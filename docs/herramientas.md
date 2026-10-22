# Ejecución y herramientas

[← Volver al índice](../README.md)

El supervisor vive fuera de la navegación de la UI. La concurrencia se configura entre 1 y 4 chats, incluso dentro del mismo repositorio. Cada chat admite un solo trabajo activo. No hay aislamiento entre chats del mismo repositorio: pueden sobrescribir cambios si editan los mismos archivos. «Detener» cancela el transporte y los comandos propios. En Windows un Job Object limita la vida de sus procesos descendientes a la del comando.

Codex admite por defecto hasta **128 rondas por solicitud**, en lugar de 16. Una ronda puede contener varias llamadas de herramienta; este límite no es la concurrencia. Se puede configurar al iniciar la aplicación con la variable `HARNESS_MAX_GENERATIONS` (de 1 a 1024); valores inválidos o cero usan 128 y valores superiores se limitan a 1024. Al alcanzar el límite, el historial se conserva y se puede continuar con otro mensaje. La cancelación, las aprobaciones y el límite de tamaño de contexto siguen vigentes.

Ejemplo en PowerShell: `$env:HARNESS_MAX_GENERATIONS = "256"`, antes de ejecutar la aplicación.

Herramientas registradas en el espacio `harness`:

| Herramienta | Comportamiento |
| --- | --- |
| `list_files` | Un directorio, hasta 200 entradas y 8 KiB de respuesta. |
| `read_file` | Lectura explícita por desplazamiento, hasta 8 KiB. |
| `write_file` | Escritura de hasta 64 KiB, con revisión y aprobación. |
| `run_command` | Programa y argumentos, con aprobación; hasta 120 segundos. |
| `project_executions` | Configuraciones y estados compartidos del proyecto; consulta paginada, sin valores de entorno. |
| `project_execution_action` | Iniciar, detener, reiniciar o guardar configuraciones del panel, con aprobación. |
| `project_execution_logs` | Lectura explícita acotada de logs de las ejecuciones del proyecto. |
| `inspect_result` | Consulta explícita de un resultado guardado, hasta 8 KiB. |
| `register_image` | Copia una imagen PNG/JPEG del repositorio a la galería del chat; devuelve solo estado y referencia. |

Las acciones devuelven solo estado y referencia. stdout, stderr y metadatos quedan en archivos de `artifacts/<chat>/<referencia>/`; cada flujo de comando conserva hasta 1 MiB y registra si fue truncado. Guardar un archivo no lo adjunta al contexto. Solo una consulta explícita incorpora fragmentos de su contenido.

En el chat, las herramientas muestran un resumen con operación, ruta del archivo,
estado, cantidad leída/escrita y duración. El contenido y el JSON de la llamada no
se vuelcan al chat. Los mensajes antiguos también se resumen; sus rutas se recuperan
del historial de llamadas cuando está disponible y no se inventan tiempos pasados.

El botón «Logs» abre `logs/<chat>/<ejecución>/` en el explorador. Cada ejecución
guarda `events.jsonl` con modelo, effort, tamaño del contexto y de la petición,
duración por generación, tiempo hasta el primer fragmento de texto, tokens
reportados (incluida caché y razonamiento), permisos, tiempos y tamaños por
herramienta, errores y estado final. `tool-N.json` conserva cada respuesta de
herramienta; las acciones apuntan mediante su referencia a los artefactos con
stdout, stderr y metadatos. Los logs no guardan encabezados OAuth, prompts completos
ni el contenido de una escritura en el registro de eventos.

Los fallos de generación incluyen `diagnostics` en el evento `generation_failed`: fase del fallo, estado HTTP e identificador `x-request-id` si el servidor lo informó. Para el streaming se registra el número de bloques, bytes y eventos recibidos, el último tipo de evento y los tiempos transcurridos. Se distingue `request_transport`, `stream_transport`, `stream_idle_timeout`, `stream_decode`, `stream_protocol` y `stream_eof_without_terminal`. Los fallos de transporte conservan su descripción y una cadena de hasta ocho causas, con cada texto limitado a 512 caracteres y URLs omitidas. No se registran cabeceras de autenticación ni contenido de los bloques; las métricas se acumulan en memoria y se escriben solo al fallar. No se reintenta automáticamente una generación.

Los logs quedan fuera del contexto y del renderizado del chat. Se limitan a 16 MiB
por ejecución y las respuestas guardadas a 128 KiB cada una. Si el registro no puede
completarse, la tarea lo indica sin repetir acciones ya realizadas. No hay escritura
de logs por cada delta del streaming. Las respuestas ya formateadas se reutilizan
para evitar volver a interpretar todos los mensajes en cada actualización.

Las respuestas de Codex y Claude interpretan negrita, cursiva, listas y código con el
renderizador nativo de Slint. Los encabezados se muestran destacados y los bloques
de código conservan líneas e indentación. Las tablas Markdown se dibujan con celdas,
encabezados y alineación por columna, conservando el formato dentro de cada celda.
Markdown no soportado usa texto plano.

Las operaciones de archivos comprueban la pertenencia al repositorio y rechazan rutas que escapan hacia el directorio privado del harness. Los comandos aprobados pueden acceder fuera del repositorio: todavía no hay un sandbox del sistema operativo. La revisión de permisos lo indica antes de ejecutar.

El ciclo admite por defecto hasta 128 rondas por pedido (configurables con `HARNESS_MAX_GENERATIONS`, hasta 1024) y hasta 4 MiB de historial serializado. Al superar esos límites se detiene con un mensaje; la compactación automática todavía está pendiente. No reintenta automáticamente una generación. Después de una interrupción conserva el historial y completa las llamadas sin resultado con un estado de interrupción al continuar.

El streaming conserva los elementos completos de `response.output_item.done`,
incluidas herramientas y razonamiento cifrado, aunque el evento terminal traiga
una salida vacía. El texto final se reconcilia con el mensaje visible sin duplicar
los fragmentos recibidos. Una solicitud sin respuesta visible ni herramientas
se informa como fallida, conservando el uso reportado, sin reintento automático.

## PowerShell para agentes

Para scripts PowerShell, los agentes deben preferir `harness.powershell_run`:

```json
{"script":"Get-ChildItem -Name", "timeout_seconds":30}
```

El harness añade `-NoProfile -NonInteractive -Command` y pasa el script como un único argumento, sin otra shell intermedia. En Windows utiliza Windows PowerShell del sistema; en otros sistemas utiliza `pwsh`, que debe estar instalado. No carga perfiles ni añade `ExecutionPolicy Bypass`.

Reutiliza la aprobación explícita (que muestra programa, argumentos y script), el directorio del repositorio, el entorno restringido, la cancelación y el límite de 1–120 segundos de `run_command`. stdout/stderr y metadatos quedan en artefactos: la respuesta solo contiene estado y referencia, y se consulta con `inspect_result`. Los scripts pueden guardar imágenes en `$env:HARNESS_ARTIFACT_DIR`.

La herramienta se anuncia tanto a Codex como a Claude mediante sus esquemas y sus instrucciones. `run_command` sigue disponible para otros ejecutables o invocaciones especiales.

El [panel de ejecución del proyecto](ejecucion-proyecto.md) usa un supervisor independiente del turno del agente. Sus acciones mantienen procesos hasta que se detienen explícitamente; los logs no se añaden automáticamente al contexto.

## Lectura de varios fragmentos

Los agentes deben preferir `harness.read_file_ranges` para obtener varios rangos concretos en una sola llamada, sin scripts ni procesos PowerShell:

```json
{"ranges":[
  {"path":"src/runtime.rs","start_line":280,"line_count":25},
  {"path":"src/tool_runtime.rs","start_line":150,"line_count":20},
  {"path":"src/tool_runtime.rs","start_line":270,"line_count":30}
]}
```

Acepta entre 1 y 10 rangos, con líneas numeradas desde 1 y entre 1 y 10000 líneas por rango. El presupuesto **global de texto** es 8192 bytes UTF-8; los metadatos de respuesta son adicionales. Devuelve los resultados en el orden solicitado, con `path`, `start_line`, `text`, `more`, `next_line`, `next_offset` y `partial_last_line`. Conserva los saltos de línea originales. `more` indica que queda parte del rango solicitado, no que haya otras líneas fuera de él.

Si `more` es verdadero y `partial_last_line` es falso, continúa desde `next_line` con las líneas pendientes. Si se corta una línea larga, usa `read_file` con `offset: next_offset` para continuar por bytes. Nunca corta un carácter UTF-8. Los rangos que no pudieron leerse por agotarse el presupuesto incluyen `not_read: true` y pueden reenviarse en otra llamada. Un rango más allá del fin del archivo devuelve texto vacío y `more: false`.

La lectura es directa y en streaming: las líneas omitidas no se cargan en memoria. Aplica las restricciones de rutas de `read_file`, valida todos los rangos antes de leer y no necesita aprobación de comandos. Un rango inválido o un fragmento con UTF-8 inválido hace fallar la llamada. `read_file` continúa disponible para lecturas por bytes.

## Búsqueda, esquemas y Git de solo lectura

Estas herramientas entregan resultados acotados directamente, sin scripts, artefactos intermedios ni aprobación de comandos. Codex y Claude reciben instrucciones para preferirlas.

### `search_files`

```json
{"paths":["src"],"patterns":["run_command","requires_approval"],"regex":false,"context_lines":2,"offset":0,"limit":30}
```

Acepta 1–10 rutas y patrones; `regex: false` busca texto literal (sensible a mayúsculas), `true` utiliza expresiones regulares de Rust. Devuelve una coincidencia por línea aunque coincidan varios patrones, con archivo, línea y contexto (0–5 líneas por lado). `offset` cuenta líneas coincidentes; `limit` admite 1–100 resultados. `next_offset` permite continuar cuando `more` es verdadero. Una coincidencia demasiado larga devuelve su ubicación y `text_omitted: true`: amplíala con `read_file_ranges`.

La búsqueda no sigue enlaces simbólicos ni rutas que salgan del proyecto o entren en datos privados. Omite `.git`, `target`, `node_modules`, `.venv`, `__pycache__` y archivos binarios/no UTF-8. Límites: 5000 archivos, 20000 entradas de recorrido, 16 MiB por archivo y 64 MiB leídos por llamada. `scan_limited: true` indica cobertura incompleta: restringe las rutas; no lo interpretes como ausencia de coincidencias. La paginación es determinista mientras los archivos no cambien.

### `file_outline`

```json
{"paths":["docs/herramientas.md","src/tool_runtime.rs"],"offset":0,"limit":50}
```

Devuelve encabezados Markdown (ATX y Setext, ignorando bloques cercados) o declaraciones **heurísticas** Rust/Python/JS/TS: funciones, clases, estructuras, enums, traits, módulos, tipos e interfaces. No es un parser AST; puede omitir símbolos o confundir declaraciones en comentarios. Los resultados incluyen archivo, línea, tipo y nombre; usa `read_file_ranges` para ampliar solo lo necesario. Acepta 1–10 archivos UTF-8, con límites de 16 MiB por archivo y 64 MiB en total. `offset` cuenta elementos; `limit` admite 1–100. Nombres excesivamente largos se acortan con `name_truncated: true`.

### `git_read`

```json
{"operation":"status","paths":[],"staged":false,"offset":0,"limit":50}
```

```json
{"operation":"diff","paths":["src/tool_runtime.rs"],"staged":false,"offset":0,"limit":4096}
```

`status` devuelve la descripción de la rama y registros con `path`, códigos `index`/`worktree` y `original` para renombres. `offset` cuenta registros y `limit` admite 1–100. `diff` devuelve el patch del árbol de trabajo; `staged: true` consulta el índice. Aquí `offset` y `limit` son bytes UTF-8 (1–8192 para el límite). Continúa siempre desde `next_offset`. `paths: []` limita al proyecto seleccionado; otras rutas son literales, no opciones ni pathspecs. No incluye el contenido de archivos sin seguimiento ni detalles de submódulos.

Git se invoca sin shell, pager, bloqueo opcional, fsmonitor, diff externo ni textconv; no hay staging, commits, checkout, hooks, red ni opciones arbitrarias. El entorno se restringe y se omite la configuración global/de sistema. Hay timeout de 30 segundos y cancelación. Si la salida interna supera 1 MiB, restringe `paths`. La respuesta se limita a 8 KiB; un registro de estado excepcionalmente largo puede devolver `path_omitted: true`. Por seguridad, no se expone Git si los datos privados del harness están dentro del proyecto. Rutas inexistentes solo se permiten cuando su padre existe (para consultar archivos eliminados).

Todas estas herramientas mantienen las restricciones de rutas del harness. La búsqueda y el outline limitan su conjunto de resultados a aproximadamente 7,6 KiB serializados, reservando espacio para metadatos; los resultados de llamadas sucesivas pueden variar si el repositorio cambia.
