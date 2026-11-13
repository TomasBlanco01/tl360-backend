-- Modelo de datos v2, segunda parte: que no quede ningún nombre del modelo anterior.
-- * 0015 renombró tablas y columnas, pero Postgres conserva los nombres de restricciones e índices
--   (empresas_admin_id_fkey, empresas_categoria_id_fkey, usuarios_contraseña_not_null, ...).
--   Acá se les pone el nombre de la tabla y la columna actuales.
-- * lat y lng pasan a DECIMAL(9,6), como en el modelo lógico (6 decimales ≈ 11 cm).

-- ================================================================ restricciones
-- Nombre estándar según el tipo: tabla_pkey, tabla_columnas_fkey, tabla_columnas_key,
-- tabla_columna_not_null y tabla_columna_check. Las que abarcan varias columnas conservan su nombre
-- descriptivo, con el prefijo de la tabla actual.
DO $$
DECLARE
    r record;
    columnas text;
    sufijo text;
    nuevo text;
    viejo_prefijo text;
    prefijos constant text[][] := ARRAY[
        ['calificaciones', 'calificacion'], ['empresas', 'empresa'],
        ['empresa_etiquetas', 'empresa_etiqueta'], ['etiquetas', 'etiqueta'],
        ['favoritos', 'favorito'], ['horarios_empresa', 'horario_empresa'],
        ['horarios', 'horario_empresa'], ['publicaciones_destacadas', 'publicidad'],
        ['push_suscripciones', 'push_suscripcion'], ['reportes', 'reporte'],
        ['categorias', 'rubro'], ['telefonos_empresa', 'telefono_empresa'],
        ['tokens_usuario', 'token_usuario'], ['turnos', 'turno'], ['usuarios', 'usuario']];
BEGIN
    FOR r IN
        SELECT c.conname, c.contype, c.conkey, c.conrelid, c.conrelid::regclass::text AS tabla
        FROM pg_constraint c
        WHERE c.connamespace = 'public'::regnamespace
          AND c.conrelid::regclass::text NOT LIKE '\_sqlx%'
    LOOP
        SELECT string_agg(a.attname, '_' ORDER BY k.n) INTO columnas
        FROM unnest(r.conkey) WITH ORDINALITY AS k(attnum, n)
        JOIN pg_attribute a ON a.attrelid = r.conrelid AND a.attnum = k.attnum;

        sufijo := CASE r.contype
            WHEN 'p' THEN 'pkey' WHEN 'f' THEN 'fkey' WHEN 'u' THEN 'key'
            WHEN 'n' THEN 'not_null' WHEN 'c' THEN 'check' END;

        IF r.contype = 'p' THEN
            nuevo := r.tabla || '_pkey';
        ELSIF r.contype IN ('f', 'u', 'n') OR (r.contype = 'c' AND cardinality(r.conkey) = 1) THEN
            -- Una FK compuesta (calificación → turno) deja el nombre descriptivo.
            IF r.contype = 'f' AND cardinality(r.conkey) > 1 THEN
                nuevo := NULL;
            ELSE
                nuevo := r.tabla || '_' || columnas || '_' || sufijo;
            END IF;
        ELSE
            nuevo := NULL;
        END IF;

        IF nuevo IS NULL THEN
            nuevo := r.conname;
            FOR i IN 1 .. array_length(prefijos, 1) LOOP
                viejo_prefijo := prefijos[i][1] || '_';
                IF left(r.conname, length(viejo_prefijo)) = viejo_prefijo THEN
                    nuevo := prefijos[i][2] || '_' || substr(r.conname, length(viejo_prefijo) + 1);
                    EXIT;
                END IF;
            END LOOP;
        END IF;

        IF nuevo <> r.conname THEN
            EXECUTE format('ALTER TABLE %I RENAME CONSTRAINT %I TO %I', r.tabla, r.conname, nuevo);
        END IF;
    END LOOP;
END $$;

-- ================================================================ índices que no son restricciones
ALTER INDEX calificaciones_turno_unico RENAME TO calificacion_turno_uq;
ALTER INDEX idx_calificaciones_empresa RENAME TO idx_calificacion_empresa;
ALTER INDEX idx_empresas_nombre_trgm RENAME TO idx_empresa_nombre_trgm;
ALTER INDEX idx_empresas_geo RENAME TO idx_empresa_geo;
ALTER INDEX idx_empresas_verificacion_pendiente RENAME TO idx_empresa_verificacion_pendiente;
ALTER INDEX idx_horarios_empresa RENAME TO idx_horario_empresa;
ALTER INDEX idx_push_suscripciones_usuario RENAME TO idx_push_suscripcion_usuario;
ALTER INDEX idx_tokens_usuario RENAME TO idx_token_usuario;
ALTER INDEX usuarios_email_uq RENAME TO usuario_email_uq;

-- ================================================================ coordenadas
ALTER TABLE empresa
    ALTER COLUMN lat TYPE decimal(9, 6),
    ALTER COLUMN lng TYPE decimal(9, 6);
