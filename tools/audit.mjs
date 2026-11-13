// Auditoría de seguridad y robustez del backend (complementa smoke.mjs, que prueba el camino feliz).
// Cada control describe el comportamiento CORRECTO; "FALLA" = hay un problema a corregir.
import crypto from "node:crypto";

const HOST = process.env.API ?? "http://127.0.0.1:3001";
const B = HOST + "/api";
let fallos = 0;
const hallazgos = [];

async function call(method, path, { token, body, raw, headers = {} } = {}) {
  const t0 = Date.now();
  const res = await fetch(B + path, {
    method,
    headers: { ...(body || raw ? { "Content-Type": "application/json" } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}), ...headers },
    body: raw ?? (body ? JSON.stringify(body) : undefined),
  });
  const text = await res.text();
  let data = null;
  try { data = JSON.parse(text); } catch {}
  return { status: res.status, data, text, ms: Date.now() - t0, headers: res.headers };
}
function check(nombre, cond, detalle = "") {
  console.log(`${cond ? "OK   " : "FALLA"} ${nombre}${detalle ? "  → " + detalle : ""}`);
  if (!cond) { fallos++; hallazgos.push(nombre); }
}
const login = async (email, pass = "Demo1234!") => call("POST", "/auth/login", { body: { email, contraseña: pass } });
const jwt = (payload, secret) => {
  const b64 = (o) => Buffer.from(JSON.stringify(o)).toString("base64url");
  const head = b64({ alg: "HS256", typ: "JWT" }), body = b64(payload);
  return `${head}.${body}.${crypto.createHmac("sha256", secret).update(`${head}.${body}`).digest("base64url")}`;
};

const cli = (await login("cliente@demo.com")).data;
const negocio = (await login("negocio@demo.com")).data;
if (!cli?.token || !negocio?.token) {
  console.error(`No se pudo iniciar sesión con las cuentas de demostración en ${B}: ¿está el backend corriendo y cargados los seeds?`);
  process.exit(1);
}
const TC = cli.token;
const TN = negocio.token;

// Comercio de prueba: el de negocio@demo.com con turnos habilitados. Si esa cuenta no tuviera uno,
// cualquiera con turnos habilitados. No depende del nombre ni de la cantidad de comercios cargados.
const emps = (await call("GET", "/empresas?limit=1000")).data ?? [];
const deNegocio = emps.filter((e) => e.usuario_id === negocio.user.id);
const propia = deNegocio.find((e) => e.turnos_habilitados) ?? deNegocio[0];
const conTurnos = propia?.turnos_habilitados ? propia : emps.find((e) => e.turnos_habilitados);
if (!propia || !conTurnos) {
  console.error(`Faltan datos para auditar: ${!propia ? "negocio@demo.com no tiene comercio" : "ningún comercio tiene turnos habilitados"} (${emps.length} comercios cargados).`);
  process.exit(1);
}
console.log(`Comercio de prueba: ${propia.nombre} (#${propia.id})${conTurnos !== propia ? `; turnos en ${conTurnos.nombre}` : ""}`);

console.log("== Autorización ==");
const email = `audit${Date.now()}@test.com`;
const reg = await call("POST", "/auth/register", { body: { nombre: "Intruso", email, contraseña: "Clave1234", rol: "superadmin" } });
const tokEsc = (await login(email, "Clave1234")).data?.token;
const puedeListar = tokEsc ? (await call("GET", "/usuarios", { token: tokEsc })).status === 200 : false;
check("el registro público NO permite elegir rol (escalada a superadmin)", !(reg.status === 201 && puedeListar), reg.status === 201 && puedeListar ? "un anónimo se registró como superadmin y lista usuarios" : "");

const forjado = jwt({ id: 1, nombre: "x", rol: "superadmin", exp: Math.floor(Date.now() / 1000) + 3600 }, "supersecreto");
check("un token firmado con el secreto por defecto no es aceptado", (await call("GET", "/usuarios", { token: forjado })).status === 401, "el JWT_SECRET por defecto permite forjar superadmin");
check("token con alg=none rechazado", (await call("GET", "/usuarios", { token: `${Buffer.from('{"alg":"none"}').toString("base64url")}.${Buffer.from('{"id":1,"nombre":"x","rol":"superadmin","exp":9999999999}').toString("base64url")}.` })).status === 401);
check("token vencido rechazado", (await call("GET", "/usuarios", { token: jwt({ id: 1, nombre: "x", rol: "superadmin", exp: 1000 }, "otro") })).status === 401);

const borrarAjena = await call("DELETE", `/empresas/${propia.id}`, { token: TC });
check("un cliente no puede borrar un comercio ajeno (403 con mensaje de permisos)", borrarAjena.status === 403 && /permisos/.test(borrarAjena.data?.error ?? ""), borrarAjena.data?.error);
check("calificar el propio comercio no está permitido", (await call("POST", `/empresas/${propia.id}/calificaciones`, { token: TN, body: { turno_id: 0, puntaje: 5 } })).status === 403);
check("un cliente no puede crear comercios a nombre de otro (usuario_id ignorado)", await (async () => {
  const r = await call("POST", "/empresas", { token: TC, body: { nombre: "Test", direccion: "x", rubro_id: propia.rubro_id, usuario_id: 1 } });
  if (r.status !== 201) return true;
  const ok = r.data.usuario_id === cli.user.id; await call("DELETE", `/empresas/${r.data.id}`, { token: TC }); return ok;
})());

console.log("\n== Autenticación ==");
const a = await login("cliente@demo.com", "mala"), b = await login("noexiste@demo.com", "mala");
check("login no revela si el email existe", a.data?.error === b.data?.error, `"${a.data?.error}" vs "${b.data?.error}"`);
check("registro rechaza contraseñas cortas", (await call("POST", "/auth/register", { body: { nombre: "A", email: `c${Date.now()}@t.com`, contraseña: "1" } })).status === 400);
for (const malo of ["no-es-email", "a..b@test.com", "a@test", "a@-test.com", "a,b@test.com"]) {
  check(`registro rechaza el email inválido "${malo}"`, (await call("POST", "/auth/register", { body: { nombre: "A", email: malo, contraseña: "Clave1234" } })).status === 400);
}
const bruto = `bruto${Date.now()}@test.com`;
await call("POST", "/auth/register", { body: { nombre: "Bruto", email: bruto, contraseña: "Clave1234" } });
const t0 = Date.now();
await Promise.all(Array.from({ length: 4 }, () => login(bruto, "mala")));
const durLogins = Date.now() - t0;
const salud = await call("GET", "/empresas?limit=1");
const conc = Date.now();
const [, sano] = await Promise.all([Promise.all(Array.from({ length: 4 }, () => login(bruto, "mala"))), (async () => { const s = Date.now(); await fetch(HOST + "/health"); return Date.now() - s; })()]);
check("bcrypt no bloquea el servidor (health responde rápido durante 8 logins)", sano < 400, `health tardó ${sano} ms con logins en paralelo (4 logins: ${durLogins} ms)`);
check("sin límite de intentos de login (fuerza bruta)", (await Promise.all(Array.from({ length: 12 }, () => login(bruto, "mala")))).some((r) => r.status === 429), "12 intentos seguidos y nunca hubo 429");
check("una cuenta bloqueada no impide entrar a las demás", (await login("cliente@demo.com")).status === 200);

console.log("\n== Validación de entrada ==");
check("limit negativo → no es un 500", (await call("GET", "/empresas?limit=-5")).status < 500);
check("offset negativo → no es un 500", (await call("GET", "/empresas?offset=-5")).status < 500);
check("limit enorme se acota", (await call("GET", "/empresas?limit=99999999")).data.length <= 1000, "devuelve todo el catálogo si piden un limit gigante");
check("inyección SQL en q no rompe nada", (await call("GET", "/empresas?q=" + encodeURIComponent("'; DROP TABLE empresas;--"))).status === 200 && (await call("GET", "/empresas?limit=1")).status === 200);
check("id o slug inexistente → 404", (await call("GET", "/empresas/abc")).status === 404);
check("JSON inválido → respuesta JSON con error", (await call("POST", "/auth/login", { raw: "{malo" })).data !== null, "el cuerpo del error no es JSON (el front espera {error})");
check("cuerpo enorme rechazado", await (async () => { try { return [413, 400].includes((await call("POST", "/auth/login", { raw: JSON.stringify({ email: "a", contraseña: "x".repeat(3_000_000) }) })).status); } catch { return true; } })());
check("contraseña larguísima no cuelga bcrypt", (await login("cliente@demo.com", "x".repeat(100_000))).ms < 2000);
const url = await call("PUT", `/empresas/${propia.id}`, { token: TN, body: { nombre: propia.nombre, direccion: propia.direccion, rubro_id: propia.rubro_id, redes_sociales: [{ tipo: "instagram", url: "javascript:alert(1)" }] } });
check("las redes sociales solo aceptan http(s)", url.status === 400, "se guardó una URL javascript: (XSS si el front la usa como href)");
check("imagen de publicidad solo acepta http(s)", (await call("POST", "/publicidades", { token: TN, body: { empresa_id: propia.id, titulo: "x", descripcion: "x", url_imagen: "javascript:alert(1)" } })).status === 400);
check("teléfono con letras rechazado", (await call("PUT", `/empresas/${propia.id}/telefonos`, { token: TN, body: { telefonos: [{ numero: "javascript:1" }] } })).status === 400);

console.log("\n== Reglas de negocio ==");
// Fechas y horas elegidas según el horario real del comercio de prueba (no se suponen días fijos).
const proximo = (diaSemana, desde) => { let d = new Date(Date.now() + desde * 864e5); while (d.getDay() !== diaSemana) d = new Date(+d + 864e5); return d.toISOString().slice(0, 10); };
const hor = (await call("GET", `/empresas/horarios/${conTurnos.id}`)).data;
const DIAS = ["Domingo", "Lunes", "Martes", "Miércoles", "Jueves", "Viernes", "Sábado"];
const cerrado = DIAS.findIndex((d) => !hor[d]?.length);
const abierto = DIAS.findIndex((d) => hor[d]?.length);
const reservarEn = (fecha, hora) => call("POST", "/turnos/reservar", { token: TC, body: { empresa_id: conTurnos.id, fecha, hora } });
if (abierto < 0) {
  check("el comercio de prueba tiene horarios cargados", false, conTurnos.nombre);
} else {
  const [apertura] = hor[DIAS[abierto]][0].split("-");
  const paso = conTurnos.duracion_turno_min ?? 30;
  const sumarMin = (hhmm, min) => { const [h, m] = hhmm.split(":").map(Number); const t = h * 60 + m + min; return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`; };
  if (cerrado >= 0) {
    const fechaCerrado = proximo(cerrado, 10);
    check("no se reserva un día en que el comercio no atiende", (await reservarEn(fechaCerrado, apertura)).status === 400, `${DIAS[cerrado]} ${fechaCerrado}, atiende: ${Object.keys(hor).join(",")}`);
  }
  const dia = proximo(abierto, 12);
  check("no se reserva de madrugada (fuera de horario)", (await reservarEn(dia, "03:00")).status === 400);
  const desalineada = sumarMin(apertura, 7);
  const rr = await reservarEn(dia, desalineada);
  check("no se reservan horarios desalineados de la grilla del comercio", rr.status === 400, `${desalineada} → ${rr.status}`);
  check("cancelar conserva el historial (estado 'cancelado') y libera el horario", await (async () => {
    const f = proximo(abierto, 13 + 7);
    const h = sumarMin(apertura, paso);
    const r = await reservarEn(f, h);
    if (r.status !== 200) return false;
    const mis = (await call("GET", "/turnos/mis-turnos", { token: TC })).data.find((t) => t.fecha.startsWith(f) && t.hora.startsWith(h));
    await call("DELETE", `/turnos/cancelar/${mis.id}`, { token: TC });
    const con = (await call("GET", "/turnos/mis-turnos?incluir_cancelados=true", { token: TC })).data.find((t) => t.id === mis.id);
    const sin = (await call("GET", "/turnos/mis-turnos", { token: TC })).data.some((t) => t.id === mis.id);
    const libre = !(await call("GET", `/turnos/ocupados?empresa_id=${conTurnos.id}&fecha=${f}`)).data.some((o) => o.hora === h);
    return con?.estado === "cancelado" && !sin && libre;
  })());
}

console.log("\n== Operación ==");
check("CORS no es abierto a cualquier origen", (await fetch(B + "/empresas?limit=1", { headers: { Origin: "https://sitio-malicioso.com" } })).headers.get("access-control-allow-origin") !== "*");
check("/health verifica la base", (await fetch(HOST + "/health")).ok);
const raiz = await (await fetch(HOST + "/")).text();
check("la raíz no expone datos internos", !/postgres|secret|panic/i.test(raiz));

console.log(fallos ? `\n${fallos} hallazgos:\n - ${hallazgos.join("\n - ")}` : "\nSIN HALLAZGOS");
process.exit(fallos ? 1 : 0);
