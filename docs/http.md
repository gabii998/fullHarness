# Peticiones HTTP

[← Volver al índice](../README.md)

La opción **HTTP** del selector de paneles abre un panel lateral redimensionable
con colecciones y requests persistentes por
proyecto. Permite configurar método, URL, headers (`Nombre: valor` por línea),
body, timeout de 1 a 120 segundos y variables mediante un objeto JSON con valores
de texto. `{{NOMBRE}}` se sustituye en URL, headers y body al enviar. No hay
sustitución recursiva ni extracción automática de variables de las respuestas.
Guardar aplica la request y los cambios de nombre/exposición de la colección.

Las colecciones se guardan en los datos de la aplicación, bajo
`http/<proyecto>/collections.json`, fuera del repositorio. Las variables no se
incluyen en las instrucciones ni en los listados de herramientas del agente.
Cada colección empieza sin exposición: **Exponer a agentes** y **Guardar**
habilitan su uso. El puente MCP de Claude y las herramientas nativas de Codex
ofrecen las mismas funciones:

- `http_collections`: descubrir colecciones/requests expuestas; paginación de
  hasta 20 elementos, solo IDs, nombres y métodos.
- `http_request`: ejecutar una request guardada por sus IDs, después de aprobación.
  Se congela la versión aprobada y se comprueba que la exposición no haya sido
  revocada. El resultado automático contiene estado y referencia; la respuesta
  se consulta con `inspect_result`, en lecturas de hasta 8 KiB.

El cliente manual no usa IA. TLS se verifica, no se siguen redirecciones y no
hay reintentos automáticos. Se captura hasta 1 MiB de respuesta; la vista virtual
muestra hasta 64 KiB. La última respuesta manual por request se guarda en
`http/<proyecto>/responses/<request>/`; las ejecuciones del agente conservan sus
propios artefactos. Se pueden consultar status, duración, headers y body, o
cancelar el envío. Cambiar de proyecto no redirige una request en curso.
