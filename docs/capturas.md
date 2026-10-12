# Imágenes del agente

[← Volver al índice](../README.md)

La opción **Imágenes** del selector abre una galería lateral redimensionable
para acceder a imágenes devueltas por el agente: diagramas, gráficos,
ilustraciones, capturas u otros resultados visuales.
Los adjuntos enviados por el usuario se gestionan desde el cuadro del chat,
mediante el botón de clip o Ctrl+V; no se agregan a esta galería.
Se puede consultar el proyecto completo o el chat actual, seleccionar una imagen
y ampliar su vista previa. Esta vista es de consulta, sin importación manual.
La galería se actualiza al abrir, cambiar de proyecto/chat y
terminar un turno del agente; también tiene actualización manual.

Las imágenes PNG y JPEG quedan en los datos de la aplicación, bajo
`artifacts/<chat>/<referencia>/`. El agente puede registrar una imagen del
repositorio mediante `register_image`, disponible en Codex y en el MCP de
Claude. Los comandos aprobados reciben `HARNESS_ARTIFACT_DIR` para guardar
directamente sus imágenes en el artefacto de esa ejecución. No se rastrea ni
se copia automáticamente cualquier imagen del repositorio.

La herramienta devuelve solo estado y referencia, nunca píxeles ni base64.
Las imágenes no se agregan automáticamente al contexto. Los logs conservan el
registro de la llamada y su referencia. La galería carga una imagen por vez,
en segundo plano, con una vista previa de hasta 1200 × 900. Se admiten archivos
de hasta 16 MiB y 16 megapíxeles. El listado muestra hasta 200 imágenes y avisa
si su exploración es parcial. No sigue enlaces de archivos o carpetas.
