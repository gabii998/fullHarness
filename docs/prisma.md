# Diagrama de Prisma

[← Volver al índice](../README.md)

El icono de base de datos de la cabecera abre un panel redimensionable. Lee el esquema del proyecto y dibuja modelos, vistas, enums, campos, claves PK/FK/UQ y relaciones con cardinalidad. La vista Relaciones muestra los campos que componen cada FK; UQ* indica participación en una clave única compuesta. Incluye zoom y desplazamiento, con creación de textos solamente para las entidades visibles.

Detecta archivos .prisma y rutas literales de prisma.config.ts o package.json. Se puede elegir otro archivo o escribir una carpeta para esquemas divididos. No ejecuta la configuración JavaScript, Prisma, consultas a la base ni llamadas de IA: representa el esquema declarado, con validación local de las estructuras compatibles. Los errores y las relaciones ambiguas se muestran en el panel. Las lecturas se ejecutan en segundo plano y están limitadas a 1 MiB, 200 entidades y 1000 relaciones. El esquema seleccionado se recuerda durante la sesión y se actualiza al terminar una ejecución del agente si el panel está abierto.
