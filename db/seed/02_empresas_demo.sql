-- DATOS DE DEMOSTRACIÓN (ficticios): 54 comercios inventados, los mismos del modo
-- mock del frontend. Sirven para levantar el proyecto desde un clon limpio, porque los comercios
-- importados (02_empresas.sql) no se versionan. tools/devdb.mjs y tools/validate.mjs usan este
-- archivo solo cuando falta 02_empresas.sql. Las coordenadas son aproximadas y no corresponden a
-- comercios reales. Ejecutar DESPUÉS de 01_rubros.sql y ANTES de 03_simulados.sql.

-- Los datos se preparan en una tabla temporal y se reparten entre empresa, su teléfono principal,
-- sus redes sociales y sus fotos (modelo v2).
CREATE TEMP TABLE _demo AS
SELECT * FROM (VALUES
  ('Estilo Urbano Peluquería', 'estilo-urbano-peluqueria', 'Villegas 482', '2392411234', 'Cortes modernos, color y peinados para toda ocasión', 'Peluquerías', 'https://instagram.com/estilourbano.tl', NULL, ARRAY['https://picsum.photos/seed/peluqueria1-a/480/320', 'https://picsum.photos/seed/peluqueria1-b/480/320']::text[], 'https://picsum.photos/seed/peluqueria1-a/480/320', 'ambos', true, 30, -35.964300, -62.738800, 'demo-1'),
  ('Barbería Dorrego', 'barberia-dorrego', 'Dorrego 156', '2392422987', 'Cortes clásicos y afeitado a la antigua', 'Peluquerías', NULL, NULL, ARRAY['https://picsum.photos/seed/barberia2-a/480/320']::text[], 'https://picsum.photos/seed/barberia2-a/480/320', 'ambos', true, 25, -35.965900, -62.733800, 'demo-2'),
  ('PowerFit Trenque', 'powerfit-trenque', 'San Martín 745', '2392433551', 'Musculación, funcional y clases grupales', 'Gimnasios', 'https://instagram.com/powerfit.tl', 'https://facebook.com/powerfittl', ARRAY['https://picsum.photos/seed/gym3-a/480/320', 'https://picsum.photos/seed/gym3-b/480/320', 'https://picsum.photos/seed/gym3-c/480/320']::text[], 'https://picsum.photos/seed/gym3-a/480/320', 'ambos', true, 60, -35.967500, -62.728800, 'demo-3'),
  ('Gimnasio Sarmiento', 'gimnasio-sarmiento', 'Sarmiento 210', '2392444120', 'Tu gimnasio de barrio, de toda la vida', 'Gimnasios', NULL, NULL, ARRAY['https://picsum.photos/seed/gym4-a/480/320']::text[], 'https://picsum.photos/seed/gym4-a/480/320', 'ambos', true, 60, -35.969100, -62.723800, 'demo-4'),
  ('Masajes Bienestar', 'masajes-bienestar', 'Belgrano 330', '2392455678', 'Descontracturantes, relajantes y drenaje linfático', 'Masajistas', 'https://instagram.com/bienestar.masajes', NULL, ARRAY['https://picsum.photos/seed/masajes5-a/480/320', 'https://picsum.photos/seed/masajes5-b/480/320']::text[], 'https://picsum.photos/seed/masajes5-a/480/320', 'ambos', true, 45, -35.970700, -62.740300, 'demo-5'),
  ('Zen Spa Masajes', 'zen-spa-masajes', 'Moreno 88', '2392466234', 'Un momento para vos', 'Masajistas', NULL, NULL, ARRAY['https://picsum.photos/seed/spa6-a/480/320']::text[], 'https://picsum.photos/seed/spa6-a/480/320', 'ambos', true, 60, -35.972300, -62.735300, 'demo-6'),
  ('Estética Integral Roca', 'estetica-integral-roca', 'Roca 512', '2392477845', 'Limpieza facial, uñas y maquillaje profesional', 'Cosmiatría', 'https://instagram.com/esteticaroca', 'https://facebook.com/esteticaroca', ARRAY['https://picsum.photos/seed/estetica7-a/480/320', 'https://picsum.photos/seed/estetica7-b/480/320']::text[], 'https://picsum.photos/seed/estetica7-a/480/320', 'ambos', true, 40, -35.973900, -62.730300, 'demo-7'),
  ('Belleza Alsina', 'belleza-alsina', 'Alsina 640', '2392488112', 'Manicura, pedicura y cejas', 'Cosmiatría', NULL, NULL, ARRAY['https://picsum.photos/seed/estetica8-a/480/320']::text[], 'https://picsum.photos/seed/estetica8-a/480/320', 'ambos', true, 35, -35.975500, -62.725300, 'demo-8'),
  ('Depilación Definitiva Mitre', 'depilacion-definitiva-mitre', 'Mitre 275', '2392499023', 'Depilación láser de última generación', 'Depilación', 'https://instagram.com/depilacion.mitre', NULL, ARRAY['https://picsum.photos/seed/depilacion9-a/480/320']::text[], 'https://picsum.photos/seed/depilacion9-a/480/320', 'ambos', true, 30, -35.977100, -62.741800, 'demo-9'),
  ('Piel Suave Rivadavia', 'piel-suave-rivadavia', 'Rivadavia 398', '2392400456', 'Cera tibia y depilación tradicional', 'Depilación', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', true, 30, -35.978700, -62.736800, 'demo-10'),
  ('Tinta Negra Tattoo', 'tinta-negra-tattoo', 'Yrigoyen 560', '2392411998', 'Tatuajes personalizados y cover-ups', 'Tatuaje', 'https://instagram.com/tintanegra.tl', NULL, ARRAY['https://picsum.photos/seed/tattoo11-a/480/320', 'https://picsum.photos/seed/tattoo11-b/480/320']::text[], 'https://picsum.photos/seed/tattoo11-a/480/320', 'ambos', true, 90, -35.963900, -62.731800, 'demo-11'),
  ('Estudio La Aguja', 'estudio-la-aguja', 'Castelli 190', '2392422334', 'Tatuajes finos y piercings', 'Tatuaje', NULL, NULL, ARRAY['https://picsum.photos/seed/tattoo12-a/480/320']::text[], 'https://picsum.photos/seed/tattoo12-a/480/320', 'ambos', true, 60, -35.965500, -62.726800, 'demo-12'),
  ('Barbería El Clásico', 'barberia-el-clasico', 'Pellegrini 210', '2392455223', 'Cortes clásicos, fade y afeitado con navaja', 'Barberías', 'https://instagram.com/elclasico.barberia', NULL, ARRAY['https://picsum.photos/seed/barberia13-a/480/320', 'https://picsum.photos/seed/barberia13-b/480/320']::text[], 'https://picsum.photos/seed/barberia13-a/480/320', 'ambos', true, 25, -35.967100, -62.743300, 'demo-13'),
  ('Barber Lounge TL', 'barber-lounge-tl', 'Mitre 88', '2392466012', 'Barbería de autor, ambiente relax', 'Barberías', NULL, NULL, ARRAY['https://picsum.photos/seed/barberia14-a/480/320']::text[], 'https://picsum.photos/seed/barberia14-a/480/320', 'ambos', true, 30, -35.968700, -62.738300, 'demo-14'),
  ('Uñas & Detalles', 'unas-detalles', 'Alsina 305', '2392477341', 'Esculpidas, semipermanente y nail art', 'Manicuria', 'https://instagram.com/unasydetalles', NULL, ARRAY['https://picsum.photos/seed/unas15-a/480/320', 'https://picsum.photos/seed/unas15-b/480/320']::text[], 'https://picsum.photos/seed/unas15-a/480/320', 'ambos', true, 45, -35.970300, -62.733300, 'demo-15'),
  ('Nails Studio TL', 'nails-studio-tl', 'Belgrano 512', '2392488556', 'Manicuría y pedicura spa', 'Manicuria', NULL, NULL, ARRAY['https://picsum.photos/seed/unas16-a/480/320']::text[], 'https://picsum.photos/seed/unas16-a/480/320', 'ambos', true, 50, -35.971900, -62.728300, 'demo-16'),
  ('Podología Dra. Funes', 'podologia-dra-funes', 'San Martín 640', '2392499778', 'Salud y cuidado integral del pie', 'Podología', NULL, NULL, ARRAY['https://picsum.photos/seed/podologia17-a/480/320']::text[], 'https://picsum.photos/seed/podologia17-a/480/320', 'ambos', true, 30, -35.973500, -62.723300, 'demo-17'),
  ('Centro Podológico Trenque', 'centro-podologico-trenque', 'Rivadavia 155', '2392400889', 'Callos, uñas encarnadas y pie diabético', 'Podología', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', true, 30, -35.975100, -62.739800, 'demo-18'),
  ('Estudio Om Yoga', 'estudio-om-yoga', 'Castelli 420', '2392411556', 'Yoga, meditación y respiración consciente', 'Yoga', 'https://instagram.com/om.yoga.tl', NULL, ARRAY['https://picsum.photos/seed/yoga19-a/480/320', 'https://picsum.photos/seed/yoga19-b/480/320']::text[], 'https://picsum.photos/seed/yoga19-a/480/320', 'ambos', true, 60, -35.976700, -62.734800, 'demo-19'),
  ('Pilates Reformer TL', 'pilates-reformer-tl', 'Dorrego 275', '2392422667', 'Pilates con máquinas, grupos reducidos', 'Yoga', NULL, NULL, ARRAY['https://picsum.photos/seed/pilates20-a/480/320']::text[], 'https://picsum.photos/seed/pilates20-a/480/320', 'ambos', true, 50, -35.978300, -62.729800, 'demo-20'),
  ('Nutrición Integral López', 'nutricion-integral-lopez', 'Yrigoyen 330', '2392433778', 'Planes personalizados, deportiva y clínica', 'Nutricionistas', 'https://instagram.com/nutricion.lopez', NULL, ARRAY['https://picsum.photos/seed/nutricion21-a/480/320']::text[], 'https://picsum.photos/seed/nutricion21-a/480/320', 'ambos', true, 40, -35.963500, -62.724800, 'demo-21'),
  ('Consultorio Nutricional Vida', 'consultorio-nutricional-vida', 'Roca 88', '2392444889', 'Nutrición para toda la familia', 'Nutricionistas', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', true, 40, -35.965100, -62.741300, 'demo-22'),
  ('Cosmiatría Piel Sana', 'cosmiatria-piel-sana', 'Sarmiento 512', '2392455990', 'Limpieza facial, peelings y radiofrecuencia', 'Cosmiatría', 'https://instagram.com/pielsana.tl', NULL, ARRAY['https://picsum.photos/seed/cosmiatria23-a/480/320', 'https://picsum.photos/seed/cosmiatria23-b/480/320']::text[], 'https://picsum.photos/seed/cosmiatria23-a/480/320', 'ambos', true, 45, -35.966700, -62.736300, 'demo-23'),
  ('Espacio Cosmético TL', 'espacio-cosmetico-tl', 'Moreno 210', '2392466001', 'Tratamientos faciales personalizados', 'Cosmiatría', NULL, NULL, ARRAY['https://picsum.photos/seed/cosmiatria24-a/480/320']::text[], 'https://picsum.photos/seed/cosmiatria24-a/480/320', 'ambos', true, 45, -35.968300, -62.731300, 'demo-24'),
  ('Kinesiología Trenque', 'kinesiologia-trenque', 'Belgrano 275', '2392433990', 'Rehabilitación deportiva y traumatológica', 'Kinesiólogos', 'https://instagram.com/kinesiologia.tl', NULL, ARRAY['https://picsum.photos/seed/kinesio25-a/480/320', 'https://picsum.photos/seed/kinesio25-b/480/320']::text[], 'https://picsum.photos/seed/kinesio25-a/480/320', 'ambos', true, 45, -35.969900, -62.726300, 'demo-25'),
  ('Fisioterapia Activa', 'fisioterapia-activa', 'Mitre 410', '2392444001', 'Recuperación muscular y articular', 'Kinesiólogos', NULL, NULL, ARRAY['https://picsum.photos/seed/fisio26-a/480/320']::text[], 'https://picsum.photos/seed/fisio26-a/480/320', 'ambos', true, 45, -35.971500, -62.742800, 'demo-26'),
  ('Maquillaje Valentina Ruiz', 'maquillaje-valentina-ruiz', 'Roca 320', '2392455112', 'Maquillaje social, novias y producciones', 'Maquillajes', 'https://instagram.com/valentinaruiz.mkup', NULL, ARRAY['https://picsum.photos/seed/maquillaje27-a/480/320', 'https://picsum.photos/seed/maquillaje27-b/480/320']::text[], 'https://picsum.photos/seed/maquillaje27-a/480/320', 'ambos', true, 60, -35.973100, -62.737800, 'demo-27'),
  ('Glam Studio TL', 'glam-studio-tl', 'San Martín 505', '2392466223', 'Maquillaje profesional para cada ocasión', 'Maquillajes', NULL, NULL, ARRAY['https://picsum.photos/seed/maquillaje28-a/480/320']::text[], 'https://picsum.photos/seed/maquillaje28-a/480/320', 'ambos', true, 45, -35.974700, -62.732800, 'demo-28'),
  ('Dojo Trenque Lauquen', 'dojo-trenque-lauquen', 'Sarmiento 610', '2392477334', 'Karate, taekwondo y defensa personal', 'Artes Marciales', 'https://instagram.com/dojo.tl', NULL, ARRAY['https://picsum.photos/seed/dojo29-a/480/320']::text[], 'https://picsum.photos/seed/dojo29-a/480/320', 'ambos', true, 60, -35.976300, -62.727800, 'demo-29'),
  ('Academia de Boxeo TL', 'academia-de-boxeo-tl', 'Villegas 720', '2392488445', 'Boxeo recreativo y competitivo', 'Artes Marciales', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', true, 60, -35.977900, -62.722800, 'demo-30'),
  ('Reiki y Armonía', 'reiki-y-armonia', 'Alsina 150', '2392499556', 'Reiki, terapias energéticas y meditación guiada', 'Reiki', 'https://instagram.com/reikiarmonia', NULL, ARRAY['https://picsum.photos/seed/reiki31-a/480/320', 'https://picsum.photos/seed/reiki31-b/480/320']::text[], 'https://picsum.photos/seed/reiki31-a/480/320', 'ambos', true, 60, -35.963100, -62.739300, 'demo-31'),
  ('Centro Holístico TL', 'centro-holistico-tl', 'Dorrego 88', '2392400667', 'Terapias alternativas para cuerpo y mente', 'Reiki', NULL, NULL, ARRAY['https://picsum.photos/seed/holistico32-a/480/320']::text[], 'https://picsum.photos/seed/holistico32-a/480/320', 'ambos', true, 60, -35.964700, -62.734300, 'demo-32'),
  ('Bronce Solárium', 'bronce-solarium', 'Yrigoyen 210', '2392411778', 'Camas solares y bronceado sin sol', 'Cosmiatría', NULL, NULL, ARRAY['https://picsum.photos/seed/solarium33-a/480/320']::text[], 'https://picsum.photos/seed/solarium33-a/480/320', 'ambos', true, 20, -35.966300, -62.729300, 'demo-33'),
  ('Sun Studio TL', 'sun-studio-tl', 'Castelli 350', '2392422889', 'Bronceado progresivo y spray tan', 'Cosmiatría', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', true, 20, -35.967900, -62.724300, 'demo-34'),
  ('Almacén Don Pedro', 'almacen-don-pedro', 'Belgrano 145', '2392433001', 'Almacén de barrio, de toda la vida', 'Kioscos', NULL, NULL, ARRAY['https://picsum.photos/seed/almacen35-a/480/320']::text[], 'https://picsum.photos/seed/almacen35-a/480/320', 'ambos', false, NULL, -35.969500, -62.740800, 'demo-35'),
  ('Kiosco 24hs Centro', 'kiosco-24hs-centro', 'San Martín 210', '2392444112', 'Golosinas, bebidas y cigarrillos', 'Kioscos', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.971100, -62.735800, 'demo-36'),
  ('Panadería La Espiga', 'panaderia-la-espiga', 'Roca 88', '2392455223', 'Pan casero, facturas y tortas', 'Panaderías', 'https://instagram.com/laespiga.tl', NULL, ARRAY['https://picsum.photos/seed/panaderia37-a/480/320', 'https://picsum.photos/seed/panaderia37-b/480/320']::text[], 'https://picsum.photos/seed/panaderia37-a/480/320', 'ambos', false, NULL, -35.972700, -62.730800, 'demo-37'),
  ('Panadería y Confitería Sol', 'panaderia-y-confiteria-sol', 'Mitre 320', '2392466334', 'Pan artesanal todos los días', 'Panaderías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.974300, -62.725800, 'demo-38'),
  ('Carnicería El Novillo', 'carniceria-el-novillo', 'Villegas 610', '2392477445', 'Cortes selectos y despacho a domicilio', 'Carnicerías', NULL, NULL, ARRAY['https://picsum.photos/seed/carniceria39-a/480/320']::text[], 'https://picsum.photos/seed/carniceria39-a/480/320', 'ambos', false, NULL, -35.975900, -62.742300, 'demo-39'),
  ('Carnicería Hermanos Ruiz', 'carniceria-hermanos-ruiz', 'Sarmiento 450', '2392488556', 'Carne vacuna, cerdo y pollo', 'Carnicerías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.977500, -62.737300, 'demo-40'),
  ('Verdulería La Huerta', 'verduleria-la-huerta', 'Dorrego 120', '2392499667', 'Frutas y verduras frescas todos los días', 'Verdulerías', NULL, NULL, ARRAY['https://picsum.photos/seed/verduleria41-a/480/320']::text[], 'https://picsum.photos/seed/verduleria41-a/480/320', 'ambos', false, NULL, -35.979100, -62.732300, 'demo-41'),
  ('Frutería El Quinto', 'fruteria-el-quinto', 'Alsina 270', '2392400778', 'Directo del mercado central', 'Verdulerías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.964300, -62.727300, 'demo-42'),
  ('Ferretería Central', 'ferreteria-central', 'San Martín 640', '2392411889', 'Herramientas, pinturas y materiales eléctricos', 'Ferreterías', NULL, NULL, ARRAY['https://picsum.photos/seed/ferreteria43-a/480/320']::text[], 'https://picsum.photos/seed/ferreteria43-a/480/320', 'ambos', false, NULL, -35.965900, -62.743800, 'demo-43'),
  ('Ferretería y Corralón Trenque', 'ferreteria-y-corralon-trenque', 'Yrigoyen 500', '2392422990', 'Todo para la construcción y el hogar', 'Ferreterías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.967500, -62.738800, 'demo-44'),
  ('Librería Estudiantil', 'libreria-estudiantil', 'Castelli 210', '2392433102', 'Útiles escolares y de oficina', 'Librerías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.969100, -62.733800, 'demo-45'),
  ('Papelería Rivadavia', 'papeleria-rivadavia', 'Rivadavia 340', '2392444213', 'Fotocopias, impresiones y librería', 'Librerías', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.970700, -62.728800, 'demo-46'),
  ('Boutique Moda TL', 'boutique-moda-tl', 'Belgrano 480', '2392455324', 'Ropa femenina y accesorios', 'Indumentarias', 'https://instagram.com/boutiquemoda.tl', NULL, ARRAY['https://picsum.photos/seed/boutique47-a/480/320', 'https://picsum.photos/seed/boutique47-b/480/320']::text[], 'https://picsum.photos/seed/boutique47-a/480/320', 'ambos', false, NULL, -35.972300, -62.723800, 'demo-47'),
  ('Calzados Trenque', 'calzados-trenque', 'Roca 275', '2392466435', 'Calzado para toda la familia', 'Indumentarias', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.973900, -62.740300, 'demo-48'),
  ('Café del Centro', 'cafe-del-centro', 'Mitre 150', '2392477546', 'Café de especialidad y pastelería', 'Restaurantes', 'https://instagram.com/cafedelcentro.tl', NULL, ARRAY['https://picsum.photos/seed/cafe49-a/480/320', 'https://picsum.photos/seed/cafe49-b/480/320']::text[], 'https://picsum.photos/seed/cafe49-a/480/320', 'ambos', false, NULL, -35.975500, -62.735300, 'demo-49'),
  ('Parrilla El Fogón', 'parrilla-el-fogon', 'Villegas 390', '2392488657', 'Asados y parrillada para compartir', 'Restaurantes', NULL, NULL, ARRAY['https://picsum.photos/seed/parrilla50-a/480/320']::text[], 'https://picsum.photos/seed/parrilla50-a/480/320', 'ambos', false, NULL, -35.977100, -62.730300, 'demo-50'),
  ('Inmobiliaria Trenque Lauquen', 'inmobiliaria-trenque-lauquen', 'San Martín 410', '2392499768', 'Venta, alquiler y tasaciones', 'Inmobiliarias', 'https://instagram.com/inmobiliaria.tl', NULL, ARRAY['https://picsum.photos/seed/inmobiliaria51-a/480/320']::text[], 'https://picsum.photos/seed/inmobiliaria51-a/480/320', 'ambos', false, NULL, -35.978700, -62.725300, 'demo-51'),
  ('Propiedades del Oeste', 'propiedades-del-oeste', 'Sarmiento 550', '2392400879', 'Tu próxima casa está acá', 'Inmobiliarias', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.963900, -62.741800, 'demo-52'),
  ('Veterinaria San Roque', 'veterinaria-san-roque', 'Dorrego 300', '2392411990', 'Consultas, vacunas y peluquería canina', 'Veterinarias', 'https://instagram.com/vetsanroque', NULL, ARRAY['https://picsum.photos/seed/vet53-a/480/320', 'https://picsum.photos/seed/vet53-b/480/320']::text[], 'https://picsum.photos/seed/vet53-a/480/320', 'ambos', false, NULL, -35.965500, -62.736800, 'demo-53'),
  ('Veterinaria Amigos Fieles', 'veterinaria-amigos-fieles', 'Alsina 420', '2392422001', 'Cuidado integral para tu mascota', 'Veterinarias', NULL, NULL, ARRAY[]::text[], NULL, 'ambos', false, NULL, -35.967100, -62.731800, 'demo-54')
) AS v(nombre, slug, direccion, telefono, slogan, rubro, instagram, facebook, fotos, portada, modo,
       turnos, duracion, lat, lng, origen_id);

INSERT INTO empresa (nombre, slug, direccion, slogan, rubro_id, url_imagen_portada, modo_contacto,
                     turnos_habilitados, duracion_turno_min, lat, lng, origen, origen_id)
SELECT v.nombre, v.slug, v.direccion, v.slogan, r.id, v.portada, v.modo, v.turnos, v.duracion,
       v.lat::numeric, v.lng::numeric, 'propio', v.origen_id
FROM _demo v
JOIN rubro r ON r.nombre = v.rubro
ON CONFLICT (origen, origen_id) DO NOTHING;

INSERT INTO telefono_empresa (empresa_id, numero, principal)
SELECT e.id, v.telefono, true
FROM _demo v JOIN empresa e ON e.origen = 'propio' AND e.origen_id = v.origen_id
WHERE v.telefono IS NOT NULL
ON CONFLICT (empresa_id, numero) DO NOTHING;

INSERT INTO red_social (empresa_id, tipo, url)
SELECT e.id, r.tipo, r.url
FROM _demo v JOIN empresa e ON e.origen = 'propio' AND e.origen_id = v.origen_id
CROSS JOIN LATERAL (VALUES ('instagram', v.instagram), ('facebook', v.facebook)) AS r(tipo, url)
WHERE r.url IS NOT NULL
ON CONFLICT (empresa_id, tipo) DO NOTHING;

INSERT INTO foto_empresa (empresa_id, url, orden)
SELECT e.id, f.url, (f.n - 1)::smallint
FROM _demo v JOIN empresa e ON e.origen = 'propio' AND e.origen_id = v.origen_id
CROSS JOIN LATERAL unnest(v.fotos) WITH ORDINALITY AS f(url, n);

DROP TABLE _demo;

INSERT INTO telefono_empresa (empresa_id, numero, descripcion)
SELECT e.id, t.numero, t.etiqueta
FROM (VALUES
  ('demo-1', '2392411234', 'Recepción'),
  ('demo-1', '2392455001', 'Sofía (color y alisados)'),
  ('demo-2', '2392422987', 'Principal'),
  ('demo-3', '2392433551', 'Recepción'),
  ('demo-3', '2392433552', 'Clases grupales'),
  ('demo-4', '2392444120', 'Principal'),
  ('demo-5', '2392455678', 'Turnos'),
  ('demo-6', '2392466234', 'Principal'),
  ('demo-7', '2392477845', 'Principal'),
  ('demo-8', '2392488112', 'Principal'),
  ('demo-9', '2392499023', 'Principal'),
  ('demo-10', '2392400456', 'Principal'),
  ('demo-11', '2392411998', 'Principal'),
  ('demo-12', '2392422334', 'Principal'),
  ('demo-13', '2392455223', 'Recepción'),
  ('demo-13', '2392455224', 'Turnos WhatsApp'),
  ('demo-14', '2392466012', 'Principal'),
  ('demo-15', '2392477341', 'Principal'),
  ('demo-16', '2392488556', 'Principal'),
  ('demo-17', '2392499778', 'Principal'),
  ('demo-18', '2392400889', 'Principal'),
  ('demo-19', '2392411556', 'Recepción'),
  ('demo-19', '2392411557', 'Clases grupales'),
  ('demo-20', '2392422667', 'Principal'),
  ('demo-21', '2392433778', 'Principal'),
  ('demo-22', '2392444889', 'Principal'),
  ('demo-23', '2392455990', 'Principal'),
  ('demo-24', '2392466001', 'Principal'),
  ('demo-25', '2392433990', 'Principal'),
  ('demo-26', '2392444001', 'Principal'),
  ('demo-27', '2392455112', 'Principal'),
  ('demo-28', '2392466223', 'Principal'),
  ('demo-29', '2392477334', 'Recepción'),
  ('demo-29', '2392477335', 'Profesor Diego'),
  ('demo-30', '2392488445', 'Principal'),
  ('demo-31', '2392499556', 'Principal'),
  ('demo-32', '2392400667', 'Principal'),
  ('demo-33', '2392411778', 'Principal'),
  ('demo-34', '2392422889', 'Principal'),
  ('demo-35', '2392433001', 'Local'),
  ('demo-35', '2392433002', 'Pedidos por WhatsApp'),
  ('demo-36', '2392444112', 'Principal'),
  ('demo-37', '2392455223', 'Principal'),
  ('demo-38', '2392466334', 'Principal'),
  ('demo-39', '2392477445', 'Principal'),
  ('demo-40', '2392488556', 'Principal'),
  ('demo-41', '2392499667', 'Principal'),
  ('demo-42', '2392400778', 'Principal'),
  ('demo-43', '2392411889', 'Principal'),
  ('demo-44', '2392422990', 'Principal'),
  ('demo-45', '2392433102', 'Principal'),
  ('demo-46', '2392444213', 'Principal'),
  ('demo-47', '2392455324', 'Principal'),
  ('demo-48', '2392466435', 'Principal'),
  ('demo-49', '2392477546', 'Principal'),
  ('demo-50', '2392488657', 'Principal'),
  ('demo-51', '2392499768', 'Principal'),
  ('demo-52', '2392400879', 'Principal'),
  ('demo-53', '2392411990', 'Principal'),
  ('demo-54', '2392422001', 'Principal')
) AS t(origen_id, numero, etiqueta)
JOIN empresa e ON e.origen = 'propio' AND e.origen_id = t.origen_id
ON CONFLICT (empresa_id, numero) DO UPDATE SET descripcion = EXCLUDED.descripcion;
