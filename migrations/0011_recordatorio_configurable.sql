-- RF-03: cada cliente elige con cuánta anticipación quiere recibir el recordatorio push.
-- El valor por defecto se mantiene en 24 h para no cambiar el comportamiento existente.
ALTER TABLE usuarios
    ADD COLUMN recordatorio_horas_anticipacion integer NOT NULL DEFAULT 24
    CHECK (recordatorio_horas_anticipacion BETWEEN 1 AND 168);
