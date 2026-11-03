-- Correcciones del profesor que necesitan datos nuevos.

-- ---------------------------------------------------------------- motivo de rechazo (RF-15)
-- Rechazar una verificación exige explicar por qué, para que el dueño sepa qué corregir. El
-- motivo existe solo mientras la solicitud está rechazada: una nueva solicitud o la aprobación lo
-- limpian. Los rechazos anteriores a esta migración no tenían motivo y se marcan como tales.
ALTER TABLE empresas ADD COLUMN motivo_rechazo text;
UPDATE empresas SET motivo_rechazo = 'Rechazada antes de que se registrara el motivo'
WHERE verificado_estado = 'rechazado';
ALTER TABLE empresas ADD CONSTRAINT empresas_motivo_rechazo_ck
    CHECK ((verificado_estado = 'rechazado') = (motivo_rechazo IS NOT NULL)
           AND (motivo_rechazo IS NULL OR char_length(btrim(motivo_rechazo)) BETWEEN 5 AND 500));

-- ---------------------------------------------------------------- turnos cargados por el comercio (RF-04)
-- El dueño puede agendar un turno para alguien sin cuenta (por teléfono o en el local) dejando
-- su nombre y, si quiere, su correo. Un turno es de un cliente registrado o de una persona sin
-- cuenta, nunca de los dos: así el nombre no duplica datos de usuarios (3FN).
ALTER TABLE turnos ALTER COLUMN usuario_id DROP NOT NULL;
ALTER TABLE turnos ADD COLUMN cliente_nombre text;
ALTER TABLE turnos ADD COLUMN cliente_email text;
ALTER TABLE turnos ADD CONSTRAINT turnos_cliente_ck CHECK (
    (usuario_id IS NOT NULL AND cliente_nombre IS NULL AND cliente_email IS NULL)
    -- cliente_nombre IS NOT NULL explícito: con NULL, char_length daría NULL y el CHECK lo dejaría pasar.
    OR (usuario_id IS NULL AND cliente_nombre IS NOT NULL
        AND char_length(btrim(cliente_nombre)) BETWEEN 1 AND 100)
);
ALTER TABLE turnos ADD CONSTRAINT turnos_cliente_email_ck
    CHECK (cliente_email IS NULL OR cliente_email ~ '^[^@[:space:]]+@[^@[:space:]]+\.[^@[:space:]]+$');
