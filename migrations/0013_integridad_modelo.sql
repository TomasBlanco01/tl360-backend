-- Reglas que hasta ahora solo cuidaba la API pasan a la base, y se corrige el trigger del rubro
-- principal. Antes de cada restricción se normalizan los datos existentes para que la migración
-- no falle sobre una base con historia.

-- ---------------------------------------------------------------- rubro principal (RF-02, RF-04)
-- Al cambiar empresas.categoria_id, el rubro anterior deja de figurar entre los del comercio;
-- antes quedaba asociado y el comercio aparecía en los dos rubros.
CREATE OR REPLACE FUNCTION sync_empresa_categoria() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.categoria_id IS DISTINCT FROM NEW.categoria_id THEN
        DELETE FROM empresa_categorias
        WHERE empresa_id = NEW.id AND categoria_id = OLD.categoria_id;
    END IF;
    INSERT INTO empresa_categorias (empresa_id, categoria_id)
    VALUES (NEW.id, NEW.categoria_id)
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ---------------------------------------------------------------- verificación (RF-15)
-- El sello visible y el estado del circuito no pueden contradecirse: verificado_estado manda.
UPDATE empresas SET verificado = (verificado_estado = 'verificado')
WHERE verificado <> (verificado_estado = 'verificado');
ALTER TABLE empresas ADD CONSTRAINT empresas_verificado_sync_ck
    CHECK (verificado = (verificado_estado = 'verificado'));

-- El CUIT se guarda normalizado: 11 dígitos, sin guiones (igual que normalizar_cuit en la API).
UPDATE empresas SET cuit = regexp_replace(cuit, '[^0-9]', '', 'g')
WHERE cuit IS NOT NULL AND cuit !~ '^[0-9]{11}$';
UPDATE empresas SET cuit = NULL WHERE cuit = '';
ALTER TABLE empresas ADD CONSTRAINT empresas_cuit_ck
    CHECK (cuit IS NULL OR cuit ~ '^[0-9]{11}$');

-- ---------------------------------------------------------------- pendientes únicos (RF-16, RF-06)
-- La API consulta antes de insertar, pero dos requests simultáneos podían pasar los dos.
-- Si ya hubiera duplicados, se conserva el más antiguo y el resto se cierra.
UPDATE reportes r SET resuelto = true
WHERE NOT r.resuelto AND r.usuario_id IS NOT NULL
  AND EXISTS (SELECT 1 FROM reportes o
              WHERE o.empresa_id = r.empresa_id AND o.usuario_id = r.usuario_id
                AND NOT o.resuelto AND o.id < r.id);
CREATE UNIQUE INDEX reportes_pendiente_uq
    ON reportes (empresa_id, usuario_id) WHERE NOT resuelto;

UPDATE solicitudes_publicidad s SET estado = 'descartada'
WHERE s.estado = 'pendiente'
  AND EXISTS (SELECT 1 FROM solicitudes_publicidad o
              WHERE lower(o.contacto) = lower(s.contacto) AND o.estado = 'pendiente' AND o.id < s.id);
CREATE UNIQUE INDEX solicitudes_pendiente_uq
    ON solicitudes_publicidad (lower(contacto)) WHERE estado = 'pendiente';

-- ---------------------------------------------------------------- calificación coherente (RF-13)
-- Una calificación atada a un turno debe ser del mismo cliente y del mismo comercio que el turno.
-- La FK compuesta solo se evalúa cuando turno_id no es NULL (MATCH SIMPLE), así que las reseñas
-- sin turno siguen permitidas. Esto controla en la base la desnormalización calificaciones.empresa_id.
ALTER TABLE turnos ADD CONSTRAINT turnos_id_empresa_usuario_uq UNIQUE (id, empresa_id, usuario_id);
ALTER TABLE calificaciones ADD CONSTRAINT calificaciones_turno_coherente_fk
    FOREIGN KEY (turno_id, empresa_id, usuario_id)
    REFERENCES turnos (id, empresa_id, usuario_id) ON DELETE CASCADE;
