-- RF-02: cada comercio decide si su teléfono se muestra para llamada, mensaje o ambos.
-- El modo del comercio es el valor por defecto; cada teléfono puede pisarlo o heredarlo (NULL).
ALTER TABLE empresas
    ADD COLUMN modo_contacto text NOT NULL DEFAULT 'ambos'
    CHECK (modo_contacto IN ('llamada', 'mensaje', 'ambos'));

ALTER TABLE telefonos_empresa
    ADD COLUMN modo_contacto text
    CHECK (modo_contacto IS NULL OR modo_contacto IN ('llamada', 'mensaje', 'ambos'));
