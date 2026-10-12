# Problemas del linter

[← Volver al índice](../README.md)

El icono de advertencia abre un panel redimensionable. Detecta herramientas según los archivos del proyecto y permite ejecutar el linter seleccionado, cancelar, filtrar por severidad o texto y abrir un diagnóstico en una pestaña de archivo, resaltando su línea. La ejecución es manual, sin IA y sin aplicar fixes. El panel conserva el resultado de la última ejecución entre aperturas; los resultados guardados se identifican como tales.

| Tecnología | Adaptador automático |
| --- | --- |
| JavaScript / TypeScript | ESLint local de node_modules, salida JSON |
| Rust | Cargo Clippy en todos los targets, diagnósticos JSON |
| Python | Ruff, desde .venv o PATH, salida JSON |
| Android (Java / Kotlin) | Gradle Android Lint, informes XML |
| Java / Kotlin | Gradle Checkstyle o Detekt; Maven Checkstyle |
| Go | go vet ./... |
| C# / .NET | dotnet build con los analizadores habilitados en el proyecto |
| C / C++ | Clang-Tidy, requiere compile_commands.json |
| PHP | PHPStan local de Composer |
| Ruby | RuboCop, salida JSON |
| Dart / Flutter | dart analyze, formato machine |
| Swift | SwiftLint, salida JSON |
| Shell | ShellCheck, salida JSON |

Las herramientas y sus SDK deben estar instalados. Android/Gradle necesitan el wrapper y reportes XML habilitados. Los informes de Checkstyle/Detekt se leen en build/reports; Maven en target/checkstyle-result.xml o target/site/checkstyle.xml. Los comandos se ejecutan con argumentos separados, desde la carpeta del proyecto. Los procesos tienen un timeout de cinco minutos, salida acotada y cancelación de sus descendientes en Windows. Un fallo de ejecución, un formato inválido o un límite excedido se muestra explícitamente; no se transforma en cero errores. Una ejecución con fallo de compilación puede devolver un informe parcial.

El botón de ajustes permite registrar otras herramientas. Se guardan en .harness/lint.json como una lista de adaptadores con ejecutable, argumentos JSON y formato de salida. Admite los formatos nativos anteriores, SARIF, Checkstyle, texto archivo:línea:columna:mensaje y JSON normalizado del harness. Ejemplo:

```json
[
  {
    "name": "Mi linter",
    "kind": "Json",
    "program": "mi-linter",
    "args": ["--format", "json"],
    "reports": false
  }
]
```

El formato Json normalizado es un array de objetos con path, line, column, error (booleano), code y message. La salida completa acotada, stderr, código de salida, comando, duración y resultado normalizado se guardan bajo la carpeta local de datos del harness, en lint/<hash-del-proyecto>. Se conserva la última ejecución por proyecto: hasta 8 MiB de stdout y 256 KiB de stderr, con límite de 20000 diagnósticos. El botón de carpeta permite abrir esos logs. No entran en el contexto de los agentes.
