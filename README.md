# tl360-backend (Rust + Axum + PostgreSQL)

Backend real que pide la SRS (sección 5, Restricciones). Reemplaza al puente en Express
(`TpFinal-backend-main`) manteniendo el contrato de API que consume `tl360-frontend` (rutas bajo
`/api`, mismos nombres de campo), y suma lo que el front necesitaba y no tenía.

## Qué hay

- **API** (`src/handlers`): auth (registro, login, recuperación de contraseña), usuarios, rubros,
  comercios (búsqueda paginada, teléfonos, redes, fotos, horarios editables, calificaciones, etiquetas RF-18),
  verificación comercial con CUIT (RF-15), turnos online y de mostrador, resumen del comercio,
  favoritos, publicidades y difusiones con aprobación (RF-06).
- **Esquema** (`migrations/`, modelo v2 desde la `0015`; la `0016` renombra restricciones e índices y
  deja `lat`/`lng` en `DECIMAL(9,6)`): 19 tablas en singular, textos de largo conocido como `VARCHAR(N)`
  (el mismo máximo que valida la API), claves foráneas en todas las relaciones y las reglas de negocio
  impuestas en la base: turno único por horario (RF-03), un titular por turno según su `origen`
  (`online` o `mostrador`), un solo rubro por comercio (`empresa.rubro_id`), dueño en
  `empresa.usuario_id`, un solo teléfono principal, teléfonos, redes y fotos en tablas propias
  (`telefono_empresa`, `red_social`, `foto_empresa`), estados en tablas propias (`estado_turno`,
  `estado_aprobacion`), puntaje 1-5, roles y horarios coherentes. Un test de integración verifica que no
  quede ninguna tabla, columna, restricción ni índice con nombres del modelo anterior. El modelo completo
  está en `documentos/modelo_datos/TL360 - modelo de datos v2.pdf` (fuera de este repositorio).
- **Datos** (`db/seed/`, se cargan aparte de las migraciones, ver [Base de datos local](#base-de-datos-local-npm-run-db)):
  - `01_rubros.sql` — 438 rubros (nombres tomados de una guía local como referencia).
  - `02_empresas.sql` — 3885 comercios importados (nombre, dirección, teléfonos, descripción, fotos).
    **Uso académico: no está versionado ni debe publicarse.** Se regenera con `tools/`.
  - `02_empresas_demo.sql` — 54 comercios **ficticios** (los del modo mock del front). Si falta
    `02_empresas.sql` (por ejemplo, en un clon limpio del repositorio), `npm run db` y `validate.mjs` usan este.
  - `03_simulados.sql` — datos **inventados** para que toda la web tenga contenido: usuarios de prueba,
    dueños, horarios, calificaciones, etiquetas, publicidades (aprobadas y una pendiente), difusiones y
    turnos online y de mostrador. Ver el encabezado del archivo.

Usuarios de prueba (contraseña `Demo1234!`): `admin@demo.com` (superadmin), `negocio@demo.com`
(dueño de un comercio con turnos), `cliente@demo.com` (con turnos reservados).

## Cómo correrlo

Requiere Rust (toolchain MSVC en Windows, con Visual Studio Build Tools) y Node 20+.

**Opción A — base local embebida (sin Docker ni Supabase):**

```bash
cd tools && npm install
npm run db          # terminal 1: Postgres en el puerto 5433 (ver la sección siguiente)
# terminal 2, en tl360-backend/:
cp .env.example .env   # y poner DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/tl360?sslmode=disable
cargo run              # aplica las migraciones; npm run db carga los seeds en cuanto terminan
# terminal 3, con todo levantado:
cd tools && npm run smoke && node audit.mjs
```

**Opción B — Supabase:** poner el connection string en `.env` y `cargo run`. Las migraciones corren
solas al arrancar (`sqlx::migrate!`). Los seeds se aplican una vez, **después** de las migraciones y en
orden, con `psql -f db/seed/01_rubros.sql`, `02_empresas_demo.sql` (o `02_empresas.sql`) y `03_simulados.sql`.

**Frontend:** en `tl360-frontend/.env`, `PUBLIC_MOCK_MODE=false` y `PUBLIC_API_URL=http://localhost:3001`.

## Base de datos local (`npm run db`)

`npm run db` (en `tools/`, ejecuta `node devdb.mjs`) deja una base PostgreSQL real en
`postgres://postgres:postgres@127.0.0.1:5433/tl360`. Se puede correr las veces que haga falta: cada paso
revisa el estado antes de actuar.

1. **Inicia PostgreSQL** en el puerto 5433 con el paquete `embedded-postgres` (no hace falta instalar
   Postgres). Los datos quedan en `tools/.pgdata` y **se conservan entre ejecuciones**: la primera vez se
   inicializa el clúster; las siguientes se reutiliza. Si ya hay un Postgres escuchando en 5433 (por
   ejemplo, uno levantado con `pg_ctl`), usa ese y no lo detiene al salir.
2. **Crea la base `tl360` si no existe:** se conecta primero a la base administrativa `postgres`,
   consulta `pg_database`, crea `tl360` solo si falta, cierra esa conexión y recién después se conecta a
   `tl360`. No depende de que `.pgdata` exista: una carpeta con el clúster pero sin la base también se
   resuelve.
3. **Espera las migraciones:** el esquema lo crea el backend (`cargo run` aplica `migrations/` con
   `sqlx::migrate!`). El script espera a que estén aplicadas **todas** las migraciones del repositorio
   (las compara con `_sqlx_migrations`) y avisa cada 15 s si falta levantar el backend.
4. **Verifica que todo sea del modelo v2:** que existan las tablas nuevas, que no existan las del modelo
   anterior y que ningún seed use tablas o columnas que ya no existen.
5. **Carga los datos iniciales**, solo si la base está vacía y en una sola transacción (si un seed falla,
   no queda nada a medias y se informa el archivo y el error):
   - `01_rubros.sql`;
   - `02_empresas.sql` si está (comercios importados, no versionado) o, si no, `02_empresas_demo.sql`
     (54 comercios ficticios, el caso de un clon limpio del repositorio). `DEVDB_SEED=demo npm run db`
     fuerza los de demostración;
   - `03_simulados.sql` (usuarios de prueba, horarios, turnos, publicidades, difusiones...).
6. Imprime `SEED-LISTO` y **queda corriendo**: Postgres vive mientras viva el proceso.

**Detener la base:** `Ctrl+C` en la terminal de `npm run db` cierra la conexión y detiene Postgres (los
datos quedan en `.pgdata`). Si se levantó con `pg_ctl`, se detiene con
`node_modules/@embedded-postgres/windows-x64/native/bin/pg_ctl.exe -D .pgdata stop` (desde `tools/`).

**Volver a iniciarla:** `npm run db` otra vez. Encuentra la base y los datos, no recarga los seeds y
espera a que levantes el backend como siempre.

**Empezar de cero:** detener la base, borrar la carpeta `tools/.pgdata` y correr `npm run db` + `cargo run`.
Para probar sobre una base nueva sin tocar `tl360`: `DEVDB_BASE=tl360_prueba npm run db` y levantar el
backend con `DATABASE_URL=.../tl360_prueba` (`DEVDB_PUERTO` cambia el puerto).

**Errores habituales:** todos se informan con `[devdb] ERROR: ...` y el proceso termina con código 1.
En Windows, Postgres no se ejecuta desde una terminal "como administrador": el mensaje indica cómo
levantarlo con `pg_ctl`, que baja los permisos, y después basta con volver a correr `npm run db`.

## Pruebas

```bash
# con la base de desarrollo levantada (npm run db en tools/):
DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5433/tl360?sslmode=disable" cargo test
```

- **50 unitarias** (sin base de datos): reglas de agenda (grilla, cierre, día sin atención, anticipación de
  cancelación), validaciones (incluidos los largos máximos de cada `VARCHAR(N)`), JWT (vencido, otro secreto,
  `alg=none`), límite de intentos, parseo de horarios, franjas superpuestas, composición de los correos, cifrado
  Web Push (RFC 8188/8291), VAPID, allowlist de endpoints push, período del resumen del comercio,
  normalización/límites de etiquetas RF-18 y rango de anticipación del recordatorio RF-03.
- **84 de integración** (`tests/api.rs`): cada test crea una base PostgreSQL propia con las migraciones aplicadas
  (`#[sqlx::test]`) y llama a la API real: registro y login, verificación de email (RF-01, vencimiento 1 h y
  reenvío que invalida el anterior), Mis datos (perfil y contraseña), bloqueo de turnos sin verificar,
  verificación comercial con CUIT y rechazo con motivo (RF-15), turnos de mostrador con nota (RF-04),
  bloqueo por intentos, recuperación de contraseña, permisos por rol (matriz de 19 acciones con cliente, dueño
  ajeno y sin sesión), comercios (teléfonos, redes y fotos en sus tablas, largos máximos), horarios, reservas (incluidas 8 solicitudes simultáneas por el mismo horario),
  cancelación (cliente y dueño, RF-05), calificaciones por turno (RF-13), favoritos, publicidades con aprobación y
  difusiones por el canal público (RF-06), resumen del comercio por período, suscripciones push, recordatorios
  (incluida la anticipación configurable por cliente, RF-03), orden por cercanía y apertura (RF-07), etiquetas
  (filtro AND, alias, reemplazo y vaciado, RF-18), restricciones del esquema y ausencia de nombres del modelo anterior.
- `tools/smoke.mjs`, `tools/audit.mjs` y `tools/vapid.mjs`: los dos primeros prueban un servidor ya levantado
  (`npm run smoke`, `node audit.mjs`; `API=http://...` cambia el destino) sin dejar los datos modificados. No suponen
  un tamaño de catálogo: eligen el comercio de `negocio@demo.com`, un rubro con comercios y un día y hora en que
  el comercio atiende, así que funcionan igual con los 54 comercios de demostración que con los importados. El
  último genera el par VAPID para push.

## Variables de entorno

| Variable | Default | Uso |
|---|---|---|
| `DATABASE_URL` | — (obligatoria) | conexión a Postgres |
| `JWT_SECRET` | `supersecreto` | **cambiar en producción** |
| `PORT` | `3001` | puerto HTTP |
| `FRONTEND_URL` | `http://localhost:5173` | base de los enlaces de los correos (verificación, recuperación y turnos) |
| `DB_MAX_CONNECTIONS` | `10` | tamaño del pool |
| `APP_ENV` | — | con `production` el servidor no arranca si `JWT_SECRET` tiene menos de 32 caracteres |
| `CORS_ORIGINS` | `FRONTEND_URL` | orígenes permitidos para llamar a la API desde un navegador (separados por coma) |
| `TRUST_PROXY` | `false` | `true` detrás de Render/proxy: toma la IP real de `X-Forwarded-For` para los límites de intentos |
| `SMTP_HOST` | - | servidor SMTP; sin esta variable los correos se escriben en el log |
| `SMTP_PORT` | `587` | puerto SMTP (587 STARTTLS, 465 wrapper) |
| `SMTP_TLS` | `starttls` | `starttls`, `wrapper` (465) o `none` (solo local) |
| `SMTP_USERNAME` / `SMTP_PASSWORD` | - | credenciales SMTP; si faltan se conecta sin autenticación |
| `SMTP_FROM` | `TL360 <no-reply@tl360.local>` | remitente (`Nombre <correo>`) |
| `VAPID_PRIVATE_KEY` | - | clave privada P-256 en base64url (32 bytes); sin ella Web Push queda apagado. Generar con `node tools/vapid.mjs` |
| `VAPID_SUBJECT` | `mailto:no-reply@tl360.local` | contacto VAPID (`mailto:` o `https://`) |

## Reglas de negocio

- **Horarios (RF-04):** `PUT /empresas/:id/horarios` rechaza con 400 dos franjas del mismo día que se superponen
  (contiguas, como 09:00-13:00 y 13:00-17:00, sí se aceptan).
- **Reservas (RF-03/04):** el día debe tener horario cargado, la hora debe caer dentro de una franja, entrar completa
  antes del cierre y estar en la grilla (inicio de franja + múltiplos de `duracion_turno_min`, 30 min si no está
  definida). El front usa la misma grilla. Dos pedidos simultáneos por el mismo horario: uno confirma y el otro 409.
- **Turnos de mostrador (RF-04):** `POST /empresas/:id/turnos` (dueño o superadmin) agenda un turno con
  `origen = 'mostrador'` para alguien sin cuenta, con `cliente_nombre`, `cliente_email` y `nota` opcionales; usa la
  misma grilla y el mismo índice único que la reserva online (`origen = 'online'`). `GET /empresas/:id/turnos`
  devuelve el correo de contacto, el `origen`, la `nota` y `sin_cuenta`.
- **Resumen del comercio:** `GET /empresas/:id/resumen?desde=&hasta=` (dueño o superadmin; sin fechas, los últimos
  30 días; hasta un año) calcula a pedido los turnos online y de mostrador, cancelados, atendidos, calificaciones y
  favoritos nuevos del período. No se guarda: el panel lo descarga en PDF.
- **Mis datos:** `PUT /usuarios/yo` cambia nombre y correo (un correo nuevo vuelve a exigir verificación y se
  envía el enlace) o la contraseña (`password_actual` + `password_nueva`). Lo usan clientes y dueños.
- **Favoritos (RF-10):** solo las cuentas de cliente marcan favoritos (`PUT /favoritos/:id` responde 403 al dueño
  o al superadmin).
- **Cancelación (RF-05):** el cliente cancela con la anticipación del comercio
  (`anticipacion_cancelacion_min`, 2 h por defecto). El dueño del comercio también puede cancelar
  internamente sin ese mínimo (solo turnos futuros); en ambos casos el turno queda `cancelado` (no
  se borra) y el horario se libera. Al cancelar el cliente se avisa al dueño; al cancelar el dueño se
  envía correo + push al cliente. `GET /turnos/mis-turnos` oculta los cancelados salvo
  `?incluir_cancelados=true`.
- **Calificaciones (RF-13):** `POST /empresas/:id/calificaciones` exige `turno_id` del propio cliente, del mismo
  comercio, ya realizado y no cancelado; es una por turno (reenviar la actualiza). `GET /turnos/mis-turnos` incluye
  `calificado` para no volver a ofrecer el mismo turno.
- **Publicidades (RF-06):** siempre son de un comercio. `POST /publicidades` del dueño queda `pendiente`; la del
  superadmin, `aprobada`. `GET /publicidades` devuelve solo las aprobadas y vigentes (`fecha_inicio`/`fecha_fin`);
  con `?estado=pendiente|…|todas` es la bandeja del superadmin (o, con `empresa_id`, la lista del dueño).
  `PUT /publicidades/:id/estado` aprueba, rechaza o finaliza (el dueño solo puede darla de baja o reenviarla
  después de un rechazo).
- **Difusiones (RF-06):** colectas y eventos sin comercio. `POST /difusiones` es público (sin sesión, con límite
  por IP) y queda `pendiente`, sin dos pendientes del mismo contacto; la que carga el superadmin nace aprobada.
  `GET /difusiones` devuelve las aprobadas y vigentes; `?estado=…` y `PUT /difusiones/:id/estado` son del
  superadmin.
- **Verificación comercial (RF-15):** el dueño solicita con CUIT (11 dígitos) y constancia (URL) → queda `pendiente`;
  solo el superadmin aprueba (sello on) o rechaza (sello off) con un `motivo` obligatorio (5 a 500 caracteres) que se
  guarda en `motivo_rechazo` y le llega al dueño por correo. El toggle clásico `PUT /empresas/:id/verificado`
  sincroniza `verificado_estado`. Una nueva solicitud sobre un comercio ya verificado lo vuelve a `pendiente` y apaga
  el sello hasta que se apruebe (la base exige `verificado = (verificado_estado = 'verificado')`). La bandeja se
  consulta con `GET /empresas?verificado_estado=pendiente`.
- **Correo (RF-01/03/05/15):** verificación y recuperación vencen en **1 h**; reenviar o pedir de nuevo invalida el
  enlace anterior. Reservar envía confirmación y cancelar avisa al cliente; si cancela el dueño, el
  correo lo identifica como tal. Aprobar/rechazar una verificación avisa al dueño. Un
  fallo de SMTP se registra pero no revierte la operación.
- **Notificaciones push (RNF-04/RF-03/14/16):** el navegador se suscribe desde `/cuenta` con la clave pública VAPID
  (`GET /push/clave-publica`) y el backend guarda endpoint + claves por usuario (`POST/DELETE /push/suscripciones`).
  El dueño recibe push al reservar o cuando el cliente cancela; el cliente recibe push si el
  comercio cancela su turno. Los superadmins al entrar una verificación, un reporte, una publicidad para
  aprobar o un pedido de difusión. **RF-03:** cada cliente elige en `/cuenta` la anticipación del recordatorio
  (`GET/PUT /usuarios/yo/recordatorio`, 1–168 h, default 24); un proceso cada 15 min recuerda los turnos
  confirmados dentro de esa ventana una sola vez (`turno.recordatorio_enviado_en`). Sin `VAPID_PRIVATE_KEY` todo
  esto queda apagado sin afectar la API. Requiere HTTPS (o `localhost`) para que el service worker pueda
  suscribirse.
- **Cercanía (RF-07):** `GET /empresas?lat=&lng=` (ambas juntas y en rango) devuelve primero los comercios
  geolocalizados, luego los abiertos y dentro de cada grupo los más cercanos (haversine). En esa consulta agrega
  `distancia_km` y `abierto_ahora`; usa el día/hora local de Argentina y las franjas de `horario_empresa`. Sin
  coordenadas mantiene el orden por verificación y turnos.
- **Etiquetas (RF-18):** `GET /empresas?etiquetas=a,b` exige todas (AND) y normaliza alias al catálogo canónico
  (`Rampa de acceso`, `Baño adaptado`, `Apto celíacos`, `Acepta mascotas`, `Wi-Fi gratis`). El listado y la ficha
  devuelven `etiquetas`; el alta/edición las reemplaza en transacción (`[]` vacía, omitirlas conserva), con
  máximo 12 por comercio.
- **Límite de intentos:** 5 fallos de login por (IP, correo) y 30 por IP cada 15 min → 429; recuperar contraseña,
  3 pedidos por correo y 10 por IP por hora; registro, 20 por IP por hora. Es en memoria (una sola instancia).
- **Teléfonos, redes y fotos (modelo v2):** el comercio se devuelve y se recibe con las listas de sus tablas:
  `telefonos: [{ numero, descripcion, canal, es_whatsapp, principal }]` (`telefono_empresa`, el principal
  primero), `redes_sociales: [{ tipo, url }]` (`red_social`, tipos `instagram`, `facebook`, `tiktok`, `x`,
  `youtube`, `web`, una por tipo) y `fotos: [url]` (`foto_empresa`, en orden). En `POST/PUT /empresas` cada lista
  reemplaza la anterior y, si no se envía, se conserva. Si ningún teléfono viene con `principal: true`, el
  principal es el primero; `PUT /empresas/:id/telefonos` reemplaza solo la lista de teléfonos. Un número
  inválido o repetido, dos principales, una red desconocida o repetida o una URL que no sea http(s) responden 400.
- **`PUT /empresas/:id`** reemplaza `slogan` y `duracion_turno_min` si no se envían; `telefonos`,
  `redes_sociales`, `fotos`, `turnos_habilitados`, `descripcion`, `etiquetas` y `usuario_id` se conservan si faltan.
- **Permisos:** un pedido sin sesión responde 401; con sesión pero sin permiso, 403 con el mensaje
  `No tenés los permisos para realizar esta acción` (y el motivo cuando es una regla de rol, por ejemplo
  `...: las reservas online se hacen con una cuenta de cliente`). El rol se lee de la base en cada pedido.
- **Validaciones:** correo con formato completo (parte local de hasta 64 caracteres, sin puntos al borde ni
  seguidos; dominio con etiquetas válidas y terminación de 2+ letras), teléfono con 6 a 15 dígitos y solo
  separadores habituales, contraseña de 8+ caracteres con mayúscula, minúscula y número, y los largos de cada
  `VARCHAR(N)`. Un correo ya registrado responde 409. El front aplica las mismas reglas.
- **Rubros:** `GET/POST /rubros`, `PUT/DELETE /rubros/:id`; cada comercio tiene un solo `rubro_id`
  (`GET /empresas?rubro_id=` y `GET /empresas/rubro/:id`).

## Simulaciones y límites (a propósito)

- **Correo:** real por SMTP (ver variables). Sin `SMTP_HOST` se mantiene el fallback al log
  (`[simulado] correo para ...`) para desarrollo y pruebas.
- **Push:** real con VAPID. Sin `VAPID_PRIVATE_KEY` la API responde 404 en `/push/clave-publica`, el front no
  muestra la tarjeta de activación y no se envían recordatorios. La anticipación del recordatorio se configura
  por cliente entre 1 y 168 h (24 h por defecto).
- **Verificación de email (RF-01):** implementada de punta a punta (registro sin verificar, enlace de un
  solo uso que vence en 1 h, reenvío ciego que invalida el anterior, reservar/cancelar como cliente
  exigen cuenta verificada).
- **Coordenadas:** la guía de origen no las publica; `lat/lng` están vacías hasta geocodificar
  las direcciones (hace falta para el mapa de RF-02/RF-07).
- **Búsqueda:** `ILIKE` sensible a acentos (`peluqueria` no encuentra `Peluquerías`); en Supabase se
  resuelve con la extensión `unaccent`.

## Herramientas (`tools/`)

`crawl.js` (extracción de la guía de origen), `gen_seed.js` (arma `db/seed/01` y `02`), `devdb.mjs`
(Postgres local, ver arriba), `smoke.mjs` (pruebas de punta a punta), `audit.mjs` (seguridad y reglas de
negocio), `validate.mjs` (valida migración y seeds sobre PGlite).
