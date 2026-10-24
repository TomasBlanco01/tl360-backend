-- RF-15: flujo de verificación comercial con CUIT: el dueño solicita (CUIT + constancia) → queda
-- 'pendiente'; el superadmin aprueba o rechaza. El booleano `verificado` se mantiene en sincronía:
-- aprobar lo enciende, rechazar lo apaga, y el toggle clásico actualiza ambos (ver moderacion.rs).
ALTER TABLE empresas ADD COLUMN verificado_estado text NOT NULL DEFAULT 'sin_solicitar';
ALTER TABLE empresas ADD CONSTRAINT empresas_verificado_estado_ck
    CHECK (verificado_estado IN ('sin_solicitar', 'pendiente', 'verificado', 'rechazado'));
ALTER TABLE empresas ADD COLUMN cuit text;
ALTER TABLE empresas ADD COLUMN constancia_cuit_url text;

-- Los sellos otorgados antes de este cambio quedan con un estado coherente.
UPDATE empresas SET verificado_estado = 'verificado' WHERE verificado;

-- La bandeja de /admin filtra por pendientes: índice parcial para esa consulta.
CREATE INDEX idx_empresas_verificacion_pendiente ON empresas (verificado_estado)
    WHERE verificado_estado = 'pendiente';
