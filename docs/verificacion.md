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
