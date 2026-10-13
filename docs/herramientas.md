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
