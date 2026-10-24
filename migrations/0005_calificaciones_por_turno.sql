-- RF-13: una calificación por turno y solo después del horario del turno.
-- Las reseñas históricas (importadas) quedan con turno_id NULL y siguen contando para el promedio;
-- las nuevas se atan a un turno del propio cliente.
ALTER TABLE calificaciones ADD COLUMN turno_id integer REFERENCES turnos(id) ON DELETE CASCADE;

ALTER TABLE calificaciones DROP CONSTRAINT calificacion_unica;

-- Un turno se califica una sola vez (el handler hace upsert sobre este índice).
CREATE UNIQUE INDEX calificaciones_turno_unico ON calificaciones (turno_id) WHERE turno_id IS NOT NULL;
