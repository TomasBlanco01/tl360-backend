// Geocodifica las direcciones de los comercios con Nominatim (OpenStreetMap, ODbL) y genera db/seed/04_geo.sql.
// Uso: node geocode.mjs   (1 consulta/seg según la política de uso; cachea en data/geocache.json y se puede reanudar)
import fs from 'fs';

const listado = JSON.parse(fs.readFileSync('data/empresas_listado.json', 'utf8'));
const CACHE = 'data/geocache.json';
const cache = fs.existsSync(CACHE) ? JSON.parse(fs.readFileSync(CACHE, 'utf8')) : {};
// Trenque Lauquen ≈ (-35.97, -62.73); caja que cubre el partido, para descartar homónimos de otras ciudades.
const BBOX = { minLat: -36.3, maxLat: -35.7, minLng: -63.1, maxLng: -62.4 };

const limpiar = (d) =>
	(d || '')
		.replace(/\s*-\s*trenque lauquen.*$/i, '')
		.replace(/\s*-\s*$/, '')
		.replace(/\s+/g, ' ')
		.trim();
const valida = (d) => d.length >= 4 && d.length <= 60 && /\d/.test(d) && !/direcci[oó]n/i.test(d);

const porDireccion = new Map();
for (const e of listado) {
	const d = limpiar(e.direccion);
	if (!valida(d)) continue;
	if (!porDireccion.has(d.toLowerCase())) porDireccion.set(d.toLowerCase(), d);
}
console.log('direcciones únicas a geocodificar:', porDireccion.size);

const dentro = (lat, lng) => lat >= BBOX.minLat && lat <= BBOX.maxLat && lng >= BBOX.minLng && lng <= BBOX.maxLng;
const pausa = (ms) => new Promise((r) => setTimeout(r, ms));

async function consultar(q) {
	const url = `https://nominatim.openstreetmap.org/search?format=json&limit=1&countrycodes=ar&viewbox=${BBOX.minLng},${BBOX.maxLat},${BBOX.maxLng},${BBOX.minLat}&bounded=1&q=${encodeURIComponent(q + ', Trenque Lauquen, Buenos Aires, Argentina')}`;
	for (let intento = 0; intento < 3; intento++) {
		try {
			const r = await fetch(url, { headers: { 'User-Agent': 'TL360-estudio/1.0 (trabajo practico UTN FRTL)' } });
			if (r.status === 429 || r.status >= 500) { await pausa(5000); continue; }
			const j = await r.json();
			if (!j.length) return null;
			const lat = parseFloat(j[0].lat), lng = parseFloat(j[0].lon);
			return dentro(lat, lng) ? { lat, lng, tipo: j[0].type } : null;
		} catch { await pausa(3000); }
	}
	return null;
}

let n = 0;
for (const [clave, dir] of porDireccion) {
	if (clave in cache) continue;
	cache[clave] = await consultar(dir);
	if (++n % 25 === 0) { fs.writeFileSync(CACHE, JSON.stringify(cache)); console.log(n, '/', porDireccion.size); }
	await pausa(1100);
}
fs.writeFileSync(CACHE, JSON.stringify(cache));

const filas = [];
for (const e of listado) {
	const g = cache[limpiar(e.direccion).toLowerCase()];
	if (g) filas.push(`  ('todotrenque', '${e.origen_id}', ${g.lat.toFixed(6)}, ${g.lng.toFixed(6)})`);
}
const sql = `-- Generado por tools/geocode.mjs. Coordenadas © colaboradores de OpenStreetMap (ODbL), obtenidas con Nominatim.
UPDATE empresas e SET lat = v.lat, lng = v.lng
FROM (VALUES
${filas.join(',\n')}
) AS v(origen, origen_id, lat, lng)
WHERE e.origen = v.origen AND e.origen_id = v.origen_id AND e.lat IS NULL;
`;
fs.writeFileSync('../db/seed/04_geo.sql', sql);
const ok = Object.values(cache).filter(Boolean).length;
console.log(`geocodificadas ${ok}/${porDireccion.size} direcciones → ${filas.length} comercios`);
