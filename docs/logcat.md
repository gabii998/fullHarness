# Logcat en tiempo real

[← Volver al índice](../README.md)

El botón «Logcat» abre un panel lateral con selector de dispositivo, filtro de
texto/tag/PID, severidad mínima, pausa y seguimiento del final. Usa el flujo
continuo de [ADB Logcat](https://developer.android.com/tools/logcat), no un volcado.
Requiere Android SDK Platform-Tools y un dispositivo autorizado o emulador.
Se busca `adb` en `ANDROID_HOME`, `ANDROID_SDK_ROOT`, el SDK habitual de Windows y
el `PATH`. La lista se consulta al abrir el panel; cerrarlo y volverlo a abrir
repite la detección tras instalar ADB o conectar Android. El selector de dispositivo
ocupa la cabecera; iniciar/pausar aparece antes de la severidad. El filtro de texto
y el botón de limpiar están al pie. El botón «Logcat» de la ventana cierra el panel.
No se muestra una leyenda de estado durante la captura; los errores se informan
cuando ocurren.

Los paneles de Git, Archivos y Logcat comparten un ancho ajustable arrastrando su
borde izquierdo (280–800 px, acotado al tamaño de la ventana). El ancho se conserva
al cambiar de panel durante la sesión. El área de trabajo se ajusta en tiempo real
durante el arrastre, reservando espacio para el panel. El máximo se reduce en
ventanas pequeñas para conservar un área de trabajo utilizable; los controles
del chat pasan a dos filas cuando falta ancho. La cabecera principal conserva
su posición y las mediciones de mensajes fuera de pantalla se difieren hasta
soltar el borde.

«Iniciar» ejecuta `adb -s <serial> logcat -v brief -T 1 *:V`; no reproduce todo el
historial anterior. Los resultados llegan por lotes cada 100 ms, con una cola
acotada y una lista virtualizada. El buffer local conserva hasta 1.000 líneas o
1 MiB de texto; cada línea admite 4 KiB antes de recortarse. Las continuaciones
de stacktraces conservan la severidad de su mensaje. Los filtros operan sobre el
buffer local. «Limpiar» solo vacía ese buffer, sin borrar los logs de Android.
Al cambiar de proyecto la captura continúa; al pausar, cambiar de dispositivo,
cerrar el panel o salir del harness se detiene el proceso de Logcat.
Los logs no se guardan automáticamente ni se incorporan al contexto del agente.

La prueba `logcat_adapter` usa un ejecutable ADB simulado: verifica mensajes nuevos
mientras el proceso sigue activo, desconexiones y cancelación con la cola llena.
No usa dispositivos, cuentas ni inferencia reales.
