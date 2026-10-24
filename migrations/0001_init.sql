-- Esquema base de TL360 (PostgreSQL / Supabase).
--
-- Criterios de diseño:
--  * Los nombres de tablas/columnas que consume el contrato de API actual (usuarios, categorias,
--    empresas, telefonos_empresa, horarios_empresa, turnos) NO cambian: el frontend no se toca.
--    Todo lo nuevo se agrega como columnas con DEFAULT/NULL o como tablas nuevas.
--  * Solo esquema. Los datos iniciales (rubros, comercios importados) viven en db/seed/ y se
--    aplican aparte, para no acoplar las migraciones a datos que pueden regenerarse.
--  * Cada regla de negocio que no puede romperse (turno único por horario, puntaje 1-5, roles,
--    horarios coherentes) se impone con constraints en la base, no solo en el backend.
--  * Cubre los RF activos del SRS v2 (RF-01..07, 09, 10, 13..16, 18); las tablas de los RF que
--    todavía no tienen endpoints quedan creadas para no migrar después con datos en producción.

CREATE EXTENSION IF NOT EXISTS pg_trgm;   -- búsqueda por nombre tolerante a errores (RF-02)

-- ---------------------------------------------------------------- usuarios (RF-01)
CREATE TABLE usuarios (
    id                serial PRIMARY KEY,
    nombre            text NOT NULL,
    email             text NOT NULL,
    "contraseña"      text NOT NULL,
    rol               text NOT NULL DEFAULT 'cliente' CHECK (rol IN ('cliente', 'admin', 'superadmin')),
    email_verificado  boolean NOT NULL DEFAULT false,
    creado_en         timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX usuarios_email_uq ON usuarios (lower(email));

-- Verificación de email (RF-01) y recuperación de contraseña: token de un solo uso, guardado hasheado.
CREATE TABLE tokens_usuario (
    id          serial PRIMARY KEY,
    usuario_id  integer NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    tipo        text NOT NULL CHECK (tipo IN ('verificar_email', 'reset_password')),
    token_hash  text NOT NULL UNIQUE,
    expira_en   timestamptz NOT NULL,
    usado_en    timestamptz,
    creado_en   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_tokens_usuario ON tokens_usuario (usuario_id, tipo);

-- ---------------------------------------------------------------- rubros
-- Dos niveles: super_rubro (ej. "Salud y bienestar") -> categoria/rubro (ej. "Cardiólogos").
CREATE TABLE super_rubros (
    id     serial PRIMARY KEY,
    nombre text NOT NULL UNIQUE,
    slug   text NOT NULL UNIQUE
);

CREATE TABLE categorias (
    id             serial PRIMARY KEY,
    nombre         text NOT NULL UNIQUE,
    slug           text UNIQUE,
    super_rubro_id integer REFERENCES super_rubros(id)
);
CREATE INDEX idx_categorias_super ON categorias (super_rubro_id);

-- ---------------------------------------------------------------- empresas (RF-02, 04, 15)
CREATE TABLE empresas (
    id                 serial PRIMARY KEY,
    nombre             text NOT NULL,
    slug               text UNIQUE,
    direccion          text NOT NULL,
    localidad          text NOT NULL DEFAULT 'Trenque Lauquen',
    codigo_postal      text,
    telefono           text,                      -- teléfono principal (contrato de API); el resto en telefonos_empresa
    slogan             text,
    descripcion        text,
    duracion_turno_min integer CHECK (duracion_turno_min IS NULL OR duracion_turno_min > 0),
    categoria_id       integer NOT NULL REFERENCES categorias(id),  -- rubro principal
    admin_id           integer REFERENCES usuarios(id) ON DELETE SET NULL,  -- dueño; NULL = ficha sin reclamar
    instagram          text,
    facebook           text,
    fotos              text[] NOT NULL DEFAULT '{}',
    imagen_perfil      text,
    imagen_portada     text,
    turnos_habilitados boolean NOT NULL DEFAULT true,   -- RF-04: false = solo ficha de contacto
    anticipacion_cancelacion_min integer NOT NULL DEFAULT 120 CHECK (anticipacion_cancelacion_min >= 0), -- RF-05
    verificado         boolean NOT NULL DEFAULT false,  -- RF-15: sello (teléfono validado + CUIT/habilitación)
    lat                numeric(9,6) CHECK (lat BETWEEN -90 AND 90),
    lng                numeric(9,6) CHECK (lng BETWEEN -180 AND 180),
    origen             text NOT NULL DEFAULT 'propio' CHECK (origen IN ('propio', 'todotrenque')),
    origen_id          text,                        -- id en la fuente, para re-importar sin duplicar
    origen_url         text,
    nivel_origen       text CHECK (nivel_origen IS NULL OR nivel_origen IN ('gratis', 'pago', 'premium')),
    creado_en          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT empresas_origen_uq UNIQUE (origen, origen_id)
);
CREATE INDEX idx_empresas_categoria ON empresas (categoria_id);
CREATE INDEX idx_empresas_admin ON empresas (admin_id) WHERE admin_id IS NOT NULL;
CREATE INDEX idx_empresas_geo ON empresas (lat, lng) WHERE lat IS NOT NULL;   -- RF-07 proximidad
CREATE INDEX idx_empresas_nombre_trgm ON empresas USING gin (nombre gin_trgm_ops);

-- Un comercio puede figurar en varios rubros (en la fuente ocurre constantemente).
CREATE TABLE empresa_categorias (
    empresa_id   integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    categoria_id integer NOT NULL REFERENCES categorias(id),
    PRIMARY KEY (empresa_id, categoria_id)
);
CREATE INDEX idx_empresa_categorias_cat ON empresa_categorias (categoria_id);

-- El rubro principal (empresas.categoria_id) siempre figura también en la relación N:M, sin depender
-- de que cada INSERT/UPDATE del backend se acuerde de mantenerla.
CREATE FUNCTION sync_empresa_categoria() RETURNS trigger AS $$
BEGIN
    INSERT INTO empresa_categorias (empresa_id, categoria_id)
    VALUES (NEW.id, NEW.categoria_id)
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_empresas_sync_categoria
    AFTER INSERT OR UPDATE OF categoria_id ON empresas
    FOR EACH ROW EXECUTE FUNCTION sync_empresa_categoria();

CREATE TABLE telefonos_empresa (
    id          serial PRIMARY KEY,
    empresa_id  integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    numero      text NOT NULL,
    etiqueta    text,
    es_whatsapp boolean NOT NULL DEFAULT false,
    CONSTRAINT telefonos_empresa_uq UNIQUE (empresa_id, numero)
);

CREATE TABLE horarios_empresa (
    id          serial PRIMARY KEY,
    empresa_id  integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    dia_semana  text NOT NULL CHECK (dia_semana IN ('Lunes','Martes','Miércoles','Jueves','Viernes','Sábado','Domingo')),
    hora_inicio time NOT NULL,
    hora_fin    time NOT NULL,
    CONSTRAINT horarios_coherentes CHECK (hora_fin > hora_inicio)
);
CREATE INDEX idx_horarios_empresa ON horarios_empresa (empresa_id);

-- ---------------------------------------------------------------- turnos (RF-03, 05)
CREATE TABLE turnos (
    id          serial PRIMARY KEY,
    empresa_id  integer NOT NULL REFERENCES empresas(id),
    usuario_id  integer NOT NULL REFERENCES usuarios(id),
    fecha       date NOT NULL,
    hora        time NOT NULL,
    estado      text NOT NULL DEFAULT 'confirmado' CHECK (estado IN ('confirmado', 'completado', 'no_show', 'cancelado')),
    creado_en   timestamptz NOT NULL DEFAULT now(),

    -- RF-03 v2: ante dos solicitudes simultáneas por el mismo horario, solo la primera se acepta.
    -- El handler hace INSERT ... ON CONFLICT DO NOTHING sobre este constraint y responde 409.
    CONSTRAINT turnos_slot_unico UNIQUE (empresa_id, fecha, hora)
);
CREATE INDEX idx_turnos_usuario ON turnos (usuario_id);
CREATE INDEX idx_turnos_empresa_fecha ON turnos (empresa_id, fecha);

-- ---------------------------------------------------------------- interacción (RF-10, 13, 16)
CREATE TABLE favoritos (
    usuario_id integer NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    empresa_id integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    creado_en  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (usuario_id, empresa_id)
);

CREATE TABLE calificaciones (
    id         serial PRIMARY KEY,
    usuario_id integer NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    empresa_id integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    puntaje    smallint NOT NULL CHECK (puntaje BETWEEN 1 AND 5),
    comentario text,
    creado_en  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT calificacion_unica UNIQUE (usuario_id, empresa_id)
);
CREATE INDEX idx_calificaciones_empresa ON calificaciones (empresa_id);

CREATE TABLE reportes (
    id         serial PRIMARY KEY,
    empresa_id integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    usuario_id integer REFERENCES usuarios(id) ON DELETE SET NULL,
    motivo     text NOT NULL,
    resuelto   boolean NOT NULL DEFAULT false,
    creado_en  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_reportes_pendientes ON reportes (creado_en) WHERE NOT resuelto;

-- ---------------------------------------------------------------- contenido (RF-06, 18)
CREATE TABLE publicaciones_destacadas (
    id          serial PRIMARY KEY,
    empresa_id  integer REFERENCES empresas(id) ON DELETE CASCADE,   -- NULL = publicación de la plataforma
    titulo      text NOT NULL,
    descripcion text,
    imagen_url  text,
    activa      boolean NOT NULL DEFAULT true,
    creado_en   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE etiquetas (
    id     serial PRIMARY KEY,
    nombre text NOT NULL UNIQUE
);
CREATE TABLE empresa_etiquetas (
    empresa_id integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    etiqueta_id integer NOT NULL REFERENCES etiquetas(id) ON DELETE CASCADE,
    PRIMARY KEY (empresa_id, etiqueta_id)
);
