// Prueba de humo de punta a punta contra un backend corriendo (por defecto http://127.0.0.1:3001).
// Requiere la base con los seeds cargados (npm run db). Uso: npm run smoke   o   API=http://... node smoke.mjs
// No supone un tamaño de catálogo: funciona igual con los comercios importados (02_empresas.sql) que
// con los 54 de demostración (02_empresas_demo.sql), y elige los datos de prueba según lo que haya.
const B = (process.env.API ?? "http://127.0.0.1:3001") + "/api";
let fallos = 0;

async function call(method, path, { token, body } = {}) {
  const res = await fetch(B + path, {
    method,
    headers: { ...(body ? { "Content-Type": "application/json" } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  let data = null;
  try { data = await res.json(); } catch {}
  return { status: res.status, data };
}
function check(nombre, cond, extra = "") {
  console.log(`${cond ? "OK  " : "FALLA"} ${nombre}${extra ? "  → " + extra : ""}`);
  if (!cond) fallos++;
}
/** Sin los datos mínimos no tiene sentido seguir: se corta con un mensaje claro. */
function exigir(cond, motivo) {
  if (!cond) {
    console.error(`\nNo se puede seguir: ${motivo}`);
    process.exit(1);
  }
}
const login = async (email, pass = "Demo1234!") => (await call("POST", "/auth/login", { body: { email, contraseña: pass } }));

let cli, neg, adm;
try {
  [cli, neg, adm] = await Promise.all([login("cliente@demo.com"), login("negocio@demo.com"), login("admin@demo.com")]);
} catch (e) {
  exigir(false, `el backend no responde en ${B} (${e.cause?.code ?? e.message}). Levantalo con cargo run.`);
}
check("login cliente/negocio/admin", cli.status === 200 && neg.status === 200 && adm.status === 200);
exigir(cli.status === 200 && neg.status === 200 && adm.status === 200, "faltan las cuentas de demostración (¿se cargaron los seeds?)");
check("login con clave incorrecta → 401", (await login("cliente@demo.com", "mala")).status === 401);
check("login sin distinguir mayúsculas en el email", (await login("CLIENTE@demo.com")).status === 200);
const TC = cli.data.token, TN = neg.data.token, TA = adm.data.token;

// ------------------------------------------------------------------ catálogo
const lista = await call("GET", "/empresas");
check("GET /empresas devuelve el catálogo", Array.isArray(lista.data) && lista.data.length > 0, `${lista.data?.length ?? 0} comercios`);
exigir(lista.data?.length > 0, "la base no tiene comercios");
check("listado sin descripción (liviano)", lista.data.every((e) => e.descripcion === undefined));
check(
  "el comercio viaja con las listas del modelo v2 (telefonos, redes_sociales, fotos)",
  lista.data.every((e) => Array.isArray(e.telefonos) && Array.isArray(e.redes_sociales) && Array.isArray(e.fotos)) &&
    lista.data.every((e) => !("telefono" in e) && !("instagram" in e) && !("facebook" in e)),
);
const filtrado = await call("GET", "/empresas?q=peluquer&limit=5");
check(
  "filtro q + limit (busca en nombre y rubro)",
  filtrado.data.length >= 1 && filtrado.data.length <= 5 && filtrado.data.every((e) => /peluquer/i.test(`${e.nombre} ${e.rubro}`)),
  `${filtrado.data.length} resultados`,
);
const rubros = await call("GET", "/rubros");
check("GET /rubros", rubros.data.length > 0, `${rubros.data.length} rubros`);
// No todos los rubros tienen comercios: se prueba con uno que sí tenga y con uno vacío si existe.
const conComercios = rubros.data.find((r) => r.cantidad > 0);
exigir(conComercios, "ningún rubro tiene comercios");
const porRubro = await call("GET", `/empresas/rubro/${conComercios.id}`);
check(
  "GET /empresas/rubro/:id trae los comercios de ese rubro",
  porRubro.data.length === conComercios.cantidad && porRubro.data.every((e) => e.rubro_id === conComercios.id),
  `${porRubro.data.length} en ${conComercios.nombre}`,
);
const vacio = rubros.data.find((r) => r.cantidad === 0);
if (vacio) check("un rubro sin comercios devuelve una lista vacía", (await call("GET", `/empresas/rubro/${vacio.id}`)).data.length === 0, vacio.nombre);

// comercio propio de negocio@demo.com
const mia = lista.data.find((e) => e.usuario_id === neg.data.user.id && e.turnos_habilitados);
check("negocio@demo.com tiene un comercio con turnos", !!mia, mia?.nombre);
exigir(mia, "negocio@demo.com no tiene un comercio con turnos habilitados");
const det = await call("GET", `/empresas/${mia.id}`);
check("ficha con teléfonos, redes y etiquetas", det.status === 200 && Array.isArray(det.data.telefonos) && Array.isArray(det.data.redes_sociales) && Array.isArray(det.data.etiquetas));
const hor = await call("GET", `/empresas/horarios/${mia.id}`);
check("horarios cargados", Object.keys(hor.data).length >= 1, Object.keys(hor.data).join(", "));

// ------------------------------------------------------------------ turnos (RF-03)
// Un día y una hora que el comercio atiende, a 20+ días: se toma la primera franja de su horario.
const DIAS = ["Domingo", "Lunes", "Martes", "Miércoles", "Jueves", "Viernes", "Sábado"];
const diaQueAtiende = DIAS.find((d) => hor.data[d]?.length);
exigir(diaQueAtiende, "el comercio de negocio@demo.com no tiene horarios");
const fecha = (() => { let d = new Date(Date.now() + 20 * 864e5); while (DIAS[d.getDay()] !== diaQueAtiende) d = new Date(+d + 864e5); return d.toISOString().slice(0, 10); })();
const [desde] = hor.data[diaQueAtiende][0].split("-");
const sumarMin = (hhmm, min) => { const [h, m] = hhmm.split(":").map(Number); const t = h * 60 + m + min; return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`; };
const paso = mia.duracion_turno_min ?? 30;
const hora = sumarMin(desde, paso), otraHora = sumarMin(desde, 2 * paso);
const reservar = (token, h, empresa_id = mia.id, f = fecha) => call("POST", "/turnos/reservar", { token, body: { empresa_id, fecha: f, hora: h } });
check("reservar con la cuenta del negocio → 403 (las reservas online son de clientes)", (await reservar(TN, otraHora)).status === 403);
const [r1, r2] = await Promise.all([reservar(TC, hora), reservar(TC, hora)]);
const estados = [r1.status, r2.status].sort();
check("RF-03: dos reservas simultáneas → una 200 y una 409", estados[0] === 200 && estados[1] === 409, `${diaQueAtiende} ${fecha} ${hora}: ${estados.join(",")}`);
const oc = await call("GET", `/turnos/ocupados?empresa_id=${mia.id}&fecha=${fecha}`);
check("el horario aparece ocupado una sola vez", oc.data.filter((o) => o.hora === hora).length === 1);
const sinTurnos = lista.data.find((e) => !e.turnos_habilitados);
if (sinTurnos) check("reservar en comercio sin turnos → 400", (await reservar(TC, "10:00", sinTurnos.id)).status === 400);
check("reservar en fecha pasada → 400", (await reservar(TC, "10:00", mia.id, "2020-01-01")).status === 400);
check("reservar sin login → 401", (await reservar(undefined, "10:00")).status === 401);
const mis = await call("GET", "/turnos/mis-turnos", { token: TC });
check("mis-turnos del cliente demo", mis.data.length >= 1, `${mis.data.length} turnos, estado '${mis.data[0]?.estado}'`);
const propio = mis.data.find((t) => t.fecha.startsWith(fecha) && t.hora.startsWith(hora));
check("cancelar un turno propio", propio && (await call("DELETE", `/turnos/cancelar/${propio.id}`, { token: TC })).status === 200);
if (propio) check("cancelar un turno ya cancelado → 404", (await call("DELETE", `/turnos/cancelar/${propio.id}`, { token: TC })).status === 404);

// ------------------------------------------------------------------ horarios (RF-04)
const nuevoHor = { Lunes: ["09:00-12:00", "16:00-20:00"], Sábado: ["09:00-13:00"] };
const put = await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: nuevoHor } });
check("dueño edita sus horarios", put.status === 200 && JSON.stringify(put.data.Sábado) === '["09:00-13:00"]');
check("cliente NO puede editar horarios → 403", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TC, body: { horarios: {} } })).status === 403);
// restaura los horarios originales para no dejar datos de ejemplo modificados
await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: hor.data } });
check("franja inválida → 400", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: { Lunes: ["20:00-10:00"] } } })).status === 400);
check("día inválido → 400", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: { Lunez: ["09:00-10:00"] } } })).status === 400);

// ------------------------------------------------------------------ ficha
const antes = det.data;
// Teléfonos, redes, fotos y etiquetas que no vienen en el cuerpo se conservan.
const fichaMinima = { nombre: antes.nombre, direccion: antes.direccion, slogan: antes.slogan, duracion_turno_min: antes.duracion_turno_min, rubro_id: antes.rubro_id, turnos_habilitados: antes.turnos_habilitados };
const upd = await call("PUT", `/empresas/${mia.id}`, { token: TN, body: fichaMinima });
check(
  "editar ficha no pierde dueño, descripción, teléfonos ni redes",
  upd.status === 200 && upd.data.usuario_id === antes.usuario_id && JSON.stringify(upd.data.telefonos) === JSON.stringify(antes.telefonos) && JSON.stringify(upd.data.redes_sociales) === JSON.stringify(antes.redes_sociales),
);
const sinPermiso = await call("PUT", `/empresas/${mia.id}`, { token: TC, body: { nombre: "x", direccion: "y", rubro_id: antes.rubro_id } });
check("cliente no edita comercio ajeno → 403 con mensaje de permisos", sinPermiso.status === 403 && /permisos/.test(sinPermiso.data?.error ?? ""), sinPermiso.data?.error);
const tels = await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: [{ numero: "2392111111", descripcion: "Turnos" }, { numero: "2392222222", principal: true }] } });
check("reemplazar teléfonos respeta el principal marcado", tels.status === 200 && tels.data[0].numero === "2392222222" && tels.data[0].principal);
check("teléfono repetido → 400", (await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: [{ numero: "2392111111" }, { numero: "2392111111" }] } })).status === 400);
check("teléfono con letras → 400", (await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: [{ numero: "2392abc111" }] } })).status === 400);
const restaurados = await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: antes.telefonos } });
check("teléfonos restaurados", restaurados.status === 200 && restaurados.data.length === antes.telefonos.length);

// ------------------------------------------------------------------ favoritos y calificaciones
await call("PUT", `/favoritos/${mia.id}`, { token: TC });
check("favorito agregado", (await call("GET", "/favoritos", { token: TC })).data.includes(mia.id));
await call("DELETE", `/favoritos/${mia.id}`, { token: TC });
check("favorito quitado", !(await call("GET", "/favoritos", { token: TC })).data.includes(mia.id));
check("favorito de empresa inexistente → 404", (await call("PUT", "/favoritos/99999999", { token: TC })).status === 404);
check("calificar sin turno → 400 (RF-13)", (await call("POST", `/empresas/${mia.id}/calificaciones`, { token: TC, body: { puntaje: 5, comentario: "Excelente" } })).status === 400);
check("puntaje fuera de rango → 400", (await call("POST", `/empresas/${mia.id}/calificaciones`, { token: TC, body: { turno_id: 1, puntaje: 9 } })).status === 400);
const resenas = await call("GET", `/empresas/${mia.id}/calificaciones`);
check("listado de reseñas coherente con la ficha (hasta 50)", Array.isArray(resenas.data) && resenas.data.length === Math.min(antes.calificacion_cantidad ?? 0, 50), `${resenas.data.length} reseñas, promedio ${antes.calificacion_promedio}`);

// ------------------------------------------------------------------ publicidades (RF-06)
const pubs = await call("GET", "/publicidades");
check("publicidades vigentes", Array.isArray(pubs.data) && pubs.data.every((p) => p.estado === "aprobada"), `${pubs.data.length} aprobadas`);
// url_imagen es obligatoria (columna NOT NULL de publicidad y campo obligatorio del formulario).
const IMAGEN = "https://example.com/imagen.jpg";
check("publicidad sin imagen → 400", (await call("POST", "/publicidades", { token: TN, body: { empresa_id: mia.id, titulo: "Sin imagen", descripcion: "x" } })).status === 400);
const nueva = await call("POST", "/publicidades", { token: TN, body: { empresa_id: mia.id, titulo: "Promo de prueba", descripcion: "2x1", url_imagen: IMAGEN } });
check("dueño crea publicidad (queda pendiente)", nueva.status === 201 && typeof nueva.data.id === "string" && nueva.data.estado === "pendiente" && nueva.data.url_imagen === IMAGEN, `${nueva.status} ${nueva.data?.error ?? ""}`);
if (nueva.status === 201) {
  check("cliente no borra publicidad ajena → 403", (await call("DELETE", `/publicidades/${nueva.data.id}`, { token: TC })).status === 403);
  check("dueño borra su publicidad", (await call("DELETE", `/publicidades/${nueva.data.id}`, { token: TN })).status === 200);
}

// ------------------------------------------------------------------ registro y contraseña
const email = `prueba${Date.now()}@test.com`;
check("registro", (await call("POST", "/auth/register", { body: { nombre: "Prueba", email, contraseña: "Clave1234" } })).status === 201);
check("registro con email repetido (otra capitalización) → 409", (await call("POST", "/auth/register", { body: { nombre: "Prueba", email: email.toUpperCase(), contraseña: "Clave1234" } })).status === 409);
check("registro con email mal formado → 400", (await call("POST", "/auth/register", { body: { nombre: "Prueba", email: "a..b@test.com", contraseña: "Clave1234" } })).status === 400);
const olvido = await call("POST", "/auth/forgot-password", { body: { email } });
check("forgot-password responde igual", olvido.status === 200);
check("reset con token inválido → 400", (await call("POST", "/auth/reset-password", { body: { token: "nope", contraseña: "Nueva1234" } })).status === 400);

// ------------------------------------------------------------------ superadmin
check("superadmin lista usuarios", (await call("GET", "/usuarios", { token: TA })).status === 200);
check("cliente NO lista usuarios → 403", (await call("GET", "/usuarios", { token: TC })).status === 403);
const rubro = await call("POST", "/rubros", { token: TA, body: { nombre: "Rubro de prueba " + Date.now() } });
check("superadmin crea rubro", rubro.status === 201 || rubro.status === 200, String(rubro.status));
if (rubro.data?.id) check("superadmin borra el rubro vacío", (await call("DELETE", `/rubros/${rubro.data.id}`, { token: TA })).status === 200);

console.log(fallos === 0 ? "\nTODO OK" : `\n${fallos} FALLOS`);
process.exit(fallos ? 1 : 0);
