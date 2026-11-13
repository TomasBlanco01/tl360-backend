-- Modelo de datos v2: correcciones de la revisión del modelo.
-- * Tablas en singular y textos de largo conocido como VARCHAR(N)/CHAR(N), con el mismo máximo que
--   valida la API.
-- * Un comercio tiene un solo rubro (categorias pasa a rubro; se eliminan super_rubros y
--   empresa_categorias).
-- * Los turnos de mostrador viven en turno (origen 'online' | 'mostrador'); se elimina
--   atenciones_mostrador. El estado del turno pasa a la tabla estado_turno.
-- * Teléfonos con descripción, canal y principal; redes sociales y fotos en tablas propias.
-- * publicaciones_destacadas pasa a publicidad, con aprobación (estado_aprobacion); las colectas y
--   los pedidos públicos pasan a difusion. Se elimina solicitudes_publicidad.
-- * El resumen del comercio se calcula a pedido: se elimina resumenes_semanales.
-- Los datos existentes se trasladan antes de borrar cada tabla o columna.

-- ================================================================ nombres en singular
ALTER TABLE usuarios RENAME TO usuario;
ALTER TABLE tokens_usuario RENAME TO token_usuario;
ALTER TABLE push_suscripciones RENAME TO push_suscripcion;
ALTER TABLE categorias RENAME TO rubro;
ALTER TABLE empresas RENAME TO empresa;
ALTER TABLE telefonos_empresa RENAME TO telefono_empresa;
ALTER TABLE horarios_empresa RENAME TO horario_empresa;
ALTER TABLE etiquetas RENAME TO etiqueta;
ALTER TABLE empresa_etiquetas RENAME TO empresa_etiqueta;
ALTER TABLE turnos RENAME TO turno;
ALTER TABLE favoritos RENAME TO favorito;
ALTER TABLE calificaciones RENAME TO calificacion;
ALTER TABLE reportes RENAME TO reporte;
ALTER TABLE publicaciones_destacadas RENAME TO publicidad;

ALTER SEQUENCE usuarios_id_seq RENAME TO usuario_id_seq;
ALTER SEQUENCE tokens_usuario_id_seq RENAME TO token_usuario_id_seq;
ALTER SEQUENCE push_suscripciones_id_seq RENAME TO push_suscripcion_id_seq;
ALTER SEQUENCE categorias_id_seq RENAME TO rubro_id_seq;
ALTER SEQUENCE empresas_id_seq RENAME TO empresa_id_seq;
ALTER SEQUENCE telefonos_empresa_id_seq RENAME TO telefono_empresa_id_seq;
ALTER SEQUENCE horarios_empresa_id_seq RENAME TO horario_empresa_id_seq;
ALTER SEQUENCE etiquetas_id_seq RENAME TO etiqueta_id_seq;
ALTER SEQUENCE turnos_id_seq RENAME TO turno_id_seq;
ALTER SEQUENCE calificaciones_id_seq RENAME TO calificacion_id_seq;
ALTER SEQUENCE reportes_id_seq RENAME TO reporte_id_seq;
ALTER SEQUENCE publicaciones_destacadas_id_seq RENAME TO publicidad_id_seq;

-- ================================================================ usuario
ALTER TABLE usuario RENAME COLUMN "contraseña" TO contrasena_hash;
ALTER TABLE usuario RENAME COLUMN recordatorio_horas_anticipacion TO recordatorio_horas;
ALTER TABLE usuario
    ALTER COLUMN nombre TYPE varchar(100),
    ALTER COLUMN email TYPE varchar(254),
    ALTER COLUMN contrasena_hash TYPE char(60),
    ALTER COLUMN rol TYPE varchar(10);

ALTER TABLE token_usuario
    ALTER COLUMN tipo TYPE varchar(20),
    ALTER COLUMN token_hash TYPE char(64);

ALTER TABLE push_suscripcion
    ALTER COLUMN endpoint TYPE varchar(2048),
    ALTER COLUMN p256dh TYPE varchar(100),
    ALTER COLUMN auth TYPE varchar(30);

-- ================================================================ rubro (RF-02): uno por comercio
DROP TRIGGER trg_empresas_sync_categoria ON empresa;
DROP FUNCTION sync_empresa_categoria();
DROP TABLE empresa_categorias;
ALTER TABLE rubro DROP COLUMN super_rubro_id;
DROP TABLE super_rubros;
ALTER TABLE rubro
    ALTER COLUMN nombre TYPE varchar(80),
    ALTER COLUMN slug TYPE varchar(100);

-- ================================================================ empresa
ALTER TABLE empresa RENAME COLUMN categoria_id TO rubro_id;
ALTER TABLE empresa RENAME COLUMN admin_id TO usuario_id;
ALTER TABLE empresa RENAME COLUMN constancia_cuit_url TO url_cuit;
ALTER TABLE empresa RENAME COLUMN imagen_perfil TO url_imagen_perfil;
ALTER TABLE empresa RENAME COLUMN imagen_portada TO url_imagen_portada;
ALTER INDEX idx_empresas_categoria RENAME TO idx_empresa_rubro;
ALTER INDEX idx_empresas_admin RENAME TO idx_empresa_usuario;

-- La importación trajo algunas direcciones mal cortadas (con el texto de la ficha pegado).
UPDATE empresa SET direccion = left(direccion, 150) WHERE char_length(direccion) > 150;
UPDATE empresa SET descripcion = left(descripcion, 2000) WHERE char_length(descripcion) > 2000;
UPDATE empresa SET slogan = left(slogan, 150) WHERE char_length(slogan) > 150;

-- ---------------------------------------------------------------- teléfonos: descripción, canal y principal
ALTER TABLE telefono_empresa RENAME COLUMN etiqueta TO descripcion;
ALTER TABLE telefono_empresa RENAME COLUMN modo_contacto TO canal;
ALTER TABLE telefono_empresa RENAME CONSTRAINT telefonos_empresa_uq TO telefono_empresa_uq;
ALTER TABLE telefono_empresa ADD COLUMN principal boolean NOT NULL DEFAULT false;
-- El teléfono de la ficha (empresa.telefono) pasa a ser el principal.
INSERT INTO telefono_empresa (empresa_id, numero)
SELECT id, left(btrim(telefono), 20) FROM empresa
WHERE telefono IS NOT NULL AND btrim(telefono) <> ''
ON CONFLICT (empresa_id, numero) DO NOTHING;
UPDATE telefono_empresa t SET principal = true
FROM empresa e WHERE e.id = t.empresa_id AND t.numero = left(btrim(e.telefono), 20);
ALTER TABLE empresa DROP COLUMN telefono;
UPDATE telefono_empresa SET numero = left(numero, 20) WHERE char_length(numero) > 20;
UPDATE telefono_empresa SET descripcion = left(descripcion, 50) WHERE char_length(descripcion) > 50;
ALTER TABLE telefono_empresa
    ALTER COLUMN numero TYPE varchar(20),
    ALTER COLUMN descripcion TYPE varchar(50),
    ALTER COLUMN canal TYPE varchar(10);
CREATE UNIQUE INDEX telefono_empresa_principal_uq ON telefono_empresa (empresa_id) WHERE principal;

-- ---------------------------------------------------------------- redes sociales
CREATE TABLE red_social (
    id          serial PRIMARY KEY,
    empresa_id  integer NOT NULL REFERENCES empresa(id) ON DELETE CASCADE,
    tipo        varchar(20) NOT NULL CHECK (tipo IN ('instagram', 'facebook', 'tiktok', 'x', 'youtube', 'web')),
    url         varchar(2048) NOT NULL,
    CONSTRAINT red_social_tipo_uq UNIQUE (empresa_id, tipo)
);
INSERT INTO red_social (empresa_id, tipo, url)
SELECT id, 'instagram', left(btrim(instagram), 2048) FROM empresa WHERE btrim(coalesce(instagram, '')) <> '';
INSERT INTO red_social (empresa_id, tipo, url)
SELECT id, 'facebook', left(btrim(facebook), 2048) FROM empresa WHERE btrim(coalesce(facebook, '')) <> '';
ALTER TABLE empresa DROP COLUMN instagram, DROP COLUMN facebook;

-- ---------------------------------------------------------------- fotos
CREATE TABLE foto_empresa (
    id          serial PRIMARY KEY,
    empresa_id  integer NOT NULL REFERENCES empresa(id) ON DELETE CASCADE,
    url         varchar(2048) NOT NULL,
    orden       smallint NOT NULL DEFAULT 0 CHECK (orden >= 0)
);
CREATE INDEX idx_foto_empresa ON foto_empresa (empresa_id, orden);
INSERT INTO foto_empresa (empresa_id, url, orden)
SELECT e.id, left(f.url, 2048), (f.n - 1)::smallint
FROM empresa e, unnest(e.fotos) WITH ORDINALITY AS f(url, n)
WHERE btrim(f.url) <> '';
ALTER TABLE empresa DROP COLUMN fotos;

-- ---------------------------------------------------------------- columnas que ya no se usan
ALTER TABLE empresa DROP COLUMN origen_url, DROP COLUMN nivel_origen;

ALTER TABLE empresa
    ALTER COLUMN nombre TYPE varchar(100),
    ALTER COLUMN slug TYPE varchar(120),
    ALTER COLUMN direccion TYPE varchar(150),
    ALTER COLUMN localidad TYPE varchar(60),
    ALTER COLUMN codigo_postal TYPE varchar(8),
    ALTER COLUMN slogan TYPE varchar(150),
    ALTER COLUMN descripcion TYPE varchar(2000),
    ALTER COLUMN url_imagen_perfil TYPE varchar(2048),
    ALTER COLUMN url_imagen_portada TYPE varchar(2048),
    ALTER COLUMN cuit TYPE char(11),
    ALTER COLUMN url_cuit TYPE varchar(2048),
    ALTER COLUMN verificado_estado TYPE varchar(15),
    ALTER COLUMN modo_contacto TYPE varchar(10),
    ALTER COLUMN motivo_rechazo TYPE varchar(500),
    ALTER COLUMN origen TYPE varchar(15),
    ALTER COLUMN origen_id TYPE varchar(40);
ALTER TABLE empresa ALTER COLUMN duracion_turno_min TYPE smallint;

ALTER TABLE horario_empresa ALTER COLUMN dia_semana TYPE varchar(9);
ALTER TABLE etiqueta ALTER COLUMN nombre TYPE varchar(40);

-- ================================================================ turno (RF-03, RF-04)
CREATE TABLE estado_turno (
    id      smallint PRIMARY KEY,
    nombre  varchar(20) NOT NULL UNIQUE
);
-- Los id son fijos: el índice único del horario se define sobre el id de 'cancelado'.
INSERT INTO estado_turno (id, nombre) VALUES
    (1, 'confirmado'), (2, 'cancelado'), (3, 'completado'), (4, 'no_asistio');

ALTER TABLE turno ADD COLUMN estado_turno_id smallint NOT NULL DEFAULT 1 REFERENCES estado_turno(id);
UPDATE turno SET estado_turno_id = CASE estado
    WHEN 'confirmado' THEN 1 WHEN 'cancelado' THEN 2 WHEN 'completado' THEN 3 ELSE 4 END;
DROP INDEX turnos_slot_activo_uq;
ALTER TABLE turno DROP COLUMN estado;
-- Un solo turno activo (no cancelado) por comercio, fecha y hora, sea online o de mostrador.
CREATE UNIQUE INDEX turno_slot_activo_uq ON turno (empresa_id, fecha, hora) WHERE estado_turno_id <> 2;
CREATE INDEX idx_turno_estado ON turno (estado_turno_id);
ALTER INDEX idx_turnos_usuario RENAME TO idx_turno_usuario;
ALTER INDEX idx_turnos_empresa_fecha RENAME TO idx_turno_empresa_fecha;

-- Origen: 'online' (reserva del cliente en la app) o 'mostrador' (lo carga el comercio, sin cuenta).
ALTER TABLE turno ADD COLUMN origen varchar(10) NOT NULL DEFAULT 'online';
UPDATE turno SET origen = 'mostrador' WHERE usuario_id IS NULL;
ALTER TABLE turno ADD COLUMN nota varchar(300);
ALTER TABLE turno DROP CONSTRAINT turnos_cliente_ck;
ALTER TABLE turno RENAME CONSTRAINT turnos_cliente_email_ck TO turno_cliente_email_ck;
ALTER TABLE turno
    ALTER COLUMN cliente_nombre TYPE varchar(100),
    ALTER COLUMN cliente_email TYPE varchar(254);
ALTER TABLE turno ADD CONSTRAINT turno_origen_ck CHECK (
    (origen = 'online' AND usuario_id IS NOT NULL AND cliente_nombre IS NULL AND cliente_email IS NULL)
    OR (origen = 'mostrador' AND usuario_id IS NULL AND cliente_nombre IS NOT NULL
        AND char_length(btrim(cliente_nombre)) BETWEEN 1 AND 100));

-- Las atenciones de mostrador eran conteos diarios sin hora: no tienen un turno equivalente.
-- Desde ahora cada atención de mostrador se registra como un turno con origen 'mostrador'.
DROP TABLE atenciones_mostrador;

-- ================================================================ calificación y reporte
UPDATE calificacion SET comentario = left(comentario, 1000) WHERE char_length(comentario) > 1000;
ALTER TABLE calificacion ALTER COLUMN comentario TYPE varchar(1000);

ALTER TABLE reporte RENAME COLUMN motivo TO descripcion;
UPDATE reporte SET descripcion = left(descripcion, 500) WHERE char_length(descripcion) > 500;
ALTER TABLE reporte ALTER COLUMN descripcion TYPE varchar(500);
ALTER INDEX reportes_pendiente_uq RENAME TO reporte_pendiente_uq;
ALTER INDEX idx_reportes_pendientes RENAME TO idx_reporte_pendiente;

-- ================================================================ aprobación: publicidad y difusión (RF-06)
CREATE TABLE estado_aprobacion (
    id      smallint PRIMARY KEY,
    nombre  varchar(20) NOT NULL UNIQUE
);
INSERT INTO estado_aprobacion (id, nombre) VALUES
    (1, 'pendiente'), (2, 'aprobada'), (3, 'rechazada'), (4, 'finalizada');

CREATE TABLE difusion (
    id                    serial PRIMARY KEY,
    usuario_id            integer REFERENCES usuario(id) ON DELETE SET NULL,
    estado_aprobacion_id  smallint NOT NULL DEFAULT 1 REFERENCES estado_aprobacion(id),
    tipo                  varchar(10) NOT NULL CHECK (tipo IN ('colecta', 'evento')),
    titulo                varchar(100) NOT NULL,
    descripcion           varchar(1000) NOT NULL,
    url_imagen            varchar(2048),
    solicitante_nombre    varchar(100) NOT NULL,
    solicitante_contacto  varchar(150) NOT NULL,
    fecha_inicio          date,
    fecha_fin             date,
    creado_en             timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT difusion_fechas_ck CHECK (fecha_fin IS NULL OR fecha_inicio IS NULL OR fecha_fin >= fecha_inicio)
);
CREATE INDEX idx_difusion_estado ON difusion (estado_aprobacion_id, creado_en);
-- Un mismo contacto no deja dos pedidos pendientes (sin distinguir mayúsculas).
CREATE UNIQUE INDEX difusion_pendiente_uq ON difusion (lower(solicitante_contacto)) WHERE estado_aprobacion_id = 1;

-- Las colectas de la plataforma (publicaciones sin comercio) pasan a ser difusiones aprobadas.
INSERT INTO difusion (estado_aprobacion_id, tipo, titulo, descripcion, url_imagen,
                      solicitante_nombre, solicitante_contacto, creado_en)
SELECT CASE WHEN activa THEN 2 ELSE 4 END, 'colecta', left(titulo, 100),
       left(coalesce(nullif(btrim(descripcion), ''), titulo), 1000), imagen_url,
       'TL360', 'plataforma', creado_en
FROM publicidad WHERE empresa_id IS NULL;
DELETE FROM publicidad WHERE empresa_id IS NULL;

-- Los pedidos públicos de difusión pasan a difusion, con su estado de gestión.
INSERT INTO difusion (estado_aprobacion_id, tipo, titulo, descripcion, solicitante_nombre,
                      solicitante_contacto, creado_en)
SELECT CASE estado WHEN 'pendiente' THEN 1 WHEN 'atendida' THEN 2 ELSE 3 END,
       CASE tipo WHEN 'colecta' THEN 'colecta' ELSE 'evento' END,
       left('Pedido de ' || nombre, 100), left(mensaje, 1000), left(nombre, 100), left(contacto, 150), creado_en
FROM solicitudes_publicidad
ORDER BY creado_en
ON CONFLICT DO NOTHING;
DROP TABLE solicitudes_publicidad;

-- Publicidad: siempre de un comercio, con imagen y descripción, y pendiente hasta que se apruebe.
DELETE FROM publicidad WHERE imagen_url IS NULL OR btrim(imagen_url) = '';
ALTER TABLE publicidad RENAME COLUMN imagen_url TO url_imagen;
UPDATE publicidad SET descripcion = titulo WHERE descripcion IS NULL OR btrim(descripcion) = '';
ALTER TABLE publicidad ADD COLUMN estado_aprobacion_id smallint NOT NULL DEFAULT 1 REFERENCES estado_aprobacion(id);
UPDATE publicidad SET estado_aprobacion_id = CASE WHEN activa THEN 2 ELSE 4 END;
ALTER TABLE publicidad DROP COLUMN activa;
ALTER TABLE publicidad ADD COLUMN fecha_inicio date, ADD COLUMN fecha_fin date;
ALTER TABLE publicidad ADD CONSTRAINT publicidad_fechas_ck
    CHECK (fecha_fin IS NULL OR fecha_inicio IS NULL OR fecha_fin >= fecha_inicio);
ALTER TABLE publicidad
    ALTER COLUMN empresa_id SET NOT NULL,
    ALTER COLUMN titulo TYPE varchar(100),
    ALTER COLUMN descripcion TYPE varchar(500),
    ALTER COLUMN descripcion SET NOT NULL,
    ALTER COLUMN url_imagen TYPE varchar(2048),
    ALTER COLUMN url_imagen SET NOT NULL;
CREATE INDEX idx_publicidad_estado ON publicidad (estado_aprobacion_id);

-- ================================================================ resumen del comercio
-- Se calcula a pedido para el período que elija el dueño (GET /empresas/:id/resumen).
DROP TABLE resumenes_semanales;
