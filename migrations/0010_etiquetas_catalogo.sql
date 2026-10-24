-- RF-18: alinea las etiquetas históricas con el catálogo canónico que usa el frontend.
-- Las etiquetas libres (por ejemplo "Atención a domicilio") se conservan tal cual.

INSERT INTO etiquetas (nombre)
SELECT DISTINCT nueva
FROM (VALUES
    ('Acceso para sillas de ruedas', 'Rampa de acceso'),
    ('Rampa', 'Rampa de acceso'),
    ('Apto celíaco', 'Apto celíacos'),
    ('Pet friendly', 'Acepta mascotas'),
    ('Wi-Fi', 'Wi-Fi gratis')
) AS equivalencias(vieja, nueva)
ON CONFLICT (nombre) DO NOTHING;

INSERT INTO empresa_etiquetas (empresa_id, etiqueta_id)
SELECT DISTINCT ee.empresa_id, nueva.id
FROM (VALUES
    ('Acceso para sillas de ruedas', 'Rampa de acceso'),
    ('Rampa', 'Rampa de acceso'),
    ('Apto celíaco', 'Apto celíacos'),
    ('Pet friendly', 'Acepta mascotas'),
    ('Wi-Fi', 'Wi-Fi gratis')
) AS equivalencias(vieja, nueva)
JOIN etiquetas vieja ON vieja.nombre = equivalencias.vieja
JOIN etiquetas nueva ON nueva.nombre = equivalencias.nueva
JOIN empresa_etiquetas ee ON ee.etiqueta_id = vieja.id
ON CONFLICT DO NOTHING;

DELETE FROM etiquetas vieja
USING (VALUES
    ('Acceso para sillas de ruedas'),
    ('Rampa'),
    ('Apto celíaco'),
    ('Pet friendly'),
    ('Wi-Fi')
) AS equivalencias(nombre)
WHERE vieja.nombre = equivalencias.nombre;
