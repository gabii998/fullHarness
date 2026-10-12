# Explorador y visor de archivos

[← Volver al índice](../README.md)

El botón **Archivos** abre un explorador del proyecto junto al chat. Las carpetas
se cargan al desplegarlas; sus expansiones se conservan durante la sesión al cambiar
de proyecto. **Actualizar** vuelve a cargar la raíz y cierra las expansiones.
Un clic en un archivo abre una pestaña de solo lectura de texto UTF-8 junto a los
chats (hasta 128 KiB, con aviso si está recortada). Reabrirlo selecciona la misma
pestaña; cada archivo puede cerrarse con ×. El visor dibuja solo las líneas visibles,
incluye números de línea y desplazamiento horizontal. La barra inferior muestra
el encoding, el estado de solo lectura y avisos de carga, errores o recorte.
Las pestañas conservan su contenido durante la sesión al cambiar de chat o proyecto.
Los archivos binarios muestran un aviso.
**Abrir carpeta** abre el proyecto en el Explorador de Windows.
La vista admite hasta 1000 entradas por carpeta, 4000 entradas visibles y 24 niveles.
Los enlaces que salen del proyecto no se pueden explorar ni previsualizar.
Las lecturas se hacen en segundo plano y no se agregan al historial del agente.
Archivos y Git comparten el panel lateral; alternarlos conserva los chats y trabajos.
