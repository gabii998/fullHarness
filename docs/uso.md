# Uso

[← Volver al índice](../README.md)

El botón **Uso** abre una pestaña con métricas del proyecto o del chat actual.
Incluye rankings de herramientas, llamadas individuales, modelos y procesos.
Los totales de modelo proceden de los registros del proveedor, sin volver a sumar
caché o razonamiento. Las herramientas usan una estimación explícita de una
exposición de sus argumentos y respuesta (caracteres / 4); no es facturación ni
incluye las posteriores relecturas del contexto. En logs antiguos faltan los
argumentos: se estima únicamente la respuesta por bytes y se marca como parcial.
Los procesos vinculan `run_command` con las consultas `inspect_result` de su
resultado cuando los logs conservan esa referencia.

La lectura ocurre en segundo plano, al abrir, actualizar o terminar un mensaje.
No usa IA ni incorpora los logs al contexto. Límites por actualización: los 1.000
logs más recientes del alcance seleccionado, 48 MiB, 10.000 llamadas; la vista
individual muestra las primeras 500 del ranking por estimación de tokens.
La selección de logs recorre los metadatos disponibles y retiene como máximo
1.000 candidatos en memoria. Las lecturas se serializan: las actualizaciones
obsoletas pendientes se omiten y las ya iniciadas no exportan tras detectar que
fueron reemplazadas. No hay todavía caché incremental ni cancelación del escaneo
en curso.

Los informes JSON locales quedan en `metrics/<proyecto>/project-report.json` y
`chat-report.json`, con fecha, alcance, cifras desconocidas y cobertura parcial.
El botón de carpeta abre esos informes. La pestaña sigue el proyecto seleccionado.

## Cobertura y relecturas

Los informes incluyen el número de logs encontrados, motivos de truncamiento y
las fechas mínima y máxima de las llamadas analizadas que tengan timestamp.
Las llamadas conservan `offset`, `limit` y `stream` cuando existen en los logs.
Se cuentan inspecciones, bytes de respuestas de lectura y relecturas exactas:
misma herramienta, objetivo, rango y stream dentro de la misma ejecución y chat.
Una relectura no implica desperdicio: un archivo o resultado puede haber cambiado.
No se detectan todavía rangos parcialmente solapados; los registros antiguos sin
rango no se cuentan como relecturas. Los bytes incluyen la respuesta serializada,
no solamente el contenido del archivo.

## Durabilidad de logs

Una respuesta de herramienta mayor de 128 KiB se omite como detalle y genera un
evento `tool_detail_omitted`, sin detener los eventos posteriores. Se reserva
1 MiB del presupuesto total de 16 MiB para eventos en vez de detalles.
Los fallos reales de escritura o el agotamiento total siguen marcando el log
como incompleto.

Los eventos se escriben inmediatamente, pero se sincronizan a disco al acumular
64 KiB, al registrar un evento después de un segundo desde la última sincronización,
en aprobaciones, fallos, cancelaciones y al finalizar la tarea. También se intenta
sincronizar al destruir el journal. No hay temporizador de fondo: una terminación
abrupta puede perder los eventos aún no sincronizados; el intento al destruir el
journal no puede informar errores.
