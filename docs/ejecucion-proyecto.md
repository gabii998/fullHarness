# Ejecución del proyecto

[← Volver al índice](../README.md)

Seleccionar **Ejecución** en el selector de paneles abre el supervisor de procesos del proyecto. Es independiente de la terminal interactiva: la interfaz y los agentes Codex/Claude comparten las mismas ejecuciones. Cerrar el panel, cambiar de proyecto o finalizar un turno del agente no detiene un servidor. **Detener** termina la ejecución y sus descendientes; **Reiniciar** detiene las ejecuciones activas de la configuración elegida antes de iniciarla nuevamente. Cerrar normalmente la aplicación solicita el cierre de las ejecuciones.

## Configuraciones y detección

La primera apertura propone y guarda configuraciones, sin ejecutar comandos. **Detectar** incorpora configuraciones nuevas sin reemplazar las editadas. La inspección es acotada: hasta 100 directorios, tres niveles y manifiestos de hasta 256 KiB. No evalúa archivos Gradle ni instala dependencias.

- **Node:** scripts de `package.json`; npm, pnpm o Yarn según `packageManager` y lockfiles. Incluye proyectos anidados, como `packages/web`. En Windows usa los ejecutables `.cmd`.
- **Kotlin / Spring:** Gradle Wrapper y tareas `bootRun`, `test`, `build`; Maven Wrapper y `spring-boot:run`, `test`, `package`. Las configuraciones permiten ajustar módulos, perfiles y argumentos. La detección de plugins es orientativa; aliases o builds dinámicos pueden necesitar una configuración manual.
- **Android:** Gradle Wrapper, módulo de aplicación y tareas Debug de compilación, instalación, tests unitarios e instrumentados. La sección Android permite elegir módulo, variante, dispositivo ADB autorizado y componente `paquete/actividad`; guardar crea configuraciones para esos valores. Los tests instrumentados y la instalación requieren un dispositivo o emulador ya iniciado. No administra AVDs ni instala el SDK.

**Actualizar dispositivos** reutiliza la detección ADB del panel Logcat. Busca ADB en `ANDROID_HOME`, `ANDROID_SDK_ROOT`, el SDK local de Windows y `PATH`. No ofrece dispositivos offline/no autorizados. También se puede introducir el serial manualmente. **Abrir Logcat** cambia al panel existente; su selección y filtros se administran allí.

Lanzar la app utiliza `adb shell am start -n`; detener la app utiliza `adb shell am force-stop`. **Detener una ejecución de Gradle/ADB no equivale a detener la app instalada**: para eso se ejecuta la configuración `stop-app`. El estado mostrado corresponde al comando, no a una supervisión permanente del proceso Android. Compilar, instalar y lanzar son acciones separadas en esta versión.

## Edición

**Configurar** permite editar un array JSON de configuraciones:

```json
[
  {
    "id": "web-dev",
    "name": "Frontend · desarrollo",
    "program": "npm.cmd",
    "args": ["run", "dev"],
    "directory": "web",
    "env": {"PORT": "3000"}
  }
]
```

Usar `npm` en lugar de `npm.cmd` fuera de Windows. `directory` es relativo al proyecto y debe existir; no admite escapes mediante `..` ni enlaces hacia fuera. Los argumentos son un array, no una línea de shell. Si se elige explícitamente un shell, sus capacidades forman parte del comando autorizado: no hay sandbox del sistema operativo.

Las configuraciones se guardan por ruta canónica del repositorio en `<datos>/project-execution/`. **Las variables se guardan en texto plano local**: evitar almacenar credenciales innecesarias. Cada configuración admite 16 KiB, hasta 128 argumentos y hasta 128 configuraciones por proyecto; el archivo combinado tiene un límite de 256 KiB.

## Estado y logs

Hay hasta ocho ejecuciones simultáneas y se rechazan duplicados activos de la misma configuración. El selector de ejecuciones muestra estado, duración y código de salida. stdout y stderr se capturan por separado, con un límite de 1 MiB por flujo; el panel muestra los últimos 8 KiB capturados. La salida adicional se drena sin almacenarse. Los resultados se guardan en `<datos>/project-runs/<id>/`, junto con metadatos finales y la indicación de truncamiento.

El historial de procesos es de la sesión de la aplicación y conserva hasta 64 entradas en memoria. Las configuraciones y los logs permanecen en disco, pero esta versión no restaura procesos ni carga ejecuciones antiguas al reiniciar, y no elimina automáticamente los logs históricos.

## Acceso del agente

Las herramientas se publican para Codex y mediante el mismo registro MCP para Claude:

- `project_executions`: configuraciones y estados paginados. No devuelve valores de variables de entorno y los detalles largos pueden recortarse.
- `project_execution_action`: `start`, `stop`, `restart` o `save`, con aprobación. La aprobación muestra el proyecto, la acción y la configuración; iniciar/reiniciar usa la copia mostrada aunque la configuración guardada cambie mientras se espera. Cada aprobación se consume una vez. La respuesta contiene solo estado y referencia; `inspect_result` permite conocer el ID generado.
- `project_execution_logs`: lectura explícita de hasta 8192 bytes de stdout, stderr o metadatos, por ID y desplazamiento. Solo permite ejecuciones del repositorio del chat.

Las consultas no lanzan tareas del proyecto. Las acciones aprobadas pueden mantener servidores activos más allá del turno del agente; detener el chat no las detiene. La interfaz refresca las configuraciones y los estados compartidos, sin insertar automáticamente logs en el contexto del agente.

## Verificación

Se añadieron pruebas para detección Node/Spring/Android/Maven, persistencia, límites, aislamiento entre repositorios, prevención de duplicados, cancelación, aprobaciones de un solo uso y conservación del comando aprobado. Ejecutar `cargo test --offline` en un entorno Rust con el toolchain nativo completo. No requieren ejecutar proyectos reales ni una cuenta de agente.
