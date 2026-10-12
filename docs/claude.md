# Claude Code

[← Volver al índice](../README.md)

1. Instala Claude Code e inicia sesion con tu cuenta en el cliente oficial.
2. En Configurar agentes, pulsa Detectar y Verificar cliente en Claude.
3. Marca Habilitado y guarda. Solo aparecen agentes habilitados y verificados.
4. En un chat nuevo, elige Claude. Modelo y effort se configuran al pie del chat.

El cliente conserva y renueva su autenticacion; el harness no copia sus tokens.
Cada chat guarda un UUID de sesion y reanuda exclusivamente ese UUID. Cambiar de
proyecto o cerrar una pestana conserva los trabajos. Un proceso nativo permanece
activo solo durante el turno; al cancelar se termina su arbol de procesos propio.
Para cambiar de proveedor, crea otro chat: una conversacion existente conserva su
motor e historial. La configuracion y los identificadores persisten entre ejecuciones;
el estado de autenticacion se comprueba de nuevo, sin pedir otro login.

La integracion requiere Claude Code 2.1.259 o posterior. Usa `--restricted`,
`--tools ""`, `--strict-mcp-config` y un prompt del harness. Las herramientas
incorporadas y los hooks estan deshabilitados. Se verifica el catalogo de herramientas
al iniciar; cualquier herramienta externa interrumpe el turno. El cliente puede
conservar EndConversation para finalizar la conversacion. El puente MCP escucha
solo en loopback y exige un permiso aleatorio por turno, sin credenciales del proveedor.
Las escrituras y los comandos esperan aprobacion en la UI; las salidas siguen siendo
acotadas, con artefactos en disco y resumenes visibles en el chat.

Los modelos Predeterminado, Sonnet, Opus y Haiku son alias del cliente, no un catalogo
de derechos de la cuenta. Un modelo no disponible produce un error, sin cambiarlo.
Los niveles de effort admitidos dependen del modelo. El uso informado al final del
turno reemplaza los reportes parciales para evitar duplicaciones. La entrada suma
entrada sin cache, lectura de cache y creacion de cache. El contexto muestra la
ultima entrada individual; la capacidad solo se muestra cuando el cliente la informa.
La cuota de la barra lateral permite alternar Codex y Claude, y sigue al proveedor del chat seleccionado. Claude se consulta mediante el control `get_usage` del cliente oficial, sin mensaje al modelo ni claves API. Esta interfaz es experimental: si no esta disponible, se informa el error sin estimar porcentajes. Se consulta al verificar la cuenta y despues de cada turno del proveedor, sin sondeo en reposo. Cambiar la tarjeta reutiliza consultas recientes; finalizar un turno fuerza una lectura nueva. Si ya hay una consulta en curso, se agrupan las actualizaciones pendientes. La cabecera tiene flechas y el nombre centrado; los puntos debajo de la tarjeta indican los proveedores conectados y el activo.

Se rechaza autenticacion por API y se eliminan las variables de claves API y rutas
alternativas del proceso. La facturacion sigue las preferencias de la cuenta de Claude:
si tiene uso extra habilitado, iniciar sesion no garantiza ausencia de cargos extra.
La aplicacion no modifica esa preferencia. Esta integracion local no intermedia
credenciales de usuarios; para distribuir un producto con acceso de terceros hay que
revisar las condiciones vigentes de Anthropic.

`cargo run -- --check-agent claude` verifica instalacion y sesion sin inferencia.
`cargo run -- --check-claude-usage` consulta solamente la cuota, sin inferencia.
`cargo run -- --check-claude` realiza una lectura MCP real en un repositorio temporal
y consume un turno de Claude. Sus logs quedan en el directorio de datos bajo `smoke/`.
No se ejecuta automaticamente. Las pruebas normales usan un ejecutable simulado,
no una cuenta: cubren aprobacion/rechazo, llamadas duplicadas, reanudacion aislada,
streaming, tokens, cancelacion y seleccion de proveedores en la UI.

Referencias: [CLI oficial](https://code.claude.com/docs/en/cli-reference),
[MCP](https://code.claude.com/docs/en/mcp) y
[condiciones](https://code.claude.com/docs/en/legal-and-compliance).
