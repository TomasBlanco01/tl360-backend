-- RNF-04 / RF-03: suscripciones Web Push del navegador (una por dispositivo/navegador)
-- y marca del recordatorio enviado por turno para no repetirlo.
CREATE TABLE push_suscripciones (
    id         serial PRIMARY KEY,
    usuario_id integer NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    endpoint   text NOT NULL UNIQUE CHECK (endpoint LIKE 'https://%'),
    p256dh     text NOT NULL,
    auth       text NOT NULL,
    creado_en  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_push_suscripciones_usuario ON push_suscripciones (usuario_id);

ALTER TABLE turnos ADD COLUMN recordatorio_enviado_en timestamptz;
