# tl360-backend (Rust + Axum + PostgreSQL)

Backend real que pide la SRS (sección 5, Restricciones). Reemplaza al puente en Express
(`TpFinal-backend-main`) manteniendo el contrato de API que consume `tl360-frontend` (rutas bajo
`/api`, mismos nombres de campo), y suma lo que el front necesitaba y no tenía.

## Qué hay

- **API** (`src/handlers`): auth (registro, login, recuperación de contraseña), usuarios, rubros,
  comercios (búsqueda paginada, teléfonos, horarios editables, calificaciones, etiquetas RF-18),
  verificación comercial con CUIT (RF-15), turnos, favoritos y publicidades.
- **Esquema** (`migrations/`): 19 tablas, con las reglas de negocio impuestas en la base (turno
  único por horario → RF-03, puntaje 1-5, roles, horarios coherentes, rubro principal siempre
  presente en la relación N:M). Incluye las tablas de RF sin endpoint propio (tokens de verificación
  de email y reportes) y el N:M de etiquetas con catálogo canónico (RF-18).
- **Datos** (`db/seed/`, se cargan aparte de las migraciones):
  - `01_rubros.sql` — 435 rubros en 10 súper rubros (nombres tomados de una guía local como referencia).
  - `02_empresas.sql` — 3885 comercios importados (nombre, dirección, teléfonos, descripción, fotos).
    **Uso académico: no está versionado ni debe publicarse.** Se regenera con `tools/`.
  - `03_simulados.sql` — datos **inventados** para que toda la web tenga contenido: usuarios de prueba,
    dueños, horarios, calificaciones, etiquetas, publicidades y turnos. Ver el encabezado del archivo.

Usuarios de prueba (contraseña `Demo1234!`): `admin@demo.com` (superadmin), `negocio@demo.com`
(dueño de un comercio con turnos), `cliente@demo.com` (con turnos reservados).

## Cómo correrlo

Requiere Rust (toolchain MSVC en Windows, con Visual Studio Build Tools) y Node 20+.

**Opción A — base local embebida (sin Docker ni Supabase):**

```bash
cd tools && npm install
npm run db          # Postgres real en el puerto 5433; carga los seeds la primera vez
# en otra terminal, en tl360-backend/:
cp .env.example .env   # y poner DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/tl360?sslmode=disable
cargo run
cd tools && npm run smoke   # 42 controles de punta a punta contra el servidor
```

**Opción B — Supabase:** poner el connection string en `.env` y `cargo run`. Las migraciones corren
solas al arrancar (`sqlx::migrate!`). Los seeds se aplican una vez con `psql -f db/seed/0X_*.sql`.

**Frontend:** en `tl360-frontend/.env`, `PUBLIC_MOCK_MODE=false` y `PUBLIC_API_URL=http://localhost:3001`.

## Pruebas

```bash
# con la base de desarrollo levantada (npm run db en tools/):
DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5433/tl360?sslmode=disable" cargo test
```

- **47 unitarias** (sin base de datos): reglas de agenda (grilla, cierre, día sin atención, anticipación de
  cancelación), validaciones, JWT (vencido, otro secreto, `alg=none`), límite de intentos, parseo de horarios,
  franjas superpuestas, composición de los correos, cifrado Web Push (RFC 8188/8291), VAPID, allowlist de endpoints push,
  semana/ventana del resumen semanal RF-14, normalización/límites de etiquetas RF-18 y rango de
  anticipación del recordatorio RF-03.
- **64 de integración** (`tests/api.rs`): cada test crea una base PostgreSQL propia con las migraciones aplicadas
  (`#[sqlx::test]`) y llama a la API real: registro y login, verificación de email (RF-01, vencimiento 1 h y
  reenvío que invalida el anterior), bloqueo de turnos sin verificar, verificación comercial con CUIT (RF-15),
  bloqueo por intentos, recuperación de contraseña, permisos por rol, comercios, horarios, reservas (incluidas 8
  solicitudes simultáneas por el mismo horario), cancelación (cliente y dueño, RF-05), calificaciones por turno (RF-13), atenciones de
  mostrador con métricas (RF-04), favoritos, publicidades (incluidas colectas sin comercio y el canal público de
  difusión, RF-06), suscripciones push, recordatorios (incluida la anticipación configurable por cliente, RF-03)
  y resumen semanal (RF-14), orden por cercanía y apertura (RF-07), etiquetas (filtro AND, alias, reemplazo y
  vaciado, RF-18) y restricciones del esquema.
- `tools/smoke.mjs`, `tools/audit.mjs` y `tools/vapid.mjs`: los dos primeros prueban un servidor ya levantado con
  los datos de demo (`npm run smoke`, `node audit.mjs`) sin modificarlos; el último genera el par VAPID para push.

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
- **Cancelación (RF-05):** el cliente cancela con la anticipación del comercio
  (`anticipacion_cancelacion_min`, 2 h por defecto). El dueño del comercio también puede cancelar
  internamente sin ese mínimo (solo turnos futuros); en ambos casos el turno queda `cancelado` (no
  se borra) y el horario se libera. Al cancelar el cliente se avisa al dueño; al cancelar el dueño se
  envía correo + push al cliente. `GET /turnos/mis-turnos` oculta los cancelados salvo
  `?incluir_cancelados=true`.
- **Calificaciones (RF-13):** `POST /empresas/:id/calificaciones` exige `turno_id` del propio cliente, del mismo
  comercio, ya realizado y no cancelado; es una por turno (reenviar la actualiza). `GET /turnos/mis-turnos` incluye
  `calificado` para no volver a ofrecer el mismo turno.
- **Difusión (RF-06):** `POST /publicidades` con `empresa_id: null` crea una colecta/evento de la plataforma y es
  exclusivo del superadmin; con `empresa_id`, el dueño publica en su comercio. `POST /solicitudes-publicidad` es
  público (sin sesión) para pedir difusión; la bandeja `GET/PUT /solicitudes-publicidad` es solo del superadmin y
  evita duplicados pendientes del mismo contacto.
- **Verificación comercial (RF-15):** el dueño solicita con CUIT (11 dígitos) y constancia (URL) → queda `pendiente`;
  solo el superadmin aprueba (sello on) o rechaza (sello off). El toggle clásico `PUT /empresas/:id/verificado`
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
  comercio cancela su turno. Los superadmins al entrar una verificación, un reporte o una solicitud
  de difusión. **RF-03:** cada cliente elige en `/cuenta` la anticipación del recordatorio
  (`GET/PUT /usuarios/yo/recordatorio`, 1–168 h, default 24); un proceso cada 15 min recuerda los turnos
  confirmados dentro de esa ventana una sola vez (`turnos.recordatorio_enviado_en`). **RF-14:** cada lunes desde
  las 09:00 (hora local), `resumen_semanal.rs` envía
  al dueño el resumen de la semana (confirmados, cancelados y próximo turno); `resumenes_semanales` evita repetirlo.
  Sin `VAPID_PRIVATE_KEY` todo esto queda apagado sin afectar la API. Requiere HTTPS (o `localhost`) para que el
  service worker pueda suscribirse.
- **Cercanía (RF-07):** `GET /empresas?lat=&lng=` (ambas juntas y en rango) devuelve primero los comercios
  geolocalizados, luego los abiertos y dentro de cada grupo los más cercanos (haversine). En esa consulta agrega
  `distancia_km` y `abierto_ahora`; usa el día/hora local de Argentina y las franjas de `horarios_empresa`. Sin
  coordenadas mantiene el orden por verificación y turnos.
- **Etiquetas (RF-18):** `GET /empresas?etiquetas=a,b` exige todas (AND) y normaliza alias al catálogo canónico
  (`Rampa de acceso`, `Baño adaptado`, `Apto celíacos`, `Acepta mascotas`, `Wi-Fi gratis`). El listado y la ficha
  devuelven `etiquetas`; el alta/edición las reemplaza en transacción (`[]` vacía, omitirlas conserva), con
  máximo 12 por comercio.
- **Límite de intentos:** 5 fallos de login por (IP, correo) y 30 por IP cada 15 min → 429; recuperar contraseña,
  3 pedidos por correo y 10 por IP por hora; registro, 20 por IP por hora. Es en memoria (una sola instancia).
- **`PUT /empresas/:id` reemplaza** los campos opcionales que no se envían (teléfono, slogan, duración, redes);
  solo `fotos`, `turnos_habilitados`, `descripcion`, `etiquetas` y `admin_id` se conservan si faltan.

## Simulaciones y límites (a propósito)

- **Correo:** real por SMTP (ver variables). Sin `SMTP_HOST` se mantiene el fallback al log
  (`[simulado] correo para ...`) para desarrollo y pruebas.
- **Push:** real con VAPID. Sin `VAPID_PRIVATE_KEY` la API responde 404 en `/push/clave-publica`, el front no
  muestra la tarjeta de activación y no se envían recordatorios ni el resumen semanal. El horario del resumen
  (lunes 09:00 local) es fijo; la anticipación del recordatorio se configura por cliente entre 1 y 168 h
  (24 h por defecto).
- **Verificación de email (RF-01):** implementada de punta a punta (registro sin verificar, enlace de un
  solo uso que vence en 1 h, reenvío ciego que invalida el anterior, reservar/cancelar como cliente
  exigen cuenta verificada).
- **Coordenadas:** la guía de origen no las publica; `lat/lng` están vacías hasta geocodificar
  las direcciones (hace falta para el mapa de RF-02/RF-07).
- **Búsqueda:** `ILIKE` sensible a acentos (`peluqueria` no encuentra `Peluquerías`); en Supabase se
  resuelve con la extensión `unaccent`.

## Herramientas (`tools/`)

`crawl.js` (extracción de la guía de origen), `gen_seed.js` (arma `db/seed/01` y `02`), `devdb.mjs`
(Postgres local), `smoke.mjs` (pruebas de punta a punta), `validate.mjs` (valida
migración y seeds sobre PGlite).
