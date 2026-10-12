# Contenedores Docker

[← Volver al índice](../README.md)

El icono Docker abre el panel del proyecto actual. Requiere Docker instalado y el daemon disponible. Muestra contenedores de Docker Compose asociados a la ruta exacta del proyecto mediante sus etiquetas, con imagen, servicio, puertos publicados, estado y salud. Permite iniciar, detener, reiniciar y consultar las últimas 200 líneas de logs. Para proyectos con compose.yaml, compose.yml, docker-compose.yaml o docker-compose.yml en la raíz, Levantar servicios Compose ejecuta compose up -d con el archivo detectado y su override convencional si existe; no busca archivos en carpetas superiores. Esta primera versión no asocia contenedores creados con docker run sin etiquetas de Compose.

Las acciones se ejecutan en segundo plano y vuelven a comprobar la pertenencia del contenedor antes de actuar. El panel refresca al abrir, cambiar de proyecto o finalizar una acción; el botón de actualización permite consultar cambios externos. Los logs son una consulta acotada, no un stream permanente. Los procesos tienen timeout, salida limitada y no abren ventanas de consola. Las pruebas usan un ejecutable Docker simulado, sin acceder a tus contenedores ni a una cuenta de IA. Las capturas de prueba están en target/ui-prisma-preview.png y target/ui-docker-preview.png.
