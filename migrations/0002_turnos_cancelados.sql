-- Cancelar un turno ya no lo borra: queda con estado 'cancelado' (historial de RF-09) y su horario
-- vuelve a estar libre (RF-05). Para eso la unicidad de RF-03 pasa a valer solo para turnos activos.
ALTER TABLE turnos DROP CONSTRAINT turnos_slot_unico;

CREATE UNIQUE INDEX turnos_slot_activo_uq
    ON turnos (empresa_id, fecha, hora)
    WHERE estado <> 'cancelado';
