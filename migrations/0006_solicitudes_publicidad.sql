-- RF-06: canal público para pedir difusión (publicidad de la plataforma o colectas/eventos
-- solidarios) sin tener cuenta, con una bandeja para que el superadmin le dé curso.
-- `publicaciones_destacadas.empresa_id` ya admitía NULL: ahora el endpoint también.
CREATE TABLE solicitudes_publicidad (
    id         serial PRIMARY KEY,
    nombre     text NOT NULL,
    contacto   text NOT NULL,                    -- email o teléfono, como lo deje la persona
    tipo       text NOT NULL DEFAULT 'publicidad' CHECK (tipo IN ('publicidad', 'colecta')),
    mensaje    text NOT NULL,
    estado     text NOT NULL DEFAULT 'pendiente' CHECK (estado IN ('pendiente', 'atendida', 'descartada')),
    creado_en  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_solicitudes_pendientes ON solicitudes_publicidad (estado, creado_en);
