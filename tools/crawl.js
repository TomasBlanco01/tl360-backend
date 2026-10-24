// Extracción de la guía (uso académico, local). Reanudable: guarda en tools/data/.
// Uso: node crawl.js listings   → recorre cada rubro y arma empresas + relación con rubros
//      node crawl.js details    → visita la ficha de cada empresa (CP, localidad, fotos, descripción completa)
const fs = require("fs");
const path = require("path");
const cheerio = require("cheerio");

const BASE = "https://www.todotrenque.com.ar";
const DATA = path.join(__dirname, "data");
fs.mkdirSync(DATA, { recursive: true });
const DELAY_MS = 250;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function get(url, tries = 3) {
  for (let i = 0; i < tries; i++) {
    try {
      const r = await fetch(url, { headers: { "User-Agent": "Mozilla/5.0 (proyecto academico UTN)" } });
      if (r.status === 404) return null;
      if (r.ok) return await r.text();
    } catch (_) {}
    await sleep(1000 * (i + 1));
  }
  return null;
}

const clean = (s) => (s || "").replace(/\s+/g, " ").trim();

function parseCards($, slugRubro) {
  const out = [];
  $(".usuario-premium, .usuario-pago, .usuario-gratis").each((_, el) => {
    const $el = $(el);
    const nivel = el.attribs.class.replace("usuario-", "");
    const a = $el.find("h3 a").first();
    const href = a.attr("href") || "";
    const slug = href.replace(/^.*\//, "").replace(".php", "");
    const uid = $el.find("[data-usuario-id]").first().attr("data-usuario-id") || null;
    const telefonos = [];
    $el.find(".lista-contacto li").each((_, li) => {
      const num = clean($(li).find(".numero-negocio").text());
      if (!num) return;
      telefonos.push({
        numero: num.replace(/\D/g, ""),
        whatsapp: $(li).find("a.wa").length > 0,
        llamada: $(li).find("a.tel").length > 0,
      });
    });
    out.push({
      origen_id: uid,
      slug,
      nombre: clean(a.text()),
      nivel,
      direccion: clean($el.find(".direccion-item").text()) || null,
      descripcion_corta: clean($el.find(".descripcion-corta").text()) || null,
      imagen_perfil: $el.find(".mini-perfil img").attr("src") || null,
      imagen_portada: $el.find(".foto-tarjeta img").attr("src") || null,
      telefonos,
      rubro: slugRubro,
    });
  });
  return out;
}

async function listings() {
  const home = await get(BASE + "/");
  const $h = cheerio.load(home);
  const rubros = new Map();
  $h('a[href*="/rubros/"]').each((_, a) => {
    const slug = ($h(a).attr("href").split("/rubros/")[1] || "").replace(".php", "");
    const nombre = clean($h(a).text());
    if (slug && (!rubros.has(slug) || nombre.length > rubros.get(slug).length)) rubros.set(slug, nombre);
  });
  fs.writeFileSync(path.join(DATA, "rubros.json"), JSON.stringify([...rubros].map(([slug, nombre]) => ({ slug, nombre })), null, 1));
  console.log("rubros:", rubros.size);

  const empresas = new Map(); // key: slug
  const rels = [];
  let i = 0;
  for (const [slug, nombre] of rubros) {
    i++;
    const html = await get(`${BASE}/rubros/${slug}.php`);
    await sleep(DELAY_MS);
    if (!html) { console.log("sin página:", slug); continue; }
    const $ = cheerio.load(html);
    const cards = parseCards($, slug);
    for (const c of cards) {
      if (!c.slug) continue;
      if (!empresas.has(c.slug)) empresas.set(c.slug, { ...c, rubros: [] });
      const e = empresas.get(c.slug);
      if (!e.rubros.includes(slug)) e.rubros.push(slug);
      // completar campos si otra aparición trae más datos
      for (const k of ["direccion", "descripcion_corta", "imagen_perfil", "imagen_portada", "origen_id"]) if (!e[k] && c[k]) e[k] = c[k];
      if (c.telefonos.length > e.telefonos.length) e.telefonos = c.telefonos;
    }
    if (i % 25 === 0) console.log(`${i}/${rubros.size} rubros, ${empresas.size} empresas`);
  }
  for (const e of empresas.values()) delete e.rubro;
  fs.writeFileSync(path.join(DATA, "empresas_listado.json"), JSON.stringify([...empresas.values()], null, 1));
  console.log("empresas únicas:", empresas.size);
}

async function detalleDe(e) {
  const html = await get(`${BASE}/${e.slug}.php`);
  await sleep(DELAY_MS);
  if (!html) return { error: '404' };
  const $ = cheerio.load(html);
  const body = $("main").length ? $("main") : $("body");
  const text = clean(body.text());
  const cp = (text.match(/CP:\s*(\d{4})/) || [])[1] || null;
  const fotos = [];
  body.find('a[href*="fotos.php"]').each((_, a) => {
    const img = $(a).find("img").attr("src");
    if (img) fotos.push(img);
  });
  const descr = clean(body.find("p").filter((_, p) => clean($(p).text()).length > 60).first().text()) || null;
  return {
    codigo_postal: cp,
    direccion: clean(text.match(/Direcci[oó]n\s+(.*?)\s+(Celular|Tel[eé]fono|Fijo)/)?.[1]) || null,
    descripcion: descr,
    fotos: [...new Set(fotos)],
  };
}

async function details() {
  const lista = JSON.parse(fs.readFileSync(path.join(DATA, 'empresas_listado.json'), 'utf8'));
  const file = path.join(DATA, 'empresas_detalle.json');
  const done = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : {};
  const pendientes = lista.filter((e) => !done[e.slug]);
  console.log('pendientes:', pendientes.length);
  let idx = 0, n = 0;
  const worker = async () => {
    while (idx < pendientes.length) {
      const e = pendientes[idx++];
      done[e.slug] = await detalleDe(e);
      if (++n % 50 === 0) {
        fs.writeFileSync(file, JSON.stringify(done));
        console.log(Object.keys(done).length + '/' + lista.length);
      }
    }
  };
  await Promise.all([worker(), worker(), worker()]);
  fs.writeFileSync(file, JSON.stringify(done, null, 1));
  console.log('detalles:', Object.keys(done).length);
}

const mode = process.argv[2];
(mode === "listings" ? listings() : mode === "details" ? details() : Promise.reject("uso: listings|details")).catch((e) => { console.error(e); process.exit(1); });
