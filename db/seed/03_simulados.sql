-- DATOS SIMULADOS (no provienen de ninguna fuente real). Sirven para que toda la web tenga contenido
-- de demostración: usuarios de prueba, dueños de comercio, horarios, calificaciones, etiquetas,
-- publicidades y algunos turnos. Es determinístico (setseed) salvo las fechas de los turnos, que son
-- relativas al día en que se corre el script. Ejecutar DESPUÉS de 01_rubros.sql y 02_empresas.sql.
--
-- Usuarios de prueba (contraseña de todos: Demo1234!)
--   admin@demo.com    superadmin de la plataforma
--   negocio@demo.com  dueño de comercio (rol admin), con un comercio que toma turnos
--   cliente@demo.com  cliente, con turnos reservados
--   dueno001..N@demo.tl360.test  dueños de los demás comercios con turnos
--   cliente01..40@demo.tl360.test  clientes que dejan las calificaciones simuladas

SELECT setseed(0.36);

-- ------------------------------------------------------------------ usuarios
INSERT INTO usuarios (nombre, email, "contraseña", rol, email_verificado) VALUES
  ('Admin TL360',    'admin@demo.com',   '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'superadmin', true),
  ('Sofía Ramírez',  'negocio@demo.com', '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'admin',      true),
  ('Martina Gómez',  'cliente@demo.com', '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'cliente',    true)
ON CONFLICT DO NOTHING;

INSERT INTO usuarios (nombre, email, "contraseña", rol, email_verificado)
SELECT (ARRAY['Sofía','Mateo','Valentina','Benjamín','Camila','Lucas','Julieta','Tomás','Martina','Joaquín',
              'Lucía','Santiago','Agustina','Nicolás','Florencia','Facundo','Micaela','Bruno','Abril','Franco'])[1 + (n * 7) % 20]
       || ' ' ||
       (ARRAY['Gómez','Fernández','López','Díaz','Martínez','Pérez','Romero','Sosa','Álvarez','Torres',
              'Ruiz','Ramírez','Flores','Acosta','Benítez','Medina','Herrera','Suárez','Aguirre','Rojas'])[1 + (n * 3) % 20],
       'cliente' || lpad(n::text, 2, '0') || '@demo.tl360.test',
       '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'cliente', true
FROM generate_series(1, 40) AS n
ON CONFLICT DO NOTHING;

-- ------------------------------------------------------------------ comercios que toman turnos (RF-04)
-- Rubros en los que reservar turno tiene sentido; se eligen hasta 200, priorizando los de nivel pago/premium.
CREATE TEMP TABLE _turneras AS
SELECT id, ROW_NUMBER() OVER (ORDER BY prioridad DESC, azar) AS rn
FROM (
  SELECT e.id,
         (CASE e.nivel_origen WHEN 'premium' THEN 2 WHEN 'pago' THEN 1 ELSE 0 END) AS prioridad,
         random() AS azar
  FROM empresas e JOIN categorias c ON c.id = e.categoria_id
  WHERE e.origen = 'todotrenque'
    AND c.nombre ~* '(peluquer|barber|gimnasio|odont|kinesi|psic[oó]log|nutricion|dermat|cardi[oó]log|pediatr|ginecol|oftalm|fisioter|masaj|manicur|est[eé]tica|depilaci|yoga|pilates|veterinar|tatuaj|podolog|fonoaud|osteop|contadores|abogados|escriban|talleres mec|lavaderos de autos|canchas|p[aá]del|m[eé]dicos cl|consultorios)'
) s;

INSERT INTO usuarios (nombre, email, "contraseña", rol, email_verificado)
SELECT 'Dueño ' || rn, 'dueno' || lpad(rn::text, 3, '0') || '@demo.tl360.test',
       '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'admin', true
FROM _turneras WHERE rn <= 200
ON CONFLICT DO NOTHING;

UPDATE empresas e
SET admin_id = u.id,
    turnos_habilitados = true,
    duracion_turno_min = (ARRAY[15, 20, 30, 30, 45, 60])[1 + floor(random() * 6)::int],
    slogan = (ARRAY['Reservá tu turno online','Atención personalizada','Tu turno sin llamadas ni esperas',
                    'Profesionales de confianza en Trenque Lauquen','Calidad y buen trato'])[1 + floor(random() * 5)::int]
FROM _turneras t
JOIN usuarios u ON u.email = 'dueno' || lpad(t.rn::text, 3, '0') || '@demo.tl360.test'
WHERE e.id = t.id AND t.rn <= 200;

-- El usuario negocio@demo.com queda como dueño del primer comercio con turnos.
UPDATE empresas SET admin_id = (SELECT id FROM usuarios WHERE email = 'negocio@demo.com')
WHERE id = (SELECT id FROM _turneras WHERE rn = 1);

-- ------------------------------------------------------------------ horarios (todos los comercios)
-- La fuente no publica horarios: se simulan según el rubro, con una variación de hasta 1 h en la apertura.
CREATE TEMP TABLE _perfiles AS
SELECT g.grupo, d.dia, g.ini::time AS ini, g.fin::time AS fin
FROM (VALUES
  ('Salud, belleza y bienestar',           ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:00'),
  ('Salud, belleza y bienestar',           ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '15:00', '19:00'),
  ('Gastronomía',                 ARRAY['Martes','Miércoles','Jueves','Viernes','Sábado','Domingo'], '11:30', '15:00'),
  ('Gastronomía',                 ARRAY['Martes','Miércoles','Jueves','Viernes','Sábado','Domingo'], '19:30', '23:30'),
  ('Deportes, turismo y eventos', ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes','Sábado'], '08:00', '21:30'),
  ('Educación y cultura',         ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:30'),
  ('Educación y cultura',         ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '14:00', '18:00'),
  ('Campo y agro',                ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:00'),
  ('Campo y agro',                ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '15:00', '18:30'),
  ('Campo y agro',                ARRAY['Sábado'], '08:00', '12:00'),
  ('Otros servicios',             ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '09:00', '18:00'),
  -- comercio general (tiendas, construcción, automotores, servicios profesionales)
  ('__general__',                 ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '09:00', '13:00'),
  ('__general__',                 ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '16:00', '20:00'),
  ('__general__',                 ARRAY['Sábado'], '09:00', '13:00')
) AS g(grupo, dias, ini, fin)
CROSS JOIN LATERAL unnest(g.dias) AS d(dia);

INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin)
SELECT e.id, p.dia, p.ini + (e.id % 3) * interval '20 minutes', p.fin
FROM empresas e
JOIN categorias c ON c.id = e.categoria_id
JOIN super_rubros sr ON sr.id = c.super_rubro_id
JOIN _perfiles p ON p.grupo = CASE WHEN sr.nombre IN ('Salud, belleza y bienestar','Gastronomía','Deportes, turismo y eventos',
                                                       'Educación y cultura','Campo y agro','Otros servicios')
                                   THEN sr.nombre ELSE '__general__' END;

-- ------------------------------------------------------------------ etiquetas (RF-18)
INSERT INTO etiquetas (nombre) VALUES
  ('Rampa de acceso'), ('Baño adaptado'), ('Apto celíacos'), ('Acepta mascotas'), ('Wi-Fi gratis'),
  ('Atención a domicilio'), ('Acepta tarjetas'), ('Estacionamiento propio'), ('Abierto los sábados'),
  ('Turnos por WhatsApp'), ('Envíos a domicilio'), ('Atención personalizada')
ON CONFLICT DO NOTHING;

INSERT INTO empresa_etiquetas (empresa_id, etiqueta_id)
SELECT e.id, t.id
FROM (SELECT id, CASE WHEN random() < 0.5 THEN 1 + floor(random() * 3)::int ELSE 0 END AS k FROM empresas) e
CROSS JOIN LATERAL (SELECT id FROM etiquetas WHERE e.k > 0 ORDER BY random() LIMIT e.k) t;

-- ------------------------------------------------------------------ calificaciones (RF-13)
INSERT INTO calificaciones (usuario_id, empresa_id, puntaje, comentario, creado_en)
SELECT cu.id, e.id,
       (ARRAY[3, 4, 4, 5, 5, 5, 2, 4, 5, 1])[1 + floor(random() * 10)::int]::smallint,
       CASE WHEN random() < 0.6 THEN
         (ARRAY['Muy buena atención.','Excelente servicio, lo recomiendo.','Cumplieron con lo pactado.',
                'Buen precio y buena onda.','Tardaron un poco pero quedé conforme.','Todo perfecto, volveré.',
                'Atención rápida y amable.','Podría mejorar la puntualidad.','Muy profesionales.',
                'Lo recomiendo a todos en Trenque.','Buena relación precio-calidad.','Siempre me atienden bien.'])[1 + floor(random() * 12)::int]
       END,
       now() - random() * interval '300 days'
FROM (SELECT id, CASE WHEN random() < 0.55 THEN 1 + floor(random() * 4)::int ELSE 0 END AS k FROM empresas) e
CROSS JOIN LATERAL (
  SELECT u.id FROM usuarios u
  WHERE e.k > 0 AND u.email LIKE 'cliente__@demo.tl360.test'
  ORDER BY random() LIMIT e.k
) cu;

-- ------------------------------------------------------------------ verificado (RF-15)
-- Simulado: en la realidad exige teléfono validado + constancia de CUIT/habilitación municipal.
UPDATE empresas SET verificado = true, verificado_estado = 'verificado'
WHERE nivel_origen = 'premium' OR (turnos_habilitados AND random() < 0.4);

-- Para ver el circuito completo en /admin: dos solicitudes pendientes (con CUIT + constancia)
-- y una rechazada, sobre comercios que toman turnos.
UPDATE empresas SET verificado = false, verificado_estado = 'pendiente', cuit = '20123456786',
       constancia_cuit_url = 'https://example.com/constancia-barberia.pdf'
WHERE id = (SELECT id FROM _turneras WHERE rn = 2);

UPDATE empresas SET verificado = false, verificado_estado = 'pendiente', cuit = '27234567816',
       constancia_cuit_url = 'https://example.com/constancia-gimnasio.pdf'
WHERE id = (SELECT id FROM _turneras WHERE rn = 3);

UPDATE empresas SET verificado = false, verificado_estado = 'rechazado', cuit = '20987654334'
WHERE id = (SELECT id FROM _turneras WHERE rn = 4);

-- ------------------------------------------------------------------ publicidades (RF-06)
INSERT INTO publicaciones_destacadas (empresa_id, titulo, descripcion, imagen_url)
SELECT e.id, e.nombre,
       (ARRAY['20% de descuento este mes','Nuevos horarios de atención','Consultá por promociones',
              'Turnos disponibles esta semana','Novedades de temporada','Atención sin cargo de consulta'])[1 + floor(random() * 6)::int],
       e.imagen_portada
FROM empresas e
WHERE e.nivel_origen IN ('premium', 'pago') AND e.imagen_portada IS NOT NULL
ORDER BY random() LIMIT 12;

-- Colecta solidaria de la plataforma (sin comercio), para ver el carrusel y la difusión sin ficha.
INSERT INTO publicaciones_destacadas (empresa_id, titulo, descripcion, imagen_url)
VALUES (NULL, 'Colecta para el hospital',
        'Juntamos insumos para la sala pediátrica: sumate hasta el viernes', NULL);

-- Pedidos de ejemplo para la bandeja "Solicitudes" del admin (RF-06).
INSERT INTO solicitudes_publicidad (nombre, contacto, tipo, mensaje) VALUES
  ('Comisión del Club Ferroviario', 'club@demo.com', 'colecta',
   'Recaudamos para arreglar el techo del vestuario y necesitamos difusión.'),
  ('María Pérez', '23925551234', 'publicidad',
   'Organizo una feria de emprendedores el sábado en la plaza principal.');

-- Walk-ins de ejemplo para el panel del comercio demo (RF-04).
INSERT INTO atenciones_mostrador (empresa_id, fecha, cantidad, importe_centavos, nota)
SELECT e.id, current_date, 3, 1200000, 'Cortes de la mañana'
FROM empresas e
WHERE e.admin_id = (SELECT id FROM usuarios WHERE email = 'negocio@demo.com');

INSERT INTO atenciones_mostrador (empresa_id, fecha, cantidad, importe_centavos, nota)
SELECT e.id, current_date - 40, 12, NULL, 'Cortes del mes pasado'
FROM empresas e
WHERE e.admin_id = (SELECT id FROM usuarios WHERE email = 'negocio@demo.com');

-- ------------------------------------------------------------------ turnos de ejemplo (RF-03)
INSERT INTO turnos (empresa_id, usuario_id, fecha, hora)
SELECT t.id, (SELECT id FROM usuarios WHERE email = 'cliente@demo.com'), current_date + (t.rn * 2)::int, '10:00'
FROM _turneras t WHERE t.rn IN (1, 2, 3)
ON CONFLICT DO NOTHING;

-- Algunos horarios ya ocupados por otros clientes, para ver el turnero con disponibilidad parcial.
INSERT INTO turnos (empresa_id, usuario_id, fecha, hora)
SELECT t.id,
       (SELECT id FROM usuarios WHERE email = 'cliente' || lpad((1 + floor(random() * 40))::int::text, 2, '0') || '@demo.tl360.test'),
       current_date + d, h::time
FROM _turneras t
CROSS JOIN generate_series(1, 5) AS d
CROSS JOIN (VALUES ('10:00'), ('11:00'), ('17:00')) AS hs(h)
WHERE t.rn <= 30 AND random() < 0.35
ON CONFLICT DO NOTHING;

DROP TABLE _turneras;
DROP TABLE _perfiles;
