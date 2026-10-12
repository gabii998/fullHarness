# Instalación y ejecución

[← Volver al índice](../README.md)

Requiere Rust estable y las herramientas C++ de Visual Studio en Windows.

```powershell
cargo run
```

Los datos se guardan en `%LOCALAPPDATA%\AscurraHarness`. Para otro directorio:

```powershell
$env:HARNESS_DATA_DIR = Join-Path (Get-Location) '.harness-data'
cargo run
```

«Abrir proyecto» permite elegir un directorio con «Buscar carpeta…». Los proyectos, chats, borradores y solicitudes terminadas se guardan localmente. Cambiar de proyecto o cerrar una pestaña conserva las ejecuciones. Cerrar la aplicación cancela sus trabajos.

La interfaz usa FemtoVG con aceleración OpenGL por defecto. El renderizador software sigue disponible como alternativa (`SLINT_BACKEND=winit-software`) e incluye un parche local de presentación en Windows para repintar áreas que estuvieron tapadas por la barra de tareas. Detalles en [documentación de los parches locales](../vendor/README.md).

En Windows, la superficie GPU no solicita un buffer de profundidad para el dibujo
2D; conserva el stencil. El visor comparte el texto del archivo y materializa
líneas bajo demanda con una caché máxima de 128 líneas, sin duplicar cada línea
del documento al abrirlo. Al mostrar una pestaña de archivo se retiran los
elementos gráficos de los mensajes, conservando el historial. Los paneles laterales
se animan mediante desplazamiento sin opacidad de grupo; los diálogos animan
el alfa del fondo para evitar una textura intermedia de toda la superficie.
El ahorro adicional de estas técnicas todavía no se cuantificó.

`scripts/measure-renderer.ps1` compara procesos nuevos con la misma ventana vacía
y guarda RAM residente, pico de memoria privada y datos del driver en JSON.
No inicializa proveedores ni consume inferencia. La prueba con AMD Radeon 860M
dio medianas de 149,9 MiB antes y 141,9 MiB después del ajuste de superficie,
en tres ejecuciones por configuración. La memoria privada reservada no bajó;
estas métricas tampoco miden VRAM ni garantizan el mismo ahorro en otro driver.
