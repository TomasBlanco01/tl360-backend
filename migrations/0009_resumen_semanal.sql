-- RF-14: resumen semanal por push al comercio. Una fila por comercio y semana enviada,
-- para que el proceso periódico no repita el aviso si vuelve a evaluar el mismo lunes.
CREATE TABLE resumenes_semanales (
    empresa_id    integer NOT NULL REFERENCES empresas(id) ON DELETE CASCADE,
    semana_inicio date NOT NULL,
    enviado_en    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (empresa_id, semana_inicio)
);
