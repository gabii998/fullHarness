# Tokens y contexto

[← Volver al índice](../README.md)

El cuadro de escritura integra un indicador compacto con `input_tokens` de la **última solicitud**
y la ventana `context_window` publicada en el catálogo de la cuenta para ese modelo.
Incluye los tokens de caché; no suma el consumo de solicitudes anteriores.
La respuesta recién generada y el borrador posterior no están incluidos en esa
medición. No es un conteo exacto del próximo envío. Un límite o una medición ausente
se muestran como desconocidos; cambiar de modelo invalida la comparación anterior.
La información aparece junto a la barra, alineada a la derecha del cuadro de escritura.
Consultar el catálogo no ejecuta inferencia. El historial largo se puede desplazar
manualmente; mientras el agente escribe, la vista sigue el final de la conversación
con desplazamiento animado. El streaming agrupa los fragmentos durante 16 ms y actualiza únicamente la última
fila, sin recorrer el historial ni recrear la navegación y las pestañas. Una respuesta
final o un cambio de chat muestra el texto acumulado sin esperar ese intervalo.
El chat mide las alturas al abrir o cambiar el ancho y las conserva en contenedores
ligeros; solo mantiene el texto y las burbujas dentro del viewport y un margen de
200 px a cada lado. Esto conserva la geometría del scroll al retirar mensajes
fuera de pantalla. El primer pase de medición aún depende del tamaño del historial.

Se registra el uso que informa cada solicitud, deduplicado por su identificador. Entrada y salida forman los totales; caché y razonamiento se muestran por separado porque son subconjuntos. Un dato ausente permanece desconocido. Si una solicitud de inferencia se interrumpe sin informar uso, la UI marca el total como parcial. Los errores anteriores al envío no se clasifican como consumo desconocido.

Estas métricas describen tokens reportados; no representan una factura ni el saldo restante de la suscripción.
