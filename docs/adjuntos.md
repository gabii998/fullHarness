# Imágenes adjuntas al chat

[← Volver al índice](../README.md)

El botón de clip junto al mensaje permite seleccionar imágenes PNG/JPEG.
También se puede pegar una imagen con **Ctrl+V** mientras el cuadro de escritura
tiene el foco. Si el portapapeles contiene texto, se mantiene el pegado habitual.
Las miniaturas aparecen encima del cuadro y su botón **×** permite quitarlas
antes de enviar. Se puede enviar un mensaje compuesto únicamente por imágenes.

Se admiten hasta cuatro imágenes por mensaje, cada una de hasta 5 MiB y
16 megapíxeles. La carga y conversión se realizan en segundo plano; el envío
espera a que terminen. Las imágenes se asocian al chat desde el que se abrió
el selector, aunque durante la carga se cambie de chat o proyecto. El borrador
y los adjuntos persisten al reiniciar la aplicación.

Los archivos se guardan bajo `attachments/<chat>/` en los datos de la aplicación,
con una miniatura pequeña y referencias en el estado del chat. Se conserva el
PNG/JPEG original para el envío; el portapapeles se convierte a PNG. Los adjuntos
del usuario no aparecen en la galería de resultados del agente.

Codex recibe bloques `input_image` con data URLs según la
[documentación oficial de OpenAI](https://developers.openai.com/api/docs/guides/images-vision).
Claude recibe bloques `image` con fuente base64 en su entrada `stream-json`,
con el formato de [Vision](https://platform.claude.com/docs/en/build-with-claude/vision).
El historial del harness guarda referencias locales: el base64 se prepara al
enviar, no se duplica en `state.json`, los historiales ni los logs del harness.
El cliente oficial Claude puede guardar las imágenes en su propia sesión.

Los adjuntos entran en el contexto y su consumo se incluye en las métricas que
reporta el proveedor. En Codex se reenvían las imágenes del historial al realizar
consultas posteriores; se admite hasta 20 MiB de imágenes en una petición.
Los logs del harness registran cantidad y tamaño, sin guardar los píxeles.

Las pruebas utilizan un servidor HTTP local para Codex y un proceso Claude
simulado para comprobar el transporte. La prueba de interfaz usa fuentes de
archivos y portapapeles simuladas, sin modificar el portapapeles del usuario
ni consumir inferencia en sus cuentas.
