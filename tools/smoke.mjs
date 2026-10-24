// Prueba de humo de punta a punta contra un backend corriendo (por defecto http://127.0.0.1:3001).
// Requiere la base con los seeds cargados (ver tools/devdb.mjs). Uso: node smoke.mjs
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
const login = async (email, pass = "Demo1234!") => (await call("POST", "/auth/login", { body: { email, contraseña: pass } }));

const cli = await login("cliente@demo.com");
const neg = await login("negocio@demo.com");
const adm = await login("admin@demo.com");
check("login cliente/negocio/admin", cli.status === 200 && neg.status === 200 && adm.status === 200);
check("login con clave incorrecta → 401", (await login("cliente@demo.com", "mala")).status === 401);
check("login sin distinguir mayúsculas en el email", (await login("CLIENTE@demo.com")).status === 200);
const TC = cli.data.token, TN = neg.data.token, TA = adm.data.token;

const lista = await call("GET", "/empresas");
check("GET /empresas devuelve todo el catálogo", lista.data.length > 3800, `${lista.data.length} comercios`);
check("listado sin descripción (liviano)", lista.data.every((e) => e.descripcion === undefined));
const filtrado = await call("GET", "/empresas?q=peluquer&limit=5");
check("filtro q + limit", filtrado.data.length === 5 && filtrado.data.every((e) => /peluquer/i.test(e.nombre)));
const cats = await call("GET", "/categorias");
check("GET /categorias", cats.data.length > 400, `${cats.data.length} rubros`);
const porCat = await call("GET", `/empresas/categoria/${cats.data.find((c) => c.nombre === "Abogados").id}`);
check("GET /empresas/categoria/:id", porCat.data.length > 0, `${porCat.data.length} abogados`);

// comercio propio de negocio@demo.com
const mia = lista.data.find((e) => e.admin_id === neg.data.user.id);
check("negocio@demo.com tiene un comercio con turnos", mia && mia.turnos_habilitados, mia?.nombre);
const det = await call("GET", `/empresas/${mia.id}`);
check("ficha con teléfonos, etiquetas, descripción", Array.isArray(det.data.telefonos) && Array.isArray(det.data.etiquetas));
const hor = await call("GET", `/empresas/horarios/${mia.id}`);
check("horarios cargados", Object.keys(hor.data).length >= 5);

// RF-03: dos reservas simultáneas al mismo horario
// martes a 20+ días: el comercio de ejemplo atiende de lunes a viernes, con turnos de 15 min
const fecha = (() => { let d = new Date(Date.now() + 20 * 864e5); while (d.getDay() !== 2) d = new Date(+d + 864e5); return d.toISOString().slice(0, 10); })();
const reservar = (token, hora, empresa_id = mia.id, f = fecha) => call("POST", "/turnos/reservar", { token, body: { empresa_id, fecha: f, hora } });
const [r1, r2] = await Promise.all([reservar(TC, "15:15"), reservar(TN, "15:15")]);
const estados = [r1.status, r2.status].sort();
check("RF-03: dos reservas simultáneas → una 200 y una 409", estados[0] === 200 && estados[1] === 409, estados.join(","));
const oc = await call("GET", `/turnos/ocupados?empresa_id=${mia.id}&fecha=${fecha}`);
check("el horario aparece ocupado una sola vez", oc.data.filter((o) => o.hora === "15:15").length === 1);
const sinTurnos = lista.data.find((e) => !e.turnos_habilitados);
check("reservar en comercio sin turnos → 400", (await reservar(TC, "10:00", sinTurnos.id)).status === 400);
check("reservar en fecha pasada → 400", (await reservar(TC, "10:00", mia.id, "2020-01-01")).status === 400);
check("reservar sin login → 401", (await reservar(undefined, "10:00")).status === 401);
const mis = await call("GET", "/turnos/mis-turnos", { token: TC });
check("mis-turnos del cliente demo", mis.data.length >= 3, `${mis.data.length} turnos, estado '${mis.data[0]?.estado}'`);
const ganador = r1.status === 200 ? TC : TN;
const propio = (await call("GET", "/turnos/mis-turnos", { token: ganador })).data.find((t) => t.hora.startsWith("15:15"));
check("cancelar un turno propio", (await call("DELETE", `/turnos/cancelar/${propio.id}`, { token: ganador })).status === 200);
check("cancelar turno ajeno → 404", (await call("DELETE", `/turnos/cancelar/${propio.id}`, { token: ganador })).status === 404);

// horarios editables (RF-04)
const nuevoHor = { Lunes: ["09:00-12:00", "16:00-20:00"], Sábado: ["09:00-13:00"] };
const put = await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: nuevoHor } });
check("dueño edita sus horarios", put.status === 200 && JSON.stringify(put.data.Sábado) === '["09:00-13:00"]');
check("cliente NO puede editar horarios → 403", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TC, body: { horarios: {} } })).status === 403);
// restaura los horarios originales para no dejar datos de ejemplo modificados
await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: hor.data } });
check("franja inválida → 400", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: { Lunes: ["20:00-10:00"] } } })).status === 400);
check("día inválido → 400", (await call("PUT", `/empresas/${mia.id}/horarios`, { token: TN, body: { horarios: { Lunez: ["09:00-10:00"] } } })).status === 400);

// edición de la ficha conserva lo que no se envía
const antes = (await call("GET", `/empresas/${mia.id}`)).data;
// un PUT reemplaza los campos opcionales que no se envían: se manda la ficha completa
const fichaCompleta = { nombre: antes.nombre, direccion: antes.direccion, telefono: antes.telefono, slogan: antes.slogan, duracion_turno_min: antes.duracion_turno_min, categoria_id: antes.categoria_id, instagram: antes.instagram, facebook: antes.facebook, turnos_habilitados: antes.turnos_habilitados };
const upd = await call("PUT", `/empresas/${mia.id}`, { token: TN, body: fichaCompleta });
check("editar ficha no pierde dueño ni descripción", upd.status === 200 && upd.data.admin_id === antes.admin_id && upd.data.descripcion === antes.descripcion);
check("cliente no edita comercio ajeno → 403", (await call("PUT", `/empresas/${mia.id}`, { token: TC, body: { nombre: "x", direccion: "y", categoria_id: 1 } })).status === 403);
const tels = await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: [{ numero: "2392111111", etiqueta: "Turnos" }, { numero: "2392111111" }] } });
check("teléfonos con número repetido no rompe", tels.status === 200 && tels.data.length === 1);
await call("PUT", `/empresas/${mia.id}/telefonos`, { token: TN, body: { telefonos: antes.telefonos.map((t) => ({ numero: t.numero, etiqueta: t.etiqueta })) } });

// favoritos, calificaciones
await call("PUT", `/favoritos/${mia.id}`, { token: TC });
check("favorito agregado", (await call("GET", "/favoritos", { token: TC })).data.includes(mia.id));
await call("DELETE", `/favoritos/${mia.id}`, { token: TC });
check("favorito quitado", !(await call("GET", "/favoritos", { token: TC })).data.includes(mia.id));
check("favorito de empresa inexistente → 404", (await call("PUT", "/favoritos/99999999", { token: TC })).status === 404);
check("calificar sin turno → 400 (RF-13)", (await call("POST", `/empresas/${mia.id}/calificaciones`, { token: TC, body: { puntaje: 5, comentario: "Excelente" } })).status === 400);
check("puntaje fuera de rango → 400", (await call("POST", `/empresas/${mia.id}/calificaciones`, { token: TC, body: { turno_id: 1, puntaje: 9 } })).status === 400);
const conRating = (await call("GET", `/empresas/${mia.id}`)).data;
check("promedio de calificaciones visible en la ficha", conRating.calificacion_cantidad >= 1, `${conRating.calificacion_promedio} (${conRating.calificacion_cantidad})`);
check("listado de reseñas", (await call("GET", `/empresas/${mia.id}/calificaciones`)).data.length >= 1);

// publicidades
const pubs = await call("GET", "/publicidades");
check("publicidades de ejemplo", pubs.data.length >= 12);
const nueva = await call("POST", "/publicidades", { token: TN, body: { empresa_id: mia.id, titulo: "Promo de prueba", descripcion: "2x1" } });
check("dueño crea publicidad", nueva.status === 201 && typeof nueva.data.id === "string");
check("cliente no borra publicidad ajena → 403", (await call("DELETE", `/publicidades/${nueva.data.id}`, { token: TC })).status === 403);
check("dueño borra su publicidad", (await call("DELETE", `/publicidades/${nueva.data.id}`, { token: TN })).status === 200);

// registro + recuperación de contraseña
const email = `prueba${Date.now()}@test.com`;
check("registro", (await call("POST", "/auth/register", { body: { nombre: "Prueba", email, contraseña: "clave123" } })).status === 201);
check("registro con email repetido (otra capitalización) → 400", (await call("POST", "/auth/register", { body: { nombre: "Prueba", email: email.toUpperCase(), contraseña: "clave123" } })).status === 400);
const olvido = await call("POST", "/auth/forgot-password", { body: { email } });
check("forgot-password responde igual", olvido.status === 200);
check("reset con token inválido → 400", (await call("POST", "/auth/reset-password", { body: { token: "nope", contraseña: "nueva123" } })).status === 400);

// permisos de superadmin
check("superadmin lista usuarios", (await call("GET", "/usuarios", { token: TA })).status === 200);
check("cliente NO lista usuarios → 403", (await call("GET", "/usuarios", { token: TC })).status === 403);
const cat = await call("POST", "/categorias", { token: TA, body: { nombre: "Rubro de prueba " + Date.now() } });
check("superadmin crea rubro", cat.status === 201 || cat.status === 200, String(cat.status));

console.log(fallos === 0 ? "\nTODO OK" : `\n${fallos} FALLOS`);
process.exit(fallos ? 1 : 0);
