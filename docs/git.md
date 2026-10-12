# Git

[← Volver al índice](../README.md)

La opción **Git** del selector de paneles abre el panel por proyecto con las pestañas «Cambios» e «Historial»
en su cabecera: rama y upstream, adelanto/atraso
cuando Git los informa localmente, archivos preparados, modificaciones, nuevos
archivos y conflictos. Si no existe repositorio, permite inicializarlo. Si falta
un remoto, admite vincular la URL o ruta de `origin`.

El diálogo **Configuración de Git** muestra los valores efectivos de `user.name`
y `user.email`, incluyendo los heredados, y permite editarlos junto a `origin`.
Los cambios de identidad se escriben únicamente con `git config --local` en el
repositorio actual. Los campos vacíos conservan su configuración; no se modifica
la configuración global ni se necesita un remoto para guardar la identidad.

**Cambios** muestra filas compactas agrupadas en «Preparados» y «Cambios», con
checkboxes para elegir qué archivos incluir en el commit. Marcarlos prepara el
archivo; desmarcarlos lo retira del índice conservando su contenido de trabajo. Cada fila muestra
las líneas agregadas y eliminadas informadas por `git diff --numstat`, separadas
entre índice y trabajo. Un archivo parcialmente preparado aparece en ambos grupos.
Al pulsar su nombre, la pestaña muestra el diff de ese grupo. Los binarios se
identifican como tales; `—` indica estadísticas no disponibles, nunca un cero
estimado. Para archivos nuevos se consultan hasta 100 archivos de hasta 8 MiB,
con dos procesos concurrentes y un límite conjunto de cinco segundos. Las
consultas no preparan archivos ni modifican el índice. El commit incluye únicamente los
archivos preparados y usa el mensaje ingresado, la identidad y los hooks de Git.
Su cuadro de escritura comparte el estilo del chat y admite mensajes multilínea;
el borrador se conserva al alternar las pestañas del panel.
**Historial** muestra los últimos 50 commits; al seleccionar uno se abre su diff.
La pestaña del diff se abre inmediatamente con «Leyendo diferencias…» en su
barra inferior; el resultado o error se guarda allí sin volver a enfocarla ni
reabrirla si se cerró durante la lectura. Las vistas de diferencias admiten hasta
128 KiB y no se envían al agente. Actualizar, Pull y Push usan botones de icono;
el panel conserva la rama sin las leyendas de primer commit o conteo global.

**Pull** usa `--ff-only`: una divergencia se informa sin crear un merge automático.
**Push** respeta la configuración de Git; si la rama aún no tiene upstream, lo
configura con `origin` o con el único remoto disponible. Estas acciones usan las
credenciales de Git del equipo y muestran los errores de autenticación, identidad
o conflictos. No se fuerza un push. Requiere Git instalado.

La consulta de estado no actualiza el índice ni accede a la red. Se refresca al
abrir el panel, cambiar de proyecto, terminar una tarea o una operación de Git, o
pulsar **Actualizar**. Las operaciones corren en segundo plano, con salida acotada
y un límite de 10 segundos para lecturas, 30 para preparación y 120 para commit,
pull o push. El ancho del panel y el área de trabajo se ajustan al arrastrar su borde.



El panel Git permite seleccionar ramas locales desde la barra de acciones. El harness bloquea el cambio si hay archivos modificados, preparados o nuevos sin commitear; vuelve a consultar el estado antes de ejecutar el cambio. No descarta cambios ni crea ramas automáticamente.

En Git, la ruta aparece debajo del nombre de cada archivo. El visor de diferencias resalta líneas añadidas y eliminadas sin perder la virtualización ni la caché acotada. El botón de ajustes abre el diálogo para crear o editar la URL de origin; los errores quedan en el diálogo y este se cierra al guardar correctamente.
