# Verificación

[← Volver al índice](../README.md)

```powershell
cargo fmt --check
cargo test --offline
cargo build --offline
```

Las pruebas cubren navegación y borradores, persistencia, migración de preferencias, deduplicación de consumo, retornos OAuth inválidos, cifrado de credenciales, streaming fragmentado, límites de herramientas y cancelación de comandos. Una prueba con servidor HTTP local verifica el ciclo completo: solicitud de escritura, espera de aprobación, ejecución de nuestra herramienta, respuesta mínima y uso separado de cada generación. No utiliza una cuenta real ni consume inferencia. La prueba de UI genera `target/ui-preview.png` y `target/ui-settings-preview.png` con el renderizador software.

El 9 de octubre de 2026 se verificó el acceso nativo con una cuenta real:
autorización por dispositivo, consulta de cuota, catálogo de siete modelos,
persistencia entre procesos e inferencia por SSE con las herramientas del harness
registradas. Una prueba mínima con GPT-6-Luna respondió «OK» y reportó 318 tokens
de entrada y 5 de salida, sin razonamiento adicional. No ejecutó herramientas.
`cargo run -- --check-codex` repite esa prueba y consume una solicitud; no se
ejecuta automáticamente al abrir la aplicación. Claude tambien se verifico con el cliente oficial, una lectura MCP real y una respuesta visible.

## Medir el scroll sin cambiar su comportamiento

En **Configuración → Medir scroll**, la aplicación activa un diagnóstico temporal y cierra el diálogo. Desplazarse unos 10 segundos en cada vista (chat, archivo y cambios/historial de Git), usando el mismo ratón y movimientos similares. Volver a Configuración y pulsar **Detener medición**. La captura no requiere permisos de administrador y no inyecta entradas.

Los archivos se guardan en `<directorio de datos>/logs/performance/scroll-*.jsonl`. No incluyen contenido, nombres de archivos, credenciales ni respuestas. El diagnóstico está desactivado por defecto, tiene un límite de 4 MiB por captura y escribe en un hilo separado; si la cola se llena descarta muestras en vez de bloquear la interfaz.

Para resumir la captura más reciente:

```powershell
powershell -NoProfile -File scripts/summarize-scroll.ps1
```

El resumen separa cada vista y muestra percentiles p50/p95/p99 de tiempo de renderizado en CPU, tiempo desde la rueda hasta comenzar/terminar el renderizado, intervalos entre cuadros y cantidad de filas creadas. Registra la frecuencia reportada por el monitor y si el ejecutable es debug. A 144 Hz el presupuesto por cuadro es aproximadamente 6,94 ms.

**No confundir estas medidas con tiempos GPU ni FPS de presentación:** el notificador mide el trabajo de CPU hasta completar la entrega del renderizado; los intervalos también dependen de la frecuencia de entrada del ratón. Comparar debug y release con el mismo archivo/repositorio y repetir cada captura antes de atribuir una mejora a un cambio. Un coste alto de renderizado apunta a dibujo/texto; una espera alta antes del renderizado requiere investigar layout, callbacks, bloqueo del hilo UI o programación de cuadros. La cantidad de filas creadas permite evaluar si la virtualización está provocando trabajo repetido, pero no cuenta por sí sola todas las reevaluaciones de bindings.
### Comparar cambios de rendimiento

La compilación de desarrollo usa `opt-level = 1` para el harness y su UI generada, y `opt-level = 3` para las dependencias de renderizado. Conserva símbolos, comprobaciones y assertions de debug; no es equivalente a release y puede hacer que algunas variables sean menos fáciles de inspeccionar al depurar. La primera compilación recompila las dependencias y tarda más. El perfil de test hereda los valores de desarrollo.

Las capturas nuevas incluyen `opt_level`, `build_profile`, dimensiones de ventana y estado de los paneles. Para comparar una referencia con la última captura:

```powershell
powershell -NoProfile -File scripts/compare-scroll.ps1 -Baseline '<ruta de la captura anterior.jsonl>'
```

Un porcentaje negativo indica menor tiempo medido, no una prueba automática de causalidad. Mantener los mismos archivos, paneles abiertos, tamaño de ventana, escala, movimientos del ratón y estado de los agentes. Repetir al menos dos veces. No modificar a la vez la virtualización y el perfil de compilación: medir primero el efecto aislado de optimizar la compilación. El tiempo entre los notificadores es tiempo transcurrido en el tramo de renderizado, y también puede incluir esperas del driver; no equivale necesariamente a uso puro de CPU.