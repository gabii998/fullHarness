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
No usa IA ni incorpora los logs al contexto. Límites por actualización: 1.000
logs, 48 MiB, 10.000 llamadas; la vista individual muestra las primeras 500.
Los informes JSON locales quedan en `metrics/<proyecto>/project-report.json` y
`chat-report.json`, con fecha, alcance, cifras desconocidas y cobertura parcial.
El botón de carpeta abre esos informes. La pestaña sigue el proyecto seleccionado.
