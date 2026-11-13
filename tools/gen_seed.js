// Genera db/seed/01_rubros.sql y db/seed/02_empresas.sql a partir de tools/data/*.json
// Uso: node gen_seed.js
const fs = require("fs");
const path = require("path");

const DATA = path.join(__dirname, "data");
const OUT = path.join(__dirname, "..", "db", "seed");
fs.mkdirSync(OUT, { recursive: true });

const rubros = JSON.parse(fs.readFileSync(path.join(DATA, "rubros.json"), "utf8"));
const listado = JSON.parse(fs.readFileSync(path.join(DATA, "empresas_listado.json"), "utf8"));
const detFile = path.join(DATA, "empresas_detalle.json");
const detalle = fs.existsSync(detFile) ? JSON.parse(fs.readFileSync(detFile, "utf8")) : {};

// No son rubros comerciales (fechas, utilidades, listados internos del sitio de origen).
const EXCLUIR = new Set(["barrios", "clasificados", "clima", "descuentos", "dia-de-la-madre", "dia-del-nino", "telefonos-utiles"]);
const EXCLUIR_NOMBRE = new Set(["Barrios", "Clasificados", "Clima", "Descuentos", "Día de la Madre", "Día del Niño", "Teléfonos Útiles"]);

const esc = (s) => String(s).replace(/[­​]/g, "").replace(/'/g, "''");
const q = (v) => (v === null || v === undefined || v === "" ? "NULL" : `'${esc(v)}'`);

// ---------------- 01_rubros.sql
const cats = rubros.filter((r) => !EXCLUIR.has(r.slug) && !EXCLUIR_NOMBRE.has(r.nombre));
let s = "-- Generado por tools/gen_seed.js. Rubros de referencia (nombres genéricos), uso académico.\n";
s += "INSERT INTO rubro (nombre, slug) VALUES\n";
s += cats.map((c) => `  ('${esc(c.nombre)}', '${esc(c.slug)}')`).join(",\n");
s += "\nON CONFLICT (nombre) DO UPDATE SET slug = EXCLUDED.slug;\n";
fs.writeFileSync(path.join(OUT, "01_rubros.sql"), s);

// ---------------- 02_empresas.sql
const catValidas = new Set(cats.map((c) => c.slug));
const empresas = [];
let sinRubro = 0;
for (const e of listado) {
  const rubs = e.rubros.filter((r) => catValidas.has(r));
  if (!rubs.length) { sinRubro++; continue; }
  const d = detalle[e.slug] && !detalle[e.slug].error ? detalle[e.slug] : {};
  const dir = d.direccion || e.direccion || "Trenque Lauquen";
  empresas.push({
    origen_id: e.origen_id || e.slug,
    slug: e.slug,
    nombre: e.nombre,
    // Algunas direcciones de la guía vienen con el texto de la ficha pegado: VARCHAR(150).
    direccion: dir.slice(0, 150),
    cp: d.codigo_postal || null,
    descripcion: (d.descripcion || e.descripcion_corta || "").slice(0, 2000) || null,
    fotos: (d.fotos || []).filter(Boolean),
    perfil: e.imagen_perfil,
    portada: e.imagen_portada,
    // Un comercio tiene un solo rubro: el primero que publica la guía.
    rubro: rubs[0],
    telefonos: e.telefonos,
  });
}

let t = "-- Generado por tools/gen_seed.js. Comercios importados de la guía de origen (uso académico, no publicar).\n";
t += "-- Se cargan como fichas sin dueño (usuario_id NULL), sin turnos habilitados y sin verificar.\n\n";
const CH = 300;
for (let i = 0; i < empresas.length; i += CH) {
  const chunk = empresas.slice(i, i + CH);
  t += "INSERT INTO empresa (nombre, slug, direccion, codigo_postal, descripcion, url_imagen_perfil, url_imagen_portada, rubro_id, turnos_habilitados, origen, origen_id)\n";
  t += "SELECT v.nombre, v.slug, v.direccion, v.cp, v.descripcion, v.perfil, v.portada, r.id, false, 'todotrenque', v.origen_id FROM (VALUES\n";
  t += chunk.map((e) => `  (${q(e.origen_id)}, ${q(e.slug)}, ${q(e.nombre)}, ${q(e.direccion)}, ${q(e.cp)}, ${q(e.descripcion)}, ${q(e.perfil)}, ${q(e.portada)}, ${q(e.rubro)})`).join(",\n");
  t += "\n) AS v(origen_id, slug, nombre, direccion, cp, descripcion, perfil, portada, rubro_slug)\n";
  t += "JOIN rubro r ON r.slug = v.rubro_slug\nON CONFLICT (origen, origen_id) DO NOTHING;\n\n";
}

const fotos = empresas.flatMap((e) => e.fotos.map((f, i) => [e.slug, f, i]));
for (let i = 0; i < fotos.length; i += 1000) {
  t += "INSERT INTO foto_empresa (empresa_id, url, orden)\nSELECT e.id, v.url, v.orden FROM (VALUES\n";
  t += fotos.slice(i, i + 1000).map(([e, f, o]) => `  ('${esc(e)}', '${esc(f)}', ${o})`).join(",\n");
  t += "\n) AS v(empresa_slug, url, orden) JOIN empresa e ON e.slug = v.empresa_slug;\n\n";
}

// El primer número de la guía es el principal (el que se edita en el formulario de la ficha).
const telMap = new Map();
for (const e of empresas) {
  e.telefonos.forEach((x, i) => {
    if (!x.numero) return;
    const numero = x.numero.slice(0, 20);
    const k = e.slug + "|" + numero;
    const previo = telMap.get(k);
    telMap.set(k, [e.slug, numero, (previo?.[2] || false) || x.whatsapp, previo?.[3] ?? i === 0]);
  });
}
const tels = [...telMap.values()];
for (let i = 0; i < tels.length; i += 1000) {
  t += "INSERT INTO telefono_empresa (empresa_id, numero, es_whatsapp, principal)\nSELECT e.id, v.numero, v.wa, v.principal FROM (VALUES\n";
  t += tels.slice(i, i + 1000).map(([e, n, w, p]) => `  ('${esc(e)}', '${esc(n)}', ${w}, ${p})`).join(",\n");
  t += "\n) AS v(empresa_slug, numero, wa, principal) JOIN empresa e ON e.slug = v.empresa_slug\nON CONFLICT (empresa_id, numero) DO UPDATE SET es_whatsapp = telefono_empresa.es_whatsapp OR EXCLUDED.es_whatsapp;\n\n";
}
fs.writeFileSync(path.join(OUT, "02_empresas.sql"), t);

console.log({ rubros: cats.length, empresas: empresas.length, sinRubroValido: sinRubro, fotos: fotos.length, telefonos: tels.length });
