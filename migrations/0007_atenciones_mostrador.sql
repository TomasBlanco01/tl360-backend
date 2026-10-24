-- RF-04 (a sumar 16/10): atenciones sin reserva (walk-ins) que el comercio marca a mano
-- para llevar un acumulado de servicios (cortes, etc.) y, opcionalmente, del dinero cobrado.
-- No es un sistema de cobros: el importe es solo un registro contable manual en centavos.
CREATE TABLE atenciones_mostrador (
    id               serial PRIMARY KEY,
    empresa_id       integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    fecha            date NOT NULL,
    cantidad         integer NOT NULL CHECK (cantidad BETWEEN 1 AND 1000),
    importe_centavos bigint CHECK (importe_centavos IS NULL OR importe_centavos > 0),
    nota             text CHECK (nota IS NULL OR char_length(nota) <= 300),
    creado_por       integer REFERENCES usuarios(id) ON DELETE SET NULL,
    creado_en        timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_atenciones_mostrador_empresa_fecha
    ON atenciones_mostrador (empresa_id, fecha DESC, id DESC);
