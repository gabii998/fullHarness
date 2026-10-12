# Cuota de Codex

[← Volver al índice](../README.md)

La barra lateral consulta la cuota de cuenta mediante `GET https://chatgpt.com/backend-api/wham/usage`, como [Hermes](https://github.com/NousResearch/hermes-agent/blob/main/agent/account_usage.py).
Muestra porcentajes restantes e intervalos de reinicio únicamente cuando el servidor
los informa. Identifica las ventanas por su duración; no supone que la principal
siempre sea la de cinco horas. La cuota de cuenta no equivale al límite particular
asignado a esta aplicación. Las fechas relativas de reinicio corresponden al momento
de la consulta. No guarda estos datos en el contexto de los agentes.

Consulta al abrir, tras autorizar y al finalizar trabajos (máximo una consulta automática
por minuto), además del botón de actualización. No hay polling en reposo ni inferencia.
Una consulta fallida suspende las actualizaciones automáticas hasta una consulta manual
o una nueva autorización. No borra la sesión ni reintenta generaciones.
El botón «Ver uso en ChatGPT» abre la administración oficial de límites por aplicación.

La sesión SiWC anterior devolvió HTTP 401 en la ruta de cuota. La migración al
acceso único de Codex se inicia con «Volver a autorizar» o
`cargo run -- --connect-codex`; la autorización requiere completar el código en el navegador.
`cargo run -- --check-usage` permite verificar la consulta y solo imprime la cuota
normalizada o un error; nunca credenciales.
