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

// Súper rubros: agrupación por palabras clave (heurística, revisable). Gana la primera que matchea.
const GRUPOS = [
  ["Salud, belleza y bienestar", /m[eé]dic|salud|cardi|alerg|anestes|bioqu|cirujan|dermat|diabet|diagn[oó]st|endocrin|enfermer|flebol|fonoaud|gastroent|geri[aá]tr|ginecol|hematol|infectol|kinesi|laborator|maternidad|nefrol|neonat|neumon|neurocir|nutricion|obstetric|odont|oftalm|oncol|[oó]ptic|ortoped|osteop|otorrino|parteras|patólog|pediatr|pedicur|podolog|psic[oó]|psiquiatr|quiroprax|radi[oó]log|reumat|sexolog|terap|tocoginec|traumat|ur[oó]log|acompa[nñ]ante|masaj|masoterap|reiki|herborister|naturist|diet[eé]tic|cosmiatr|est[eé]tica$|depilaci|manicur|peluquer[ií]as$|barber|maquillaj|tatuaj|yoga|pilates|farmacia/i],
  ["Gastronomía", /gastron|^bares?$|barra de tragos|cafeter|carnicer|casas de comida|catering|cervecer|chacinad|delivery|fiambrer|hamburgueser|helader[ií]|lunch|panader|pancher|parrilla|^pastas$|pasteler|pollos|quesos|reposter|restaur|resto bar|rotiser|sandwich|tortas|verduler|viandas|vinos|bebidas|condimentos|agua minerales|almac[eé]n|autoservicio|supermercado|kiosco|l[aá]cteos|^miel$|pescader|pizzer|panchos|equipamiento gastron/i],
  ["Automotores y transporte", /automotor|agencias? (de )?(auto|moto)|alineaci|autopartes|auxilios|balanceo|bater[ií]as|camionero|chapist|ciclomotor|combis|combustible|embrague|escapes|estaciones de servicio|frenos|gomer[ií]|gr[uú]as|lavaderos de autos|lubricentro|mec[aá]nicos|micros|motocicleta|neum[aá]tic|parabrisas|pintores de autos|polarizado|radiadores|remises|repuestos|talleres mec|transportes|traslados|veh[ií]culos|fletes|expresos|mensajer|rectificaci|cocheras|alquiler de autos|n[aá]utica|karting|autoelevadores|^motores$|^talleres$/i],
  ["Construcción y hogar", /aberturas|albañil|alambrador|aluminios|amoblamient|arenados|arquitect|carpinter|construcci|constructor|corralones|cortinas|durlock|electricidad|electricista|excavaci|ferreter|galpones|gasistas|herrajes|herrer|hidr[aá]ulica|hormig[oó]n|ingenier|jardiner|maderas|maestro mayor|mamparas|materiales|metal[uú]rgic|mosaicos|movimientos de suelo|muebler|pinturer|^pintores$|pisos|plomeros|poceros|premoldeados|revestimiento|sanitarios|techistas|terrenos|tinglados|torner|afilados|vidrier|yesistas|zinguer|agrimensor|volquetes|containers|inmobiliaria|martilleros|viviendas|limpieza de terrenos|estructuras met|piletas|calefones|aire acondic|refrigeraci|lavarropas|electrodom[eé]st|servicio t[eé]c|servicio tec|alarmas|c[aá]maras de seguridad|cerrajer|automatizaci[oó]n de portones|energ[ií]a solar|grupos electr|instalaciones comerciales|decoraciones|tapicer|blanquer|bazar|art[ií]culos para el hogar|leña|gas insumos|gases|nitr[oó]geno|matafuegos|extintores|bobinados|electromec|bombas|caños|santer|oficios/i],
  ["Campo y agro", /campos|agr[ií]cola|agricultura|agro|aguadas|alimentos balanceados|api[cs]ultor|av[ií]colas|casillas rurales|cereales|consignatarios|forrajer|hacienda|molinero|monitores de siembra|pulverizaci|quintas|semillas|tractores|maquinarias|viveros|fumigaci|control de plagas|lagunas|guia de pesca|^pesca$/i],
  ["Educación y cultura", /^clases$|colegios|colonia|cursos|escuelas|guarder[ií]as|institutos|ingles|jardines|profesores|psicopedag|universidades|museos|librer|iglesias|ludotecas|^estudios$|traductor|radios|filmaci|fot[oó]grafos|cantantes|disc jockey|rocker|danza|baile|disquer|pel[ií]culas|pintores de cuadros/i],
  ["Deportes, turismo y eventos", /deportes|artes marciales|boxeo|canchas|carreras|clubes|gimnasio|hockey|karate|nataci[oó]n|p[aá]del|turismo|hoteles|hospedaje|alojamientos|caba[nñ]as|residencias|agencias de turismo|cotill[oó]n|disfraces|inflables|peloteros|animaciones|eventos|organizaci[oó]n de evento|salones|sonido|alquiler de (salones|vajillas)|pantallas led|trofeos|alquiler herramientas|alquileres temporarios/i],
  ["Servicios profesionales", /abogados|contadores|escriban|gestor[ií]as|mandatarios|seguros|financieras|consultora|recursos humanos|profesionales|higiene y seguridad|comisionistas|marketing|community|dise[nñ]o|publicidad|imprentas|gr[aá]ficas|carteles|serigraf|estampados|fotocopiadoras|impresiones|inform[aá]tica|computaci[oó]n|internet|drones|electr[oó]nica|celulares|directv|departamentos judiciales|juzgados|organismos p|municipalidad|sindicatos|mutuales|obra sociales|bancos|sucursales de cobro|tr[aá]mites|filiales|agencia de loter|quiniela|compra venta|emergencias|seguridad/i],
  ["Comercios y tiendas", /calzado|indumentaria|jugueter|bijouter|bolsas|bolsos|cuchiller|joyer|lanas|marroquiner|mercer|modistas|perfumer|regaler|relojer|telas|talabart|zapatero|confecci|costura|el[aá]sticos|antig[uü]edades|artesan[ií]as|florer[ií]as|papeleras|descartables|distribuidoras|herramientas|buloneras|bicicleter|mascotas|veterinaria|est[eé]ticas mascotas|peluquer[ií]as canina|tabaquer|armer[ií]as|art[ií]culos de limpieza|lavaderos|^limpieza$|servicios de limpieza|tarot|lonas|pa[nñ]aleras/i],
];
const OTROS = "Otros servicios";
const grupoDe = (nombre) => (GRUPOS.find(([, re]) => re.test(nombre)) || [OTROS])[0];

const esc = (s) => String(s).replace(/[­​]/g, "").replace(/'/g, "''");
const q =(v) => (v === null || v === undefined || v === "" ? "NULL" : `'${esc(v)}'`);
const slugify = (s) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

// ---------------- 01_rubros.sql
const cats = rubros.filter((r) => !EXCLUIR.has(r.slug) && !EXCLUIR_NOMBRE.has(r.nombre)).map((r) => ({ ...r, grupo: grupoDe(r.nombre) }));
const grupos = [...new Set([...GRUPOS.map((g) => g[0]), OTROS])];
let s = "-- Generado por tools/gen_seed.js. Rubros de referencia (nombres genéricos), uso académico.\n";
s += "INSERT INTO super_rubros (nombre, slug) VALUES\n" + grupos.map((g) => `  ('${esc(g)}', '${slugify(g)}')`).join(",\n") + "\nON CONFLICT (nombre) DO NOTHING;\n\n";
s += "INSERT INTO categorias (nombre, slug, super_rubro_id)\nSELECT v.nombre, v.slug, sr.id FROM (VALUES\n";
s += cats.map((c) => `  ('${esc(c.nombre)}', '${esc(c.slug)}', '${esc(c.grupo)}')`).join(",\n");
s += "\n) AS v(nombre, slug, grupo) JOIN super_rubros sr ON sr.nombre = v.grupo\nON CONFLICT (nombre) DO UPDATE SET slug = EXCLUDED.slug, super_rubro_id = EXCLUDED.super_rubro_id;\n";
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
    direccion: dir,
    cp: d.codigo_postal || null,
    telefono: e.telefonos[0] ? e.telefonos[0].numero : null,
    descripcion: d.descripcion || e.descripcion_corta || null,
    fotos: d.fotos || [],
    perfil: e.imagen_perfil,
    portada: e.imagen_portada,
    nivel: e.nivel,
    rubros: rubs,
    telefonos: e.telefonos,
  });
}

let t = "-- Generado por tools/gen_seed.js. Comercios importados de la guía de origen (uso académico, no publicar).\n";
t += "-- Se cargan como fichas sin dueño (admin_id NULL), sin turnos habilitados y sin verificar.\n\n";
const CH = 300;
for (let i = 0; i < empresas.length; i += CH) {
  const chunk = empresas.slice(i, i + CH);
  t += "INSERT INTO empresas (nombre, slug, direccion, codigo_postal, telefono, descripcion, fotos, imagen_perfil, imagen_portada, categoria_id, turnos_habilitados, origen, origen_id, origen_url, nivel_origen)\n";
  t += "SELECT v.nombre, v.slug, v.direccion, v.cp, v.telefono, v.descripcion, v.fotos, v.perfil, v.portada, c.id, false, 'todotrenque', v.origen_id, v.url, v.nivel FROM (VALUES\n";
  t += chunk.map((e) => {
    const fotos = e.fotos.length ? `ARRAY[${e.fotos.map((f) => `'${esc(f)}'`).join(",")}]::text[]` : "'{}'::text[]";
    return `  (${q(e.origen_id)}, ${q(e.slug)}, ${q(e.nombre)}, ${q(e.direccion)}, ${q(e.cp)}, ${q(e.telefono)}, ${q(e.descripcion)}, ${fotos}, ${q(e.perfil)}, ${q(e.portada)}, ${q(e.nivel)}, ${q(e.rubros[0])}, ${q("https://www.todotrenque.com.ar/" + e.slug + ".php")})`;
  }).join(",\n");
  t += "\n) AS v(origen_id, slug, nombre, direccion, cp, telefono, descripcion, fotos, perfil, portada, nivel, rubro_slug, url)\n";
  t += "JOIN categorias c ON c.slug = v.rubro_slug\nON CONFLICT (origen, origen_id) DO NOTHING;\n\n";
}

const rels = empresas.flatMap((e) => e.rubros.map((r) => [e.slug, r]));
for (let i = 0; i < rels.length; i += 1000) {
  t += "INSERT INTO empresa_categorias (empresa_id, categoria_id)\nSELECT e.id, c.id FROM (VALUES\n";
  t += rels.slice(i, i + 1000).map(([e, r]) => `  ('${esc(e)}', '${esc(r)}')`).join(",\n");
  t += "\n) AS v(empresa_slug, rubro_slug) JOIN empresas e ON e.slug = v.empresa_slug JOIN categorias c ON c.slug = v.rubro_slug\nON CONFLICT DO NOTHING;\n\n";
}

const telMap = new Map();
for (const e of empresas) {
  for (const x of e.telefonos) {
    if (!x.numero) continue;
    const k = e.slug + "|" + x.numero;
    telMap.set(k, [e.slug, x.numero, (telMap.get(k)?.[2] || false) || x.whatsapp]);
  }
}
const tels = [...telMap.values()];
for (let i = 0; i < tels.length; i += 1000) {
  t += "INSERT INTO telefonos_empresa (empresa_id, numero, es_whatsapp)\nSELECT e.id, v.numero, v.wa FROM (VALUES\n";
  t += tels.slice(i, i + 1000).map(([e, n, w]) => `  ('${esc(e)}', '${esc(n)}', ${w})`).join(",\n");
  t += "\n) AS v(empresa_slug, numero, wa) JOIN empresas e ON e.slug = v.empresa_slug\nON CONFLICT (empresa_id, numero) DO UPDATE SET es_whatsapp = telefonos_empresa.es_whatsapp OR EXCLUDED.es_whatsapp;\n\n";
}
fs.writeFileSync(path.join(OUT, "02_empresas.sql"), t);

const conteo = {};
for (const c of cats) conteo[c.grupo] = (conteo[c.grupo] || 0) + 1;
console.log({ categorias: cats.length, empresas: empresas.length, sinRubroValido: sinRubro, relaciones: rels.length, telefonos: tels.length });
console.log(conteo);
console.log("Otros:", cats.filter((c) => c.grupo === OTROS).map((c) => c.nombre).join(", "));
