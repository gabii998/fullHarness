# Flujos del navegador y exportación de tests

Abrí **Navegador / Flujos** en el selector de paneles del proyecto.

## Qué registra

Cada turno del agente que usa `browser` genera un flujo independiente. Se guardan automáticamente sus acciones, resultado, duración, localizador exportable y referencia al artefacto. El registro persiste en el directorio de datos del harness, separado por proyecto; no ensucia el repositorio. Las acciones concurrentes se serializan para conservar el orden.

El panel lista hasta 100 flujos recientes. **Actualizar** vuelve a leer el registro, incluso mientras el agente está ejecutándose. Los flujos se identifican por chat e ID. Los pasos fallidos, snapshots y cierres están excluidos por defecto.

Los flujos anteriores a esta funcionalidad no se reconstruyen a partir de los artefactos.

## Revisar y guardar

1. Elegí un flujo.
2. Marcá los pasos que querés incluir. Hacé clic en un paso para consultar el localizador y el error; los pasos `screenshot` muestran su imagen si el artefacto todavía existe.
3. Definí nombre del test, URL base y ruta relativa de salida (`*.spec.ts`). El directorio destino debe existir.
4. Añadí, si hace falta, una verificación al final: texto esperado, URL exacta o elemento visible mediante CSS. Estas verificaciones manuales todavía no se han ejecutado: se ejecutarán al correr el test.
5. Pulsá **Previsualizar test…**. Revisá el código y confirmá **Guardar en el proyecto**.

La confirmación guarda exactamente el código y la ruta mostrados en la vista previa. No ejecuta nada, no instala dependencias y **no sobrescribe tests existentes**. Rechaza rutas absolutas, `..`, destinos fuera del proyecto y enlaces simbólicos finales existentes. Si un paso falló o contiene una referencia temporal sin localizador resuelto, hay que excluirlo antes de exportar.

Las ediciones de selección, nombre, URL base y assertions son locales al panel. Cambiar de flujo, actualizar o cambiar de proyecto las descarta; el registro original no se modifica. El archivo exportado es la versión editable y duradera del test.

## Localizadores y datos sensibles

Las referencias `@e1` del snapshot no se exportan. Al interactuar con una referencia, el worker registra un localizador: `data-testid`, etiqueta del formulario o CSS. Los selectores CSS explícitos se conservan. Cuando no hay una identificación mejor se usa la ruta estructural del DOM (`nth-of-type`): puede ser frágil y debe revisarse. Todavía no hay un editor visual de localizadores; se pueden editar en el archivo exportado.

**Los valores `fill` no se guardan en el registro del flujo ni en el test.** Cada uno se sustituye por una variable como `FLOW_VALUE_3`. Los errores de esos pasos también se omiten del registro para no persistir el valor a través de los mensajes de Playwright. Los valores introducidos siguen sujetos a las reglas de aprobación y artefactos de la herramienta; este registro no borra el historial del chat.

Las URLs, selectores y verificaciones sí se registran. Evitá poner tokens o datos sensibles en ellos. Las capturas pueden contener información sensible visible en la página.

## Ejecutar el test generado

El proyecto necesita el runner de tests, además del Chromium usado por la herramienta:

```sh
npm install --save-dev @playwright/test
npx playwright install chromium
```

Ejemplo en PowerShell:

```powershell
$env:FLOW_BASE_URL = 'http://localhost:3000'
$env:FLOW_VALUE_3 = 'demo@example.com'
npx playwright test browser-flow-xxxxxxxx.spec.ts
```

El nombre de cada variable aparece en la vista previa. Las navegaciones que parten de la URL base se convierten en rutas relativas a `FLOW_BASE_URL`; las URLs de otros orígenes se mantienen. Configurá el `testDir` del proyecto para incluir el archivo elegido. Las capturas del test usan el directorio de resultados de Playwright.

Esta primera versión permite **registro, revisión, assertions, capturas y exportación**. No incluye aún reproducción o ejecución de tests desde el panel, reordenamiento de pasos, editor visual avanzado ni aprobación por flujo completo: cada llamada del agente conserva su aprobación individual.
