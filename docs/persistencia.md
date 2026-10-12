# Persistencia e historiales

[← Volver al índice](../README.md)

El índice `state.json` conserva metadatos, borradores y métricas. Los mensajes y el
historial del proveedor se guardan en `histories/<chat>-<sha256>.json`. Al abrir se
carga el chat seleccionado; los chats inactivos se descargan después de guardar.
Las tareas en ejecución y las esperas de aprobación permanecen en memoria.
El formato anterior con historial inline se migra automáticamente: primero se
publican los archivos de historial y después se reemplaza el índice atómicamente.
Un fallo de lectura se informa y bloquea el envío; no se reemplaza por un chat vacío.
Las versiones anteriores de la aplicación no deben usarse sobre este nuevo índice.
