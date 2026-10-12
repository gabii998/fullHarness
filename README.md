# Ascurra Harness

Aplicación de escritorio en Rust + Slint para trabajar con múltiples repositorios y chats. Codex se conecta directamente por OAuth y HTTP/SSE: el ciclo de agente y las herramientas pertenecen al harness. No requiere instalar Codex CLI. Claude se integra mediante el ejecutable nativo de Claude Code y un puente MCP local a las mismas herramientas.

## Índice de documentación

### Primeros pasos

- [Instalación y ejecución](docs/ejecutar.md)

### Agentes

- [Conectar Codex](docs/codex.md)
- [Claude Code](docs/claude.md)
- [Ejecución y herramientas](docs/herramientas.md)

### Proyecto e interfaz

- [Explorador y visor de archivos](docs/archivos.md)
- [Git](docs/git.md)
- [Proyectos, configuración y tareas](docs/proyectos.md)

### Paneles y utilidades

- [Logcat en tiempo real](docs/logcat.md)
- [Diagrama de Prisma](docs/prisma.md)
- [Contenedores Docker](docs/docker.md)
- [Problemas del linter](docs/linters.md)
- [Terminal del proyecto](docs/terminal.md)
- [Peticiones HTTP](docs/http.md)
- [Imágenes del agente](docs/capturas.md)
- [Imágenes adjuntas al chat](docs/adjuntos.md)

### Métricas y almacenamiento

- [Tokens y contexto](docs/tokens.md)
- [Persistencia e historiales](docs/persistencia.md)
- [Cuota de Codex](docs/cuota.md)
- [Uso](docs/uso.md)

### Desarrollo

- [Verificación](docs/verificacion.md)
- [Parches locales y dependencias vendorizadas](vendor/README.md)
