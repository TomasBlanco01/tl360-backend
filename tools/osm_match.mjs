// Cruza los POIs de OpenStreetMap (data/osm_pois.json, vía Overpass) con los comercios por nombre y genera db/seed/05_geo_osm.sql.
import fs from 'fs';
const listado = JSON.parse(fs.readFileSync('data/empresas_listado.json', 'utf8'));
const osm = JSON.parse(fs.readFileSync('data/osm_pois.json', 'utf8')).elements;
const norm = (s) => (s || '').toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '').replace(/[^a-z0-9 ]/g, ' ').replace(/\b(el|la|los|las|de|del|y|sa|srl)\b/g, ' ').replace(/\s+/g, ' ').trim();
const filas = []; const sinMatch = [];
for (const p of osm) {
	const lat = p.lat ?? p.center?.lat, lng = p.lon ?? p.center?.lon;
	const n = norm(p.tags.name);
	if (!lat || n.length < 4) continue;
	const hits = listado.filter((e) => { const m = norm(e.nombre); return m === n || (n.length >= 6 && m.includes(n)) || (m.length >= 6 && n.includes(m)); });
	if (!hits.length) { sinMatch.push(p.tags.name); continue; }
	for (const e of hits) filas.push(`  ('todotrenque', '${e.origen_id}', ${(+lat).toFixed(6)}, ${(+lng).toFixed(6)})`);
}
const sql = `-- Generado por tools/osm_match.mjs. Coordenadas © colaboradores de OpenStreetMap (ODbL), vía Overpass; cruce por nombre.
UPDATE empresas e SET lat = v.lat, lng = v.lng
FROM (VALUES
${filas.join(',\n')}
) AS v(origen, origen_id, lat, lng)
WHERE e.origen = v.origen AND e.origen_id = v.origen_id AND e.lat IS NULL;
`;
if (filas.length) fs.writeFileSync('../db/seed/05_geo_osm.sql', sql);
console.log('POIs OSM:', osm.length, '· coincidencias con comercios:', filas.length, '· sin coincidencia:', sinMatch.length);
console.log(sinMatch.slice(0, 40));
