# Terminal del proyecto

[← Volver al índice](../README.md)

El icono de terminal abre otro panel redimensionable. Iniciar crea una sesión nativa PTY (ConPTY en Windows) en la carpeta del proyecto. Permite elegir cmd, predeterminado por su menor consumo, o PowerShell sin perfil. Recibe teclado interactivo, flechas, Tab y Ctrl+C; Ctrl+V pega y Ctrl+Shift+C copia la pantalla visible. La rueda recorre el historial.

Cada proyecto conserva su sesión al cambiar de proyecto o cerrar el panel. Detener cierra explícitamente esa sesión y sus procesos; cerrar la aplicación también libera las sesiones. Hay un máximo de ocho sesiones y 500 líneas de historial por sesión. La pantalla ANSI mantiene colores, Unicode y cursor; solo actualiza las filas modificadas y deja de refrescar la UI cuando el panel está oculto. La terminal no comparte salida ni comandos con los agentes.

Las pruebas incluyen formatos de los linters, cancelación, persistencia de logs, filtros y navegación a archivos. Una prueba real y aislada de ConPTY ejecuta comandos únicamente en un directorio temporal, verifica la carpeta, el redimensionado y el cierre. Las capturas son target/ui-lint-preview.png y target/ui-terminal-preview.png.
