-- DATOS SIMULADOS (no provienen de ninguna fuente real). Sirven para que toda la web tenga contenido
-- de demostración: usuarios de prueba, dueños de comercio, horarios, calificaciones, etiquetas,
-- publicidades, difusiones y algunos turnos. Es determinístico (setseed) salvo las fechas de los
-- turnos, que son relativas al día en que se corre el script. Ejecutar DESPUÉS de 01_rubros.sql y
-- 02_empresas.sql.
--
-- Usuarios de prueba (contraseña de todos: Demo1234!)
--   admin@demo.com    superadmin de la plataforma
--   negocio@demo.com  dueño de comercio (rol admin), con un comercio que toma turnos
--   cliente@demo.com  cliente, con turnos reservados
--   dueno001..N@demo.tl360.test  dueños de los demás comercios con turnos
--   cliente01..40@demo.tl360.test  clientes que dejan las calificaciones simuladas

SELECT setseed(0.36);

-- ------------------------------------------------------------------ usuarios
INSERT INTO usuario (nombre, email, contrasena_hash, rol, email_verificado) VALUES
  ('Admin TL360',    'admin@demo.com',   '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'superadmin', true),
  ('Sofía Ramírez',  'negocio@demo.com', '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'admin',      true),
  ('Martina Gómez',  'cliente@demo.com', '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'cliente',    true)
ON CONFLICT DO NOTHING;

INSERT INTO usuario (nombre, email, contrasena_hash, rol, email_verificado)
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
-- Rubros en los que reservar turno tiene sentido; se eligen hasta 200. Los comercios de
-- demostración (origen 'propio') van primero para que el panel demo siempre sea el mismo.
CREATE TEMP TABLE _turneras AS
SELECT id, ROW_NUMBER() OVER (ORDER BY propio DESC, azar) AS rn
FROM (
  SELECT e.id, (e.origen = 'propio') AS propio, random() AS azar
  FROM empresa e JOIN rubro r ON r.id = e.rubro_id
  -- Vale para los comercios importados (02_empresas.sql) y para los de demostración (02_empresas_demo.sql).
  WHERE r.nombre ~* '(peluquer|barber|gimnasio|odont|kinesi|psic[oó]log|nutricion|dermat|cardi[oó]log|pediatr|ginecol|oftalm|fisioter|masaj|manicur|est[eé]tica|depilaci|yoga|pilates|veterinar|tatuaj|podolog|fonoaud|osteop|contadores|abogados|escriban|talleres mec|lavaderos de autos|canchas|p[aá]del|m[eé]dicos cl|consultorios)'
) s;

INSERT INTO usuario (nombre, email, contrasena_hash, rol, email_verificado)
SELECT 'Dueño ' || rn, 'dueno' || lpad(rn::text, 3, '0') || '@demo.tl360.test',
       '$2b$10$o7Ikm9wMNP6NDhUgMGGM7.9m8pVkWX7Dw0gCV0HiO92Tr96Jgw7OK', 'admin', true
FROM _turneras WHERE rn <= 200
ON CONFLICT DO NOTHING;

UPDATE empresa e
SET usuario_id = u.id,
    turnos_habilitados = true,
    duracion_turno_min = (ARRAY[15, 20, 30, 30, 45, 60])[1 + floor(random() * 6)::int],
    slogan = (ARRAY['Reservá tu turno online','Atención personalizada','Tu turno sin llamadas ni esperas',
                    'Profesionales de confianza en Trenque Lauquen','Calidad y buen trato'])[1 + floor(random() * 5)::int]
FROM _turneras t
JOIN usuario u ON u.email = 'dueno' || lpad(t.rn::text, 3, '0') || '@demo.tl360.test'
WHERE e.id = t.id AND t.rn <= 200;

-- El usuario negocio@demo.com queda como dueño del primer comercio con turnos.
UPDATE empresa SET usuario_id = (SELECT id FROM usuario WHERE email = 'negocio@demo.com')
WHERE id = (SELECT id FROM _turneras WHERE rn = 1);

-- ------------------------------------------------------------------ horarios (todos los comercios)
-- La fuente no publica horarios: se simulan según el tipo de rubro, con una variación de hasta 1 h
-- en la apertura.
CREATE TEMP TABLE _perfiles AS
SELECT g.grupo, d.dia, g.ini::time AS ini, g.fin::time AS fin
FROM (VALUES
  ('salud',     ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:00'),
  ('salud',     ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '15:00', '19:00'),
  ('gastro',    ARRAY['Martes','Miércoles','Jueves','Viernes','Sábado','Domingo'], '11:30', '15:00'),
  ('gastro',    ARRAY['Martes','Miércoles','Jueves','Viernes','Sábado','Domingo'], '19:30', '23:30'),
  ('deporte',   ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes','Sábado'], '08:00', '21:30'),
  ('educacion', ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:30'),
  ('educacion', ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '14:00', '18:00'),
  ('campo',     ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '08:00', '12:00'),
  ('campo',     ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '15:00', '18:30'),
  ('campo',     ARRAY['Sábado'], '08:00', '12:00'),
  -- comercio general (tiendas, construcción, automotores, servicios profesionales)
  ('general',   ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '09:00', '13:00'),
  ('general',   ARRAY['Lunes','Martes','Miércoles','Jueves','Viernes'], '16:00', '20:00'),
  ('general',   ARRAY['Sábado'], '09:00', '13:00')
) AS g(grupo, dias, ini, fin)
CROSS JOIN LATERAL unnest(g.dias) AS d(dia);

INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin)
SELECT e.id, p.dia, p.ini + (e.id % 3) * interval '20 minutes', p.fin
FROM empresa e
JOIN rubro r ON r.id = e.rubro_id
JOIN _perfiles p ON p.grupo = CASE
    WHEN r.nombre ~* '(m[eé]dic|salud|odont|kinesi|psic|nutric|dermat|pediatr|ginecol|oftalm|fisioter|masaj|manicur|est[eé]tic|depilaci|peluquer|barber|cosmiatr|yoga|pilates|farmac|veterinar|podolog|fonoaud|osteop|laborator)' THEN 'salud'
    WHEN r.nombre ~* '(gastron|restaur|bar|cafeter|parrilla|pizzer|rotiser|helader|panader|comida|cervecer)' THEN 'gastro'
    WHEN r.nombre ~* '(deport|gimnasio|canchas|p[aá]del|club|natac|hotel|hospedaje|turismo|evento|salon)' THEN 'deporte'
    WHEN r.nombre ~* '(escuela|colegio|instituto|curso|clases|profesor|jard[ií]n|estudios|universidad)' THEN 'educacion'
    WHEN r.nombre ~* '(campo|agro|agr[ií]col|cereal|hacienda|forrajer|semilla|tractor|maquinaria)' THEN 'campo'
    ELSE 'general' END;

-- ------------------------------------------------------------------ etiquetas (RF-18)
INSERT INTO etiqueta (nombre) VALUES
  ('Rampa de acceso'), ('Baño adaptado'), ('Apto celíacos'), ('Acepta mascotas'), ('Wi-Fi gratis'),
  ('Atención a domicilio'), ('Acepta tarjetas'), ('Estacionamiento propio'), ('Abierto los sábados'),
  ('Turnos por WhatsApp'), ('Envíos a domicilio'), ('Atención personalizada')
ON CONFLICT DO NOTHING;

INSERT INTO empresa_etiqueta (empresa_id, etiqueta_id)
SELECT e.id, t.id
FROM (SELECT id, CASE WHEN random() < 0.5 THEN 1 + floor(random() * 3)::int ELSE 0 END AS k FROM empresa) e
CROSS JOIN LATERAL (SELECT id FROM etiqueta WHERE e.k > 0 ORDER BY random() LIMIT e.k) t;

-- ------------------------------------------------------------------ calificaciones (RF-13)
INSERT INTO calificacion (usuario_id, empresa_id, puntaje, comentario, creado_en)
SELECT cu.id, e.id,
       (ARRAY[3, 4, 4, 5, 5, 5, 2, 4, 5, 1])[1 + floor(random() * 10)::int]::smallint,
       CASE WHEN random() < 0.6 THEN
         (ARRAY['Muy buena atención.','Excelente servicio, lo recomiendo.','Cumplieron con lo pactado.',
                'Buen precio y buena onda.','Tardaron un poco pero quedé conforme.','Todo perfecto, volveré.',
                'Atención rápida y amable.','Podría mejorar la puntualidad.','Muy profesionales.',
                'Lo recomiendo a todos en Trenque.','Buena relación precio-calidad.','Siempre me atienden bien.'])[1 + floor(random() * 12)::int]
       END,
       now() - random() * interval '300 days'
FROM (SELECT id, CASE WHEN random() < 0.55 THEN 1 + floor(random() * 4)::int ELSE 0 END AS k FROM empresa) e
CROSS JOIN LATERAL (
  SELECT u.id FROM usuario u
  WHERE e.k > 0 AND u.email LIKE 'cliente__@demo.tl360.test'
  ORDER BY random() LIMIT e.k
) cu;

-- ------------------------------------------------------------------ verificado (RF-15)
-- Simulado: en la realidad exige teléfono validado + constancia de CUIT/habilitación municipal.
UPDATE empresa SET verificado = true, verificado_estado = 'verificado'
WHERE turnos_habilitados AND random() < 0.4;

-- Para ver el circuito completo en /admin: dos solicitudes pendientes (con CUIT + constancia)
-- y una rechazada, sobre comercios que toman turnos.
UPDATE empresa SET verificado = false, verificado_estado = 'pendiente', cuit = '20123456786',
       url_cuit = 'https://example.com/constancia-barberia.pdf'
WHERE id = (SELECT id FROM _turneras WHERE rn = 2);

UPDATE empresa SET verificado = false, verificado_estado = 'pendiente', cuit = '27234567816',
       url_cuit = 'https://example.com/constancia-gimnasio.pdf'
WHERE id = (SELECT id FROM _turneras WHERE rn = 3);

UPDATE empresa SET verificado = false, verificado_estado = 'rechazado', cuit = '20987654334',
       motivo_rechazo = 'La constancia no corresponde al CUIT declarado'
WHERE id = (SELECT id FROM _turneras WHERE rn = 4);

-- ------------------------------------------------------------------ publicidades (RF-06)
-- Aprobadas (estado_aprobacion 2), para que el carrusel tenga contenido.
INSERT INTO publicidad (empresa_id, estado_aprobacion_id, titulo, descripcion, url_imagen)
SELECT e.id, 2, e.nombre,
       (ARRAY['20% de descuento este mes','Nuevos horarios de atención','Consultá por promociones',
              'Turnos disponibles esta semana','Novedades de temporada','Atención sin cargo de consulta'])[1 + floor(random() * 6)::int],
       e.url_imagen_portada
FROM empresa e
WHERE e.url_imagen_portada IS NOT NULL
ORDER BY (e.origen = 'propio') DESC, random() LIMIT 12;

-- Una pendiente del comercio demo, para ver la bandeja de aprobación del superadmin.
INSERT INTO publicidad (empresa_id, estado_aprobacion_id, titulo, descripcion, url_imagen)
SELECT t.id, 1, 'Semana del cliente', 'Todos los cortes con 15% de descuento de lunes a jueves',
       'https://picsum.photos/seed/semana-cliente/480/320'
FROM _turneras t WHERE t.rn = 1;

-- ------------------------------------------------------------------ difusiones (RF-06)
-- Colecta de la plataforma, ya aprobada, para ver el carrusel con una difusión sin ficha.
INSERT INTO difusion (estado_aprobacion_id, tipo, titulo, descripcion, url_imagen, solicitante_nombre,
                      solicitante_contacto, usuario_id)
SELECT 2, 'colecta', 'Colecta para el hospital',
       'Juntamos insumos para la sala pediátrica: sumate hasta el viernes',
       'https://picsum.photos/seed/colecta-hospital/480/320', 'Admin TL360', 'admin@demo.com', u.id
FROM usuario u WHERE u.email = 'admin@demo.com';

-- Pedidos públicos pendientes, para la bandeja de difusiones del admin.
INSERT INTO difusion (tipo, titulo, descripcion, solicitante_nombre, solicitante_contacto) VALUES
  ('colecta', 'Techo del vestuario del Club Ferroviario',
   'Recaudamos para arreglar el techo del vestuario y necesitamos difusión.',
   'Comisión del Club Ferroviario', 'club@demo.com'),
  ('evento', 'Feria de emprendedores',
   'Organizo una feria de emprendedores el sábado en la plaza principal.',
   'María Pérez', '23925551234');

-- ------------------------------------------------------------------ turnos de ejemplo (RF-03, RF-04)
INSERT INTO turno (empresa_id, usuario_id, fecha, hora)
SELECT t.id, (SELECT id FROM usuario WHERE email = 'cliente@demo.com'), current_date + (t.rn * 2)::int, '10:00'
FROM _turneras t WHERE t.rn IN (1, 2, 3)
ON CONFLICT DO NOTHING;

-- Turno de mostrador que el comercio demo cargó a mano para alguien sin cuenta (por teléfono).
INSERT INTO turno (empresa_id, origen, fecha, hora, cliente_nombre, cliente_email, nota)
SELECT t.id, 'mostrador', current_date + 3, '12:00', 'Carlos Medina', 'carlos.medina@ejemplo.com', 'Llamó por teléfono'
FROM _turneras t WHERE t.rn = 1
ON CONFLICT DO NOTHING;

-- Atenciones de mostrador ya hechas (estado completado), para el resumen del comercio demo.
INSERT INTO turno (empresa_id, origen, estado_turno_id, fecha, hora, cliente_nombre, nota)
SELECT t.id, 'mostrador', 3, current_date - d, h::time, c, 'Atención sin reserva'
FROM _turneras t
CROSS JOIN (VALUES (1, '09:30', 'Juan Pérez'), (1, '11:00', 'Laura Díaz'), (8, '10:30', 'Pedro Sosa')) AS a(d, h, c)
WHERE t.rn = 1
ON CONFLICT DO NOTHING;

-- Algunos horarios ya ocupados por otros clientes, para ver el turnero con disponibilidad parcial.
-- El cliente se elige con un JOIN determinístico: un random() dentro de una subconsulta escalar se
-- reevalúa fila por fila y puede no encontrar a nadie (usuario_id NULL) y abortar todo el seed.
INSERT INTO turno (empresa_id, usuario_id, fecha, hora)
SELECT t.id, u.id, current_date + d, h::time
FROM _turneras t
CROSS JOIN generate_series(1, 5) AS d
CROSS JOIN (VALUES ('10:00'), ('11:00'), ('17:00')) AS hs(h)
JOIN usuario u ON u.email = 'cliente' || lpad((1 + (t.rn * 7 + d * 3) % 40)::text, 2, '0') || '@demo.tl360.test'
WHERE t.rn <= 30 AND random() < 0.35
ON CONFLICT DO NOTHING;

DROP TABLE _turneras;
DROP TABLE _perfiles;
