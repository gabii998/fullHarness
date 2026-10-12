# Conectar Codex

[← Volver al índice](../README.md)

1. Abrí «Configurar agentes» y pulsá «Continue with ChatGPT».
2. Ingresá el código mostrado en `https://auth.openai.com/codex/device`. El harness verifica la identidad y guarda su propia sesión; este único acceso sirve para chats, catálogo y cuota.
3. Elegí un modelo del catálogo de tu cuenta, marcá «Habilitado» y guardá.
4. Abrí un chat dentro de un proyecto y enviá el pedido.

Al pie del chat, junto al selector de agente, podés elegir modelo y effort.
Las preferencias se guardan por conversación y se restauran al cambiar de chat.
El catálogo se conserva localmente y se actualiza al abrir Configurar agentes
con la sesión guardada, sin pedir otra autorización.

El selector de effort permite guardar `none`,
`minimal`, `low`, `medium`, `high`, `xhigh` o `max`. Los niveles admitidos
dependen del modelo: si rechaza un valor, la tarea muestra el error sin reintentar
ni cambiar el nivel automáticamente. «Effort: auto» omite `reasoning.effort`.
La preferencia se toma al iniciar la tarea y se conserva en todas sus generaciones.
Un nivel menor suele reducir tiempo y tokens de razonamiento; ver la
[documentación oficial](https://developers.openai.com/api/docs/guides/reasoning).

Cerrar configuración, guardando o cancelando, actualiza las pestañas y los
controles con las conexiones vigentes. El botón × cierra la pestaña y conserva
el chat en la barra lateral, con su historial y sus trabajos activos.
La UI usa transiciones de 100–140 ms en selección, hover y apertura/cierre
de paneles; no mantiene animaciones en bucle cuando está en reposo.

«Actualizar modelos» consulta el catálogo sin generar una respuesta. La presencia de un modelo en el catálogo no garantiza que una generación tenga cuota disponible. No se usan claves API ni se cambia automáticamente a facturación por API. El uso de créditos de Codex se administra en ChatGPT Settings → Usage; el harness no modifica esa preferencia. Antes de cada generación nativa consulta la cuota y detiene el pedido si una ventana de cuenta está agotada. La comprobación no reserva cuota frente a otros clientes.

La conexión nueva sigue el protocolo OAuth por dispositivo de [Codex](https://github.com/openai/codex/blob/main/codex-rs/login/src/device_code_auth.rs), también usado por [Hermes](https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/auth_codex.py). Usa el cliente OAuth público de Codex, verifica firma, emisor, destinatario y vigencia del ID token y comprueba acceso a la cuota antes de guardar. Los pedidos de inferencia y catálogo van a `https://chatgpt.com/backend-api/codex`; la cuota va a `/backend-api/wham/usage` con la misma credencial. La renovación es serializada y la persistencia atómica. El harness se identifica con su propio User-Agent y ejecuta sus herramientas sin lanzar el motor o CLI de Codex.

Las sesiones SiWC anteriores se conservan y siguen usando su ruta original hasta que el usuario pulse «Volver a autorizar». Esa autorización reemplaza la sesión solo después de validar la misma identidad y el acceso; cancelarla o fallar no elimina el permiso anterior. Se requiere esta migración una vez para habilitar la cuota con un único acceso.

Al migrar entre clientes, los sujetos OAuth pueden diferir: la migración comprueba
el correo de la identidad firmada contra la cuenta guardada. Luego, las nuevas
autorizaciones de Codex exigen coincidencia exacta de sujeto. La sesión anterior
no se reemplaza cuando la cuenta no coincide.

El catálogo nativo requiere `client_version`, una versión de compatibilidad de
Codex que no corresponde a la versión del paquete del harness. Se consulta con
el sentinel `99.0.0` utilizado por Hermes y, ante un catálogo vacío, con `0.0.0`.
No se agregan modelos inventados al catálogo. El User-Agent sigue identificando
a Ascurra Harness.

Las credenciales del harness se cifran con Windows DPAPI para el usuario actual. No se leen ni modifican credenciales de otros clientes. El MVP admite una cuenta por directorio de datos y una instancia que sea dueña de esa sesión. «Desconectar» borra las credenciales locales e intenta revocar la sesión remota; si no se puede confirmar la revocación, la UI indica cómo hacerlo desde ChatGPT. El almacenamiento seguro en otros sistemas operativos está pendiente.
